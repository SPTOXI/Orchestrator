# ADR-0025 — Motor local próprio (llama.cpp), sem Ollama

- **Estado:** Aceita
- **Fase:** depois da 12 (pedido do usuário)
- **Substitui:** a seção "Modelos offline (Ollama)" da
  [ADR-0021](./0021-configuracoes-assinaturas-offline-e-mcp.md) (item 5)

## Contexto

O usuário baixou o phi4-mini pelo Ollama e pediu uma tarefa ao modelo. A
resposta foi: "Como uma IA desenvolvida pela Microsoft, não estou conectada
localmente em seu computador nem acesso arquivos…".

A causa é o **contexto**:

- A cada pedido, o Orchestrator manda ao modelo as instruções, as regras,
  as skills, o contexto do projeto e as definições das ferramentas. Só as 51
  ferramentas do runtime somam cerca de 7.900 tokens. Com as de tasks,
  agentes, projetos e MCP, o total passa de 10 mil.
- O Ollama roda os modelos com 4.096 tokens de contexto na configuração
  padrão. A API compatível com OpenAI, que o Orchestrator usa, não deixa
  mudar esse valor por pedido.
- Quando o pedido passa do limite, o Ollama corta o começo sem avisar. O
  modelo recebe só a última mensagem, sem instruções nem ferramentas, e
  responde como um chat genérico.

O usuário perguntou por que depender do Ollama. A vantagem dele é
compartilhar modelos com outros programas (Open WebUI, Continue, Cline…) e
servir a partir de outro computador. O usuário não usa nada disso e
escolheu um motor próprio, sem Ollama.

## Decisão

### Motor: llama.cpp gerenciado pelo Orchestrator

O `llama-server` do [llama.cpp](https://github.com/ggml-org/llama.cpp) é o
motor que o próprio Ollama usa por baixo. O Orchestrator baixa, instala,
liga e desliga esse motor, sem nada a instalar à parte.

- **De onde:** os releases oficiais de `ggml-org/llama.cpp` no GitHub.
  Instala o release mais recente e guarda qual foi. "Atualizar motor"
  aparece quando sai um release novo, porque famílias novas de modelos
  pedem um motor novo.
- **Qual pacote:** escolhido pelo sistema e pela placa de vídeo. O
  usuário pode trocar.

  | Sistema | Automático | Opções |
  | ------- | ---------- | ------ |
  | Windows x64 | NVIDIA (`nvcuda.dll`): `win-cuda-12.4-x64`, mais as DLLs `cudart-llama-bin-win-cuda-12.4-x64`; senão `win-vulkan-x64` | CPU, Vulkan, CUDA |
  | Windows arm64 | `win-cpu-arm64` | CPU, Vulkan |
  | macOS Apple Silicon | `macos-arm64` (Metal) | — |
  | macOS Intel | `macos-x64` | — |
  | Linux x64 | `ubuntu-vulkan-x64` se houver `libvulkan.so.1`; senão `ubuntu-x64` | CPU, Vulkan |
  | Linux arm64 | `ubuntu-vulkan-arm64` se houver Vulkan; senão `ubuntu-arm64` | CPU, Vulkan |

  Os pacotes com GPU também trazem a CPU: sem placa compatível, o motor
  roda no processador.
- **Conferência:** o download confere o tamanho e o SHA-256 que a API do
  GitHub publica para cada arquivo (`digest`). Sem conferência, nada é
  instalado.
- **Onde:** `<dados>/local/engine/<tag>-<pacote>/`, com `engine.json`
  descrevendo o que está instalado. Atualizar troca a pasta e apaga a
  antiga só depois que a nova funciona.
- **Espelhos:** `ORCHESTRATOR_ENGINE_API` (uma API compatível com a do
  GitHub, com os releases do llama.cpp) e `HF_ENDPOINT` (a mesma variável
  das ferramentas do Hugging Face) trocam as origens, para redes que só
  alcançam um espelho.

### Modelos: arquivos GGUF

Quatro origens. Todas viram entradas em `<dados>/local/models.json`:

1. **Catálogo:** modelos que funcionam com as ferramentas do
   Orchestrator, apontando para repositórios do Hugging Face e uma
   quantização (Q4_K_M, na maioria). O arquivo é escolhido pela lista do
   repositório na hora de baixar, e modelos divididos em partes vêm
   inteiros.
2. **Hugging Face:** qualquer repositório público (`dono/repo`). O app
   lista os `.gguf` e o usuário escolhe um.
3. **Importar do Ollama:** os modelos que o Ollama já baixou são GGUF.
   O app lê os manifests (`$OLLAMA_MODELS` ou `~/.ollama/models`, e
   `/usr/share/ollama/.ollama/models` no Linux) e cria um **link físico**
   para o arquivo do modelo na pasta do Orchestrator, sem baixar e sem
   ocupar espaço de novo. Se não der (outro disco), copia. O modelo
   continua funcionando mesmo depois de desinstalar o Ollama.
4. **Arquivo do disco:** um `.gguf` qualquer, usado no lugar onde está.

Os downloads mostram o progresso, continuam de onde pararam
(`.part` + `Range`), podem ser cancelados e conferem o SHA-256 que o
Hugging Face publica (`lfs.oid`). Os arquivos ficam em
`<dados>/local/models/<id>/`.

O app lê o cabeçalho GGUF de cada modelo: arquitetura, nome, contexto de
treino, quantização e o modelo de conversa (`tokenizer.chat_template`).
Se o modelo de conversa fala de ferramentas, o modelo recebe as
ferramentas pela API. Se não, elas vão pelo prompt, como já acontecia.

### Contexto: definido pelo Orchestrator

Cada modelo tem um **contexto** que o usuário pode mudar. O padrão é
32.768 tokens com 24 GB de memória ou mais, e 16.384 abaixo disso, sem
passar do contexto de treino. A tela avisa quando o valor fica abaixo de
16.384, porque as instruções e ferramentas ocupam cerca de 10 mil tokens.

O valor vai para o `llama-server` (`-c`) e para a conexão (`contextWindow`),
para a compactação da ADR-0018 saber o limite real.

### Ligar, trocar e desligar

- A conexão **"Modelos locais"** (`id: local`) é criada e mantida pelo app,
  com os modelos do registro. Ela é marcada como local: antes de cada
  chamada, o provider pede ao motor o endereço do modelo
  (`LocalEndpoint::acquire`), e o motor liga o `llama-server` com ele se
  preciso:

  ```text
  llama-server -m <arquivo> --host 127.0.0.1 --port <livre> -c <contexto>
               --alias <id> --jinja --no-webui [-ngl 0]
  ```

- **Placa de vídeo:** com "automático", o llama.cpp põe na placa o que
  couber (`--fit`). Com "desligada", passa `-ngl 0`.
- **Um modelo por vez:** pedir outro modelo espera as chamadas em
  andamento terminarem e troca. A troca nunca corta uma resposta no meio.
- **Prazos:** o motor espera até 10 minutos pelo `/health`, porque modelos
  grandes demoram a carregar. Se o processo sair antes, o erro traz as
  últimas linhas do registro dele (ex.: memória insuficiente, arquitetura
  desconhecida, com a dica de atualizar o motor).
- **Ocioso:** desliga depois de 10 minutos sem chamadas (configurável),
  para devolver a memória.
- **Sem órfãos:** o processo entra na supervisão da ADR-0018 (Job Object
  no Windows, registro de processos no Linux e no macOS).
- O app guarda as últimas 200 linhas do registro do motor e mostra na
  tela.

### Sem Ollama

O módulo `ollama`, os comandos `offline_*` e a tela antiga saem do app.

A conexão `ollama` que o app criou antes **não é apagada**, porque é dado
do usuário (ADR-0022). A seção Modelos locais avisa que ela existe e
oferece "Importar do Ollama" e "Remover a conexão antiga".

### Histórico

| Evento | Quando | `data` |
| ------ | ------ | ------ |
| `LOCAL_ENGINE_INSTALLED` | motor instalado ou atualizado | `tag`, `variant`, `from` |
| `LOCAL_ENGINE_REMOVED` | motor removido | `tag`, `variant` |
| `LOCAL_MODEL_ADDED` | modelo baixado, importado ou adicionado | `id`, `source`, `size`, `sha256` |
| `LOCAL_MODEL_REMOVED` | modelo removido | `id` |
| `LOCAL_MODEL_LOADED` | o motor ligou um modelo | `id`, `context`, `gpu`, `ms` |

## Consequências

- Os modelos locais recebem as instruções e as ferramentas inteiras.
  Modelos pequenos (3 a 4B) continuam fracos com ferramentas; os do
  catálogo de 7 a 8B em diante funcionam bem.
- Nada a instalar à parte. O primeiro uso baixa o motor, de 30 a 600 MB
  conforme o pacote (o CUDA é o maior).
- O Orchestrator passa a acompanhar os releases do llama.cpp. Um modelo de
  família nova pode pedir "Atualizar motor".
- Quem usava o Ollama importa os modelos uma vez e pode desinstalá-lo.
- Os binários do llama.cpp não são assinados pelo Orchestrator. A
  confiança vem do release oficial no GitHub, do TLS e da conferência do
  SHA-256.

### Fica para depois

- Mais de um modelo carregado ao mesmo tempo.
- Repositórios privados ou restritos do Hugging Face (token).
- Modelos de visão (`mmproj`) e de embeddings.
- Servir o motor para outro computador da rede.

# packages/local

**Modelos locais com o motor do próprio Orchestrator**: crate
`orchestrator-local`
([ADR-0025](../../docs/adr/0025-motor-local-proprio.md)). Referência de uso:
[`docs/settings.md`](../../docs/settings.md#modelos-locais).

Roda modelos de IA no computador do usuário com o `llama-server` do
[llama.cpp](https://github.com/ggml-org/llama.cpp), sem nada a instalar à
parte. O Orchestrator controla o contexto de cada modelo.

| Módulo | O que faz |
| ------ | --------- |
| `platform.rs` | sistema, processador, placa (NVIDIA, Vulkan), memória; o pacote do llama.cpp para cada um; o contexto padrão |
| `sources.rs` | releases do llama.cpp no GitHub, arquivos de um repositório do Hugging Face (quantização, modelos em partes), o catálogo, os modelos que o Ollama baixou |
| `download.rs` | downloads com `.part`, `Range`, tamanho e SHA-256 conferidos, progresso e cancelamento |
| `archive.rs` | `.zip` e `.tar.gz` sem caminhos que saiam da pasta; acha o `llama-server` e as pastas de bibliotecas |
| `engine.rs` | instala um pacote (baixa, confere, extrai, testa com `--version`), `engine.json`, remove; o comando com as bibliotecas no caminho |
| `gguf.rs` | o cabeçalho de um GGUF: arquitetura, nome, contexto de treino, quantização, KV por token, modelo de conversa (ferramentas) |
| `store.rs` | `models.json` e `settings.json`, gravados de forma atômica; ids únicos |
| `server.rs` | o `llama-server` rodando: um modelo por vez, troca quando nenhuma chamada usa o atual, desliga quando ocioso, erros com dica a partir do registro |
| `lib.rs` | `LocalEngine`: tudo junto, a conexão `local` e o `LocalEndpoint` do provider de API |

Depende de `orchestrator-provider-api` (a conexão e o `LocalEndpoint`) e
`orchestrator-providers`. A supervisão do processo (ADR-0018) entra pelo
trait `ProcessWatch`, que o app liga ao runtime.

Testes: `cargo test -p orchestrator-local`. Os de integração usam
`fake-llama-server` (`src/bin`), um substituto do `llama-server` com
`/health` e `/v1/chat/completions`, um GitHub e um Hugging Face simulados,
e cobrem:

- instalar o motor a partir de um release (SHA-256, extração, teste);
- baixar do catálogo e do Hugging Face, importar do Ollama, usar um
  arquivo;
- o contexto de cada modelo chegando ao motor e à conexão;
- uma chamada pela conexão local, a troca de modelo e a placa desligada;
- um modelo que o motor não conhece, com a dica de atualizar;
- downloads que continuam e arquivos que não conferem;
- a troca que espera as chamadas e o desligamento por ociosidade.

# ADR-0027 — IA Nativa: motor próprio, modelo próprio, operando o Orchestrator e o computador

- **Estado:** Proposta
- **Fase:** depois da 12 (pedido do usuário)
- **Depende de:** [ADR-0016](./0016-autonomia-e-pause.md) (autonomia),
  [ADR-0020](./0020-acesso-total-da-ia.md) (acesso total),
  [ADR-0026](./0026-economia-de-tokens-nas-ferramentas-e-nos-pedidos.md)
  (economia de tokens)
- **Relação com a [ADR-0025](./0025-motor-local-proprio.md):** o llama.cpp
  continua servindo modelos de terceiros; a IA Nativa **não** depende dele

## Contexto

O usuário pediu uma **IA própria do Orchestrator**. A base do pedido é:

- **motor próprio**: o código que executa o modelo é do Orchestrator, não
  um programa de terceiros baixado (llama.cpp, Ollama, ONNX Runtime);
- **modelo próprio**: arquitetura, tokenizador, pesos e treino do
  Orchestrator, não um modelo de terceiros escolhido num catálogo.

E a IA precisa:

1. rodar **e ser treinada** em notebooks com **8 GB de RAM e 1 GB de
   vídeo**, offline;
2. falar e aprender **a língua que o usuário quiser**;
3. **gerenciar o Orchestrator** (projetos, tasks, agentes, providers,
   autonomia, backups, atualizações);
4. **gerenciar o computador**, podendo fazer tudo o que o usuário pode
   fazer, sem restrição;
5. ter uma **lista de funções** a executar (rotinas);
6. **aprender** o que o usuário faz e o que ele quer que ela faça.

O que já existe e será reaproveitado: o leitor de GGUF de
`packages/local` (ADR-0025), o Tool Runtime auditado com arquivos, shell,
processos, git, web e segredos (ADR-0020), o modo Acesso Irrestrito
(ADR-0016), Task Manager, agentes e memória em SQLite (ADRs 0012–0015).

Limites reais, que guiam as decisões abaixo. O alvo de **treino** é o
mesmo do uso: **notebook com 8 GB de RAM e 1 GB de vídeo**.

- Um notebook desses faz, com CPU e GPU integrada juntos, da ordem de
  10¹¹ a 10¹² operações por segundo úteis em treino. Numa noite de 8 h,
  isso é **~3×10¹⁵ a 3×10¹⁶ operações**.
- Treinar um modelo custa ~6 × parâmetros × tokens operações. Um modelo
  denso de 1,2 bilhão em 50 bilhões de tokens (3,6×10²⁰) levaria
  **décadas** num notebook. Copiar a receita dos grandes laboratórios em
  escala menor não funciona: é preciso outra receita.
- Um modelo de **~50 milhões** de parâmetros em **100 milhões** de tokens
  custa 3×10¹⁶: **uma noite**. Modelos desse tamanho já escrevem texto
  coerente quando o domínio é estreito e os dados são bons.
- **O cérebro humano como referência:** ~20 W (menos que o notebook), ~86
  bilhões de neurônios e ~10¹⁴–10¹⁵ sinapses. Ele não é eficiente por ser
  pequeno, mas porque:
  1. **ativa pouco**: só ~1–2% dos neurônios disparam ao mesmo tempo;
  2. **é modular**: áreas especializadas, trocadas por um roteamento;
  3. **aprende localmente**: cada sinapse muda com o que acontece perto
     dela, sem um cálculo global sobre tudo;
  4. **separa memória rápida e lenta**: o hipocampo grava o dia; o sono
     consolida no córtex, repassando o que importa, sem apagar o antigo;
  5. **não decora o mundo**: consulta o ambiente, anotações, outras
     pessoas;
  6. **cresce e poda**: começa com conexões demais e elimina as inúteis;
  7. **aprende continuamente**, pouco por dia, a vida toda.
  A Nativa copia esses princípios, não o tamanho.
- Com motor próprio, o aprendizado pode mudar os pesos no próprio
  computador, não só a memória.

## Decisão

### 1. Motor próprio: `orchestrator-engine` (Rust, sem dependências de inferência)

Um crate novo, `packages/engine`, escrito pelo projeto:

| Parte | O que faz |
| ----- | --------- |
| Formato | `.nativa` (cabeçalho + tensores, mapeado em memória com `mmap`); importa GGUF só para testes de equivalência |
| Quantização | pesos ternários (−1, 0, +1) com escala, 4 e 8 bits; ativações em f32/bf16 |
| Kernels | multiplicação matriz-vetor e matriz-matriz quantizadas com SIMD: AVX2/AVX-512 (x86), NEON (ARM, Apple M); caminho escalar de referência para testes |
| Execução | núcleo + especialistas carregados sob demanda com roteador esparso; transformer decoder (RMSNorm, RoPE, atenção com grupos de KV, SwiGLU), cache KV quantizado em 8 bits, lotes para o prompt, multithread por núcleos físicos |
| Decodificação | amostragem (temperatura, top-p, min-p), **decodificação com gramática** (JSON Schema → autômato) para que toda chamada de ferramenta saia válida |
| Embeddings | o mesmo modelo, pela média da última camada, para a busca na memória |
| Treino local | treino bloco a bloco (núcleo, especialistas, LoRA) com o resto congelado; gradientes em 8 bits, *checkpointing*, CPU + GPU de 1 GB por blocos — é o pipeline de treino da Nativa, no próprio app |
| GPU | backend por `wgpu` (Vulkan/Metal/DX12) que funciona com 1 GB de vídeo, por blocos; mesmos testes |

- Roda **dentro do processo** do Orchestrator (sem servidor HTTP, sem
  porta), com uma fila de pedidos e cancelamento imediato pelo botão Parar.
- Teto de memória: o motor sabe quanto vai usar antes de carregar
  (pesos + cache KV + ativações) e recusa ou reduz o contexto em vez de
  levar o computador a trocar para o disco.
- Corretude garantida por testes de equivalência: cada kernel contra o
  caminho escalar; o modelo inteiro contra uma implementação de referência
  em PyTorch, usada só como gabarito nos testes (logits iguais até a
  tolerância).

### 2. Modelo próprio: a Nativa como um cérebro modular

Arquitetura, tokenizador e pesos do projeto. A ideia central: **nunca
treinar o modelo inteiro de uma vez**. Ele é feito de peças pequenas, cada
uma treinável numa noite dentro de 8 GB de RAM e 1 GB de vídeo.

```text
           texto do usuário (qualquer língua)
                         │
                ┌────────▼────────┐
                │ tokenizador     │  bytes UTF-8 + BPE: cobre toda língua e escrita
                └────────┬────────┘
                ┌────────▼────────┐
                │ NÚCLEO (~40 M)  │  raciocínio curto, protocolo de ferramentas
                └────────┬────────┘
                ┌────────▼────────┐
                │ ROTEADOR        │  escolhe 1–2 especialistas por token (~5% ativo)
                └─┬────┬────┬───┬─┘
       ┌──────────┘    │    │   └──────────┐
 ┌─────▼─────┐ ┌───────▼┐ ┌─▼─────────┐ ┌──▼─────────┐
 │ idioma:pt │ │ system │ │orchestr.  │ │ usuário    │  especialistas de ~10–30 M
 │ idioma:es │ │ shell  │ │ rotinas   │ │ (pessoal)  │  cada, treinados um por vez
 └───────────┘ └────────┘ └───────────┘ └────────────┘
                         │
                memória externa (SQLite + embeddings) e web: o conhecimento
                fica fora dos pesos, como anotações
```

**Princípios (do cérebro para a Nativa):**

| Cérebro | Nativa |
| ------- | ------ |
| poucos neurônios ativos | **mistura de especialistas esparsa**: ~5% dos pesos por token; o total cresce sem o custo por token crescer |
| áreas especializadas | **especialistas por função e por língua**, carregados sob demanda; só o que está em uso ocupa memória |
| aprendizado local | **treino bloco a bloco**: só o especialista (ou o bloco) em treino recebe gradiente; o resto fica congelado e quantizado, sem guardar ativações dele |
| hipocampo + sono | **memória rápida** (trilhas e notas, na hora) + **consolidação noturna** nos pesos, com repetição de exemplos antigos para não esquecer |
| consultar em vez de decorar | **conhecimento fora dos pesos**: memória, documentação do SO e web por busca; os pesos guardam *como fazer*, não *todos os fatos* |
| crescer e podar | **crescimento progressivo**: começa pequeno, ganha camadas/especialistas quando estabiliza; especialistas pouco usados são podados |
| aprender a vida toda | **treino contínuo**: um pouco por noite, sempre validado |

**Técnicas que cabem no notebook:**

- **Pesos ternários (−1, 0, +1) com escala**, treinados assim desde o
  início (estilo BitNet b1.58): ~1,6 bit por peso, multiplicações viram
  somas — mais rápido no CPU, 10× menos memória.
- **Gradientes em 8 bits, otimizador com estado comprimido**, e
  *checkpointing* de ativações: um bloco de 30 M treina em < 1 GB.
- **GPU de 1 GB** usada por blocos (via `wgpu`): o bloco em treino vai à
  GPU, o resto fica na RAM.
- **Currículo e dados densos**: textos curtos e limpos primeiro, depois
  trilhas de ferramentas; poucos tokens muito bons valem mais que muitos
  ruins.
- **Dados sintéticos verificáveis**: o próprio Orchestrator gera tarefas
  (ex.: "liste os processos que usam mais RAM"), executa em sandbox e
  guarda só o que deu certo — dado de treino que se confere sozinho.

**Orçamento (notebook de 8 GB, ~1,5×10¹⁶ operações por noite):**

| Peça | Parâmetros | Tokens | Noites |
| ---- | ---------- | ------ | ------ |
| Núcleo (Semente) | ~40 M | ~200 M | 3–5 |
| Especialista de função | ~20 M | ~50 M | 1 |
| Especialista de língua | ~20 M | ~50–100 M | 1–2 |
| Adaptador pessoal (LoRA) | ~1–4 M | trilhas do usuário | minutos–1 h |

Um modelo de fábrica com núcleo + ~10 especialistas (~250 M no total,
~50 M ativos) fica em **< 100 MB** em ternário e roda bem em 8 GB.

**Como a Nativa nasce:**

- A **Semente** (núcleo + especialistas básicos) é treinada pelo projeto
  com esse mesmo pipeline, em notebooks comuns ou em qualquer máquina que
  o projeto tiver; vem pronta no instalador. Uma GPU alugada **acelera**,
  mas não é requisito.
- **Colmeia (opcional, desligada por padrão):** quem quiser doa noites de
  treino; cada computador treina um especialista em dados públicos e
  devolve só os pesos, que o projeto valida e junta. Dados do usuário
  nunca participam.

### 2a. Qualquer língua que o usuário quiser

- **Tokenizador com base em bytes UTF-8**: nenhum texto vira "desconhecido";
  qualquer língua e escrita é representável desde o primeiro dia.
- **Especialistas de língua**: a Semente vem com português, inglês e
  espanhol. Para outra língua, o usuário escolhe em Configurações e a
  Nativa:
  1. baixa um corpus público daquela língua (ex.: Wikipédia, textos de
     domínio público), uns poucos GB;
  2. treina um especialista de língua novo em 1–2 noites;
  3. traduz as descrições das ferramentas e as rotinas, e gera trilhas de
     ferramentas na língua nova a partir das que já existem;
  4. só ativa o especialista se passar no teste de compreensão daquela
     língua.
- **Troca a qualquer momento**: a língua da conversa é detectada por
  mensagem, e o roteador usa o especialista dela. As ferramentas,
  nomes internos e o protocolo continuam iguais em todas as línguas.
- O adaptador pessoal aprende também o jeito de escrever do usuário
  (gírias, termos do trabalho).

### 3. O agente: roteamento em dois passos, sempre com gramática

- Primeiro o modelo escolhe um **grupo** (`orchestrator`, `system`,
  `files`, `shell`, `web`, `routine`); depois só as ferramentas desse grupo
  vão ao prompt (≤ ~1.500 tokens).
- A saída é presa à gramática do JSON Schema da ferramenta: não existe
  chamada malformada.
- Argumentos validados antes de executar; em erro, nova tentativa com a
  mensagem do erro (até 2).

### 4. Ferramentas novas

**`orchestrator.*`** — os comandos do gateway IPC (ADR-0003) que o usuário
já usa: abrir/fechar projetos, criar/mover tasks, pôr agentes para
trabalhar, trocar provider, mudar a autonomia, fazer backup, buscar
atualização. Só a Nativa recebe as que mudam a autonomia, e só com o
interruptor "A Nativa administra o Orchestrator" ligado.

**`system.*`** — o computador, por sistema operacional:

| Ferramenta | Faz |
| ---------- | --- |
| `system.info` | CPU, RAM, disco, bateria, rede, SO, uptime |
| `system.processes` / `system.kill` | listar e encerrar processos |
| `system.services` | listar, iniciar, parar e habilitar serviços |
| `system.packages` | instalar, atualizar e remover programas (winget, brew, apt/dnf/pacman) |
| `system.disk` | uso, limpeza de temporários e caches |
| `system.network` | interfaces, Wi-Fi, conexões, testes |
| `system.power` | suspender, reiniciar, desligar, perfil de energia |
| `system.schedule` | tarefas agendadas do SO |
| `system.open` | abrir arquivos, pastas, URLs e programas |
| `system.clipboard` / `system.notify` | área de transferência e notificações |
| `system.elevate` | roda um comando como administrador (UAC/sudo/polkit) |

"Sem restrição" = tudo o que **a conta do usuário** pode fazer. Ações de
administrador passam pela elevação do próprio sistema; o usuário pode
guardar a senha de sudo no cofre (ADR-0020) para não ser perguntado. O
Orchestrator não contorna as proteções do SO.

### 5. Rotinas: a lista de funções

Uma **rotina** é uma lista ordenada de passos guardada no SQLite:

```yaml
nome: fim do dia
gatilho: { cron: "0 18 * * 1-5" }   # manual | cron | evento | ao iniciar
passos:
  - git.status: { todos_os_projetos: true }
  - pedir: "faça commit do que estiver pronto e me resuma"
  - orchestrator.backup: {}
  - system.power: { acao: suspender, se_ocioso_min: 30 }
```

- Passos diretos (chamada de ferramenta, sem modelo) ou pedidos em
  linguagem natural (a Nativa decide).
- Gatilhos de evento: projeto aberto, task concluída, bateria baixa,
  arquivo mudou, CI falhou, horário.
- Painel **ROTINAS**: criar, editar, ligar/desligar, rodar agora,
  histórico. A fila aceita também pedidos avulsos ("quando terminar isso,
  faça aquilo").

### 6. Aprendizado: memória **e** pesos

1. **Observação.** As ações do usuário (`CallOrigin::User` e os comandos
   da interface) viram **trilhas** locais, por projeto, apagáveis.
2. **Mineração.** Sequências repetidas (≥ 3 vezes) viram propostas de
   rotina com os valores que mudam como parâmetros.
3. **Ensinar mostrando.** Botão "Gravar": a trilha vira uma **habilidade**
   nomeada e editável.
4. **Preferências.** Correções e negações viram regras na memória L3
   ("usar pnpm, não npm").
5. **Recuperação.** A cada pedido, as habilidades e preferências mais
   próximas (embeddings do próprio motor) entram no prompt como exemplos.
6. **Sono: consolidação noturna.** Com o computador ocioso e na tomada,
   o motor treina o **especialista pessoal** (LoRA de alguns MB) nas
   trilhas aprovadas e nas correções do dia, misturadas com uma amostra de
   exemplos antigos (repetição, para não esquecer). Também pode treinar um
   especialista novo (língua, programa que o usuário passou a usar).
7. **Validação.** Um peso novo só substitui o anterior se passar no
   conjunto de testes fixo da Nativa sem piorar; senão é descartado.
   Sempre é possível voltar ao anterior ou ao modelo de fábrica.

Nada disso sai do computador sem pedido explícito do usuário.

### 7. Autonomia

A Nativa segue os modos da ADR-0016. No **Acesso Irrestrito**, nada é
perguntado. Sempre ficam, porque não são restrição: a auditoria de toda
chamada, Parar/Pausar com atalho global, e pontos de restauração
(ADR-0022) antes de rotinas que mudam o Orchestrator.

## Consequências

- O Orchestrator passa a ter IA e motor 100% próprios, offline, sem custo
  por uso, num notebook comum.
- O treino cabe no hardware alvo: nenhuma versão da Nativa exige GPU
  alugada. O preço é que a arquitetura modular, os pesos ternários e o
  treino bloco a bloco são menos testados que o treino denso clássico:
  cada técnica entra só depois de medida contra um gabarito (fases T).
- O projeto ganha duas frentes novas e grandes: manter um motor de
  inferência e treino (kernels por arquitetura de CPU e GPU) e um
  pipeline de dados verificáveis.
- A Nativa é forte no que foi treinada para fazer (operar o Orchestrator
  e o computador, conversar na língua do usuário) e consulta memória e web
  para fatos; tarefas longas de código vão aos providers grandes por
  handoff.
- Com Acesso Irrestrito e `system.*`, um erro do modelo pode apagar
  arquivos ou desligar o computador. O usuário escolhe isso
  conscientemente, e a tela de ativação diz isso claramente.
- Trilhas e adaptador pessoal ficam só no computador.

Plano de implementação em [`docs/native-ai.md`](../native-ai.md).

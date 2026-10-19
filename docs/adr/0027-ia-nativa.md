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

1. rodar em notebooks e computadores com **até 8 GB de RAM**, offline;
2. **gerenciar o Orchestrator** (projetos, tasks, agentes, providers,
   autonomia, backups, atualizações);
3. **gerenciar o computador**, podendo fazer tudo o que o usuário pode
   fazer, sem restrição;
4. ter uma **lista de funções** a executar (rotinas);
5. **aprender** o que o usuário faz e o que ele quer que ela faça.

O que já existe e será reaproveitado: o leitor de GGUF de
`packages/local` (ADR-0025), o Tool Runtime auditado com arquivos, shell,
processos, git, web e segredos (ADR-0020), o modo Acesso Irrestrito
(ADR-0016), Task Manager, agentes e memória em SQLite (ADRs 0012–0015).

Limites reais, que guiam as decisões abaixo:

- treinar do zero um modelo do tamanho dos de API custa milhões de dólares;
  um modelo **pequeno e especializado** (≤ 1,5 bilhão de parâmetros) custa
  milhares e cabe em 8 GB;
- um modelo pequeno não sabe "tudo", mas pode saber **muito bem** o que a
  Nativa faz: chamar as ferramentas do Orchestrator e do sistema, seguir
  rotinas, conversar em português;
- com motor próprio, o **aprendizado pode mudar os pesos** no próprio
  computador (adaptadores LoRA treinados no CPU), não só a memória.

## Decisão

### 1. Motor próprio: `orchestrator-engine` (Rust, sem dependências de inferência)

Um crate novo, `packages/engine`, escrito pelo projeto:

| Parte | O que faz |
| ----- | --------- |
| Formato | `.nativa` (cabeçalho + tensores, mapeado em memória com `mmap`); importa GGUF só para testes de equivalência |
| Quantização | pesos em 4 bits por grupo (estilo Q4) e 8 bits; ativações em f32/bf16 |
| Kernels | multiplicação matriz-vetor e matriz-matriz quantizadas com SIMD: AVX2/AVX-512 (x86), NEON (ARM, Apple M); caminho escalar de referência para testes |
| Execução | transformer decoder (RMSNorm, RoPE, atenção com grupos de KV, SwiGLU), cache KV quantizado em 8 bits, lotes para o prompt, multithread por núcleos físicos |
| Decodificação | amostragem (temperatura, top-p, min-p), **decodificação com gramática** (JSON Schema → autômato) para que toda chamada de ferramenta saia válida |
| Embeddings | o mesmo modelo, pela média da última camada, para a busca na memória |
| Treino local | retropropagação só nos adaptadores **LoRA** (pesos base congelados e quantizados), para o aprendizado da §5 |
| GPU (depois) | backend opcional por `wgpu` (Vulkan/Metal/DX12), mesmo formato e mesmos testes |

- Roda **dentro do processo** do Orchestrator (sem servidor HTTP, sem
  porta), com uma fila de pedidos e cancelamento imediato pelo botão Parar.
- Teto de memória: o motor sabe quanto vai usar antes de carregar
  (pesos + cache KV + ativações) e recusa ou reduz o contexto em vez de
  levar o computador a trocar para o disco.
- Corretude garantida por testes de equivalência: cada kernel contra o
  caminho escalar; o modelo inteiro contra uma implementação de referência
  em PyTorch no pipeline de treino (logits iguais até a tolerância).

### 2. Modelo próprio: família **Nativa**

Arquitetura, tokenizador e pesos do projeto, treinados pelo pipeline em
`training/` (Python + PyTorch, fora do app; roda em GPUs alugadas).

| Modelo | Parâmetros | Arquivo (4 bits) | Papel |
| ------ | ---------- | ---------------- | ----- |
| **Nativa-Mini** | ~150 M | ~100 MB | prova do motor e do pipeline; roteador de grupos e embeddings em máquinas de 4 GB |
| **Nativa-1** | ~1,2 B | ~750 MB | o modelo principal para 8 GB |
| **Nativa-2** (depois) | ~3 B | ~1,8 GB | para máquinas de 12 GB ou mais |

- **Arquitetura:** decoder-only, 24–28 camadas, atenção com grupos de KV,
  RoPE com contexto de 8 K (16 K na Nativa-2), vocabulário de ~48 mil
  tokens.
- **Tokenizador próprio:** BPE treinado com peso alto em português, código,
  comandos de shell e JSON (menos tokens por pedido = mais rápido no CPU).
- **Tokens especiais** para o protocolo da Nativa: `<|ferramenta|>`,
  `<|resultado|>`, `<|rotina|>`, `<|memória|>`; o formato de chamada é
  parte do modelo, não um template colado depois.
- **Treino em três etapas:**
  1. **pré-treino** do zero em corpus com licença aberta (texto em
     português e inglês, código, documentação de sistemas operacionais e
     ferramentas), ~40–60 bilhões de tokens para a Nativa-1;
  2. **ajuste supervisionado** em trilhas de uso de ferramentas: as
     ferramentas reais do Orchestrator e de `system.*`, geradas num
     ambiente simulado (VMs Windows, macOS, Linux) e validadas executando;
  3. **ajuste por preferência** (DPO) em pares bom/ruim das mesmas trilhas
     (chamada certa × errada, ação segura × destrutiva sem pedido).
- **Custo estimado da Nativa-1:** ~3×10²⁰ FLOPs de pré-treino, da ordem de
  alguns dias em 8 GPUs H100 alugadas (alguns milhares de dólares); a
  Nativa-Mini roda numa única GPU em horas.
- **Dados de terceiros:** só corpora com licença que permita treinar e
  redistribuir pesos. Saídas de modelos de API só onde os termos do
  fornecedor permitem usá-las para treinar outro modelo; o pipeline marca
  a origem de cada exemplo.
- **Distribuição:** os pesos `.nativa` vêm como download do próprio
  release do Orchestrator, com SHA-256 conferido, como o resto da
  atualização (ADR-0019).

O que a Nativa-1 **não** será: um substituto dos modelos de API para
tarefas longas de código. Para isso ela delega (handoff, ADR-0013) aos
providers que o usuário tiver; sem nenhum, ela faz o que der e diz isso.

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
6. **Treino no computador.** Com o computador ocioso e na tomada, o motor
   treina um **adaptador LoRA pessoal** (alguns MB) nas trilhas aprovadas
   e nas correções. O adaptador só substitui o anterior se passar no
   conjunto de testes fixo da Nativa sem piorar; senão é descartado.
   Sempre é possível voltar ao adaptador anterior ou ao modelo de fábrica.

Nada disso sai do computador sem pedido explícito do usuário.

### 7. Autonomia

A Nativa segue os modos da ADR-0016. No **Acesso Irrestrito**, nada é
perguntado. Sempre ficam, porque não são restrição: a auditoria de toda
chamada, Parar/Pausar com atalho global, e pontos de restauração
(ADR-0022) antes de rotinas que mudam o Orchestrator.

## Consequências

- O Orchestrator passa a ter IA e motor 100% próprios, offline, sem custo
  por uso, num notebook comum.
- O projeto ganha duas frentes novas e grandes: manter um motor de
  inferência (kernels por arquitetura de CPU) e um pipeline de treino com
  custo de GPU a cada versão do modelo.
- A Nativa-1 é forte no que foi treinada para fazer e fraca fora disso;
  tarefas longas de código vão aos providers grandes por handoff.
- Com Acesso Irrestrito e `system.*`, um erro do modelo pode apagar
  arquivos ou desligar o computador. O usuário escolhe isso
  conscientemente, e a tela de ativação diz isso claramente.
- Trilhas e adaptador pessoal ficam só no computador.

Plano de implementação em [`docs/native-ai.md`](../native-ai.md).

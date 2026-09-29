# Arquitetura do Orchestrator

Este documento descreve a arquitetura-alvo do Orchestrator, o que já foi
implementado e onde cada responsabilidade vive no repositório. Toda mudança
estrutural é registrada antes como ADR em [`docs/adr/`](./docs/adr).

## 1. Princípios (regra de ouro)

1. **IA é substituível.** Nenhum módulo do núcleo depende de OpenAI ou Anthropic.
2. **Projeto é permanente.** Memória, histórico e decisões pertencem ao projeto.
3. **Agentes são descartáveis.** Conhecimento vive no projeto, não no agente.
4. **Providers são intercambiáveis.** Todos implementam a mesma interface `AIProvider`.
5. **Workspace local é a fonte primária.** GitHub é remoto.
6. **O Orchestrator controla a execução.** Providers pedem `tool_call`; o runtime executa.
7. **O usuário decide o nível de autonomia** (Assistido, Autônomo, Acesso Irrestrito).
8. **Observabilidade não é restrição.** Logs, histórico e auditoria são sempre registrados,
   inclusive em Acesso Irrestrito.

## 2. Visão em camadas

```text
┌──────────────────────────────────────────────────────────────────────┐
│ Desktop UI (React + TypeScript)                  apps/desktop/src    │
│   painéis: PROJECT · AI PROVIDERS · TASKS · AGENTS · TERMINAL · GIT   │
│            MEMORY · HISTORY                                           │
└───────────────▲──────────────────────────────┬───────────────────────┘
                │ eventos (runtime://stream,    │ invoke (runtime_invoke,
                │          runtime://audit)     │  terminal_input, session_*, …)
┌───────────────┴──────────────────────────────▼───────────────────────┐
│ Tauri (ponte IPC, sem lógica de domínio)   apps/desktop/src-tauri    │
└───────────────▲──────────────────────────────┬───────────────────────┘
                │ EventSink                     │ ToolRuntime::invoke(ToolCall),
                │                               │ SessionManager (providers)
┌───────────────┴──────────────────────────────▼───────────────────────┐
│ Rust Runtime                                                          │
│   Tool Runtime (packages/runtime)                                     │
│     filesystem · shell · terminal (PTY) · process manager             │
│     git (Fase 2) · github (Fase 10) · package managers · runtimes     │
├───────────────────────────────────────────────────────────────────────┤
│ Orchestrator Core                                                     │
│   core (contratos)  · provider layer (Fase 3: AIProvider, registro,   │
│   sessões) · orchestrator engine · agent manager · task manager ·     │
│   context builder · memory/history                                    │
├───────────────────────────────────────────────────────────────────────┤
│ Local Database (SQLite, Fase 6)                                       │
└───────────────────────────────────────────────────────────────────────┘
```

O fluxo de controle é sempre:

```text
React → Tauri → Rust Runtime → Orchestrator Core
```

e o fluxo de trabalho de uma IA é:

```text
AIProvider ──tool_call──▶ Orchestrator ──▶ Tool Runtime ──▶ SO / Workspace / Git ──▶ GitHub
```

**Nenhum provider acessa o sistema operacional diretamente.** Providers só
produzem `ToolCall`s; quem executa é o `ToolRuntime`, que registra cada
execução como evento de auditoria.

## 3. Módulos e localização

Todo o núcleo roda em Rust, no processo do Tauri (ver
[ADR-0001](./docs/adr/0001-monorepo-hibrido-nucleo-em-rust.md)). O React é
apenas apresentação.

| Módulo | Local | Linguagem | Fase | Estado |
| ------ | ----- | --------- | ---- | ------ |
| Desktop UI | `apps/desktop/src` | React/TS | 1–3 | ✅ shell, PROJECT, AI PROVIDERS + sessões, GIT, terminal, processos, HISTORY |
| Ponte IPC | `apps/desktop/src-tauri` | Rust | 1–3 | ✅ |
| Core (contratos) | `packages/core` | Rust | 1–3 | ✅ `ToolCall`, `ToolResult`, `AuditEvent`, `StreamEvent`, `EventSink`, `ProjectProfile`, `SessionInfo`, `SessionEvent`, `TokenUsage` |
| Tool Runtime | `packages/runtime` | Rust | 1–2 | ✅ filesystem, shell, terminal, process, project, git, package, runtime |
| Git local | `packages/git` | Rust | 2 | ✅ `git` do sistema (ADR-0007) |
| Project Discovery / Profile | `packages/core` (tipos) + `packages/runtime` (detecção) | Rust | 2 | ✅ (ADR-0008) |
| AIProvider / Registry / Sessions | `packages/providers` | Rust | 3 | ✅ trait, registro, sessões, provider `echo` de desenvolvimento (ADR-0009) |
| OpenAI / Codex | `packages/providers/openai` | Rust | 4 | planejado |
| Claude Code | `packages/providers/claude` | Rust | 5 | planejado |
| SQLite, Memory, History, Decisions | `packages/memory` | Rust | 6 | planejado |
| Context Builder, Handoff | `packages/orchestrator` | Rust | 7 | planejado |
| Task Manager, Agent Manager, Subagents, File Locks | `packages/orchestrator` + `packages/agents` | Rust | 8 | planejado |
| Autonomia (Assistido/Autônomo/Irrestrito) | `packages/orchestrator` | Rust | 9 | planejado |
| GitHub | `packages/git` | Rust | 10 | planejado |
| Otimização de tokens, cache, compactação, scheduling | `packages/orchestrator` | Rust | 11 | planejado |

A única alteração à estrutura original é a adição de `packages/runtime`
([ADR-0002](./docs/adr/0002-pacote-runtime-para-o-tool-runtime.md)).

## 4. Tool Runtime (Fases 1–2)

Ponto único de execução de operações de sistema. Entrada: `ToolCall`
(`{ id, tool, args, origin }`). Saída: `ToolResult`
(`{ callId, tool, ok, output, error, startedAt, finishedAt, durationMs }`).

```text
ToolRuntime::invoke(call)
  ├── valida e desserializa args (camelCase, campos desconhecidos rejeitados)
  ├── despacha para o módulo (filesystem | shell | terminal | process |
  │                          project | git | package | runtime)
  ├── emite AuditEvent TOOL_CALLED (sempre, com sucesso ou erro)
  ├── emite eventos de domínio (FILE_CHANGED, COMMAND_EXECUTED, …)
  └── retorna ToolResult
```

Catálogo implementado (detalhes em [`docs/tool-runtime.md`](./docs/tool-runtime.md)):

| Grupo | Operações |
| ----- | --------- |
| filesystem | `list`, `read`, `write`, `move`, `delete` |
| shell | `execute`, `list` |
| terminal | `create`, `write`, `read`, `close`, `list` |
| process | `start`, `stop`, `list`, `read` |
| project | `discover`, `profile`, `open` |
| git | `status`, `diff`, `log`, `branch`, `checkout`, `add`, `commit`, `pull`, `push`, `stash`, `reset` |
| package | `install`, `run` |
| runtime | `node`, `python`, `docker` |

Cada ferramenta é marcada no catálogo como consulta (`readOnly`) ou ação; a
marca vai no evento `TOOL_CALLED` (ADR-0008). Extensões ao catálogo da seção 9:
`shell.list`, `terminal.list`, `process.read`
([ADR-0004](./docs/adr/0004-operacoes-auxiliares-do-tool-runtime.md)) e
`project.*` ([ADR-0008](./docs/adr/0008-projeto-deteccao-e-diretorio-base.md)).

Operações planejadas: `github.*` (Fase 10).

### 4.1 Terminal real

- PTY nativo via `portable-pty` (ConPTY no Windows, pty Unix no Linux/macOS).
- Shells detectados: PowerShell 7 (`pwsh`), Windows PowerShell, CMD, WSL,
  Git Bash (Windows); bash, zsh, fish, sh, pwsh (Unix).
- A saída é decodificada em UTF-8 de forma incremental, publicada em tempo real
  (`StreamEvent::TerminalOutput`) e mantida num buffer circular endereçado por
  offset, para que uma IA leia apenas o que é novo (`terminal.read { since }`).

### 4.2 Shell e processos

- `shell.execute` roda um comando não interativo e devolve stdout, stderr,
  exit code, duração e `timedOut`. Exit code ≠ 0 **não** é erro da ferramenta:
  é informação para quem chamou.
- `process.start` mantém processos de longa duração; saída em tempo real e em
  buffer; `process.stop` encerra a **árvore** de processos (grupo de processos
  no Unix, `taskkill /T /F` no Windows).
- Ao fechar o app (inclusive por SIGTERM/SIGINT/SIGHUP no Unix), todos os
  terminais e processos gerenciados são encerrados.

### 4.3 Projeto (Fase 2)

- `project.open` define o projeto ativo: o **diretório base do runtime** passa
  a ser a raiz do projeto (caminhos relativos e `cwd` padrão de shell,
  terminais e processos) e é emitido `PROJECT_OPENED`.
- `project.profile` monta o PROJECT PROFILE: nome, caminho, Git (root, branch,
  upstream, remotes, contagem de alterações), linguagens, frameworks, package
  managers, runtimes (com versão pedida), Docker (Dockerfiles, compose,
  imagens), bancos (Prisma, compose, dependências), ferramentas, arquivos
  importantes, scripts e as **evidências** de cada conclusão. `.env` nunca é
  lido.
- `project.discover` varre raízes (padrão: pasta do usuário e pastas comuns
  como `C:\Projetos`) em largura, sem entrar em `node_modules`, `target`,
  pastas ocultas etc., e sem descer dentro de um projeto encontrado.

### 4.4 Git (Fase 2)

`packages/git` executa o `git` do sistema (credenciais, hooks e configuração
do usuário valem igual ao terminal) e lê formatos estáveis para máquinas
([ADR-0007](./docs/adr/0007-git-via-cli-do-sistema.md)). Sem prompts
interativos (`GIT_TERMINAL_PROMPT=0`); consultas não disputam o `index.lock`
(`GIT_OPTIONAL_LOCKS=0`). `git.commit` emite `GIT_COMMIT`; `git.push`, `GIT_PUSH`.
Falhas do Git viram `COMMAND_FAILED` com a saída do comando.

### 4.5 Autonomia e o runtime

Desde a Fase 3 há dois chamadores: o usuário, pela UI, e as sessões de
provider, por `TurnContext::call_tool` → `ToolRuntime::invoke` (origem
`agent`). Até a Fase 9 as chamadas de IA são executadas sem gate, com
auditoria completa. O caminho `call_tool` → `invoke` é o ponto onde, na
Fase 9, entra o gate de autonomia:

| Modo | Comportamento no gate |
| ---- | --------------------- |
| Assistido | pede autorização ao usuário quando a política exigir |
| Autônomo | aplica as políticas configuradas pelo usuário |
| Acesso Irrestrito | **nenhuma** política operacional; sem confirmações ocultas, sem lista de comandos proibidos |

Em todos os modos a auditoria continua ativa. O runtime **não** contém hoje
nenhuma lista de comandos proibidos nem bloqueio silencioso, e não deve passar
a conter fora do gate explícito da Fase 9.

## 5. Contratos e eventos (packages/core)

- `ToolCall`, `ToolResult`, `ToolError`, `ToolErrorKind`, `CallOrigin`
  (`user` | `agent { agentId, sessionId?, provider? }` | `system`), `ToolSpec`.
- `SessionInfo`, `SessionEvent`, `SessionLogEntry`, `TokenUsage`,
  `SessionStatus`, `TurnStatus`, ids `SessionId`/`TurnId`/`ProviderId` —
  contratos das sessões de provider (Fase 3, ADR-0009).
- `ProjectProfile`, `ProjectCandidate`, `GitSummary` — contratos do projeto
  (Fase 2).
- `AuditEvent { id, at, kind, origin, summary, data }` — histórico durável,
  independente de provider. `EventKind` contém todos os eventos da seção 22 do
  documento mestre, mais `PROCESS_EXITED` e `TERMINAL_EXITED`
  ([ADR-0005](./docs/adr/0005-observabilidade-antes-do-sqlite.md)) e
  `SESSION_STARTED`, `SESSION_RESUMED`, `SESSION_CLOSED`, `TURN_COMPLETED`
  ([ADR-0009](./docs/adr/0009-camada-de-providers-e-sessoes.md)).
- `StreamEvent` — eventos de alta frequência e não duráveis (saída de
  terminal/processo, término, eventos de sessão de provider).
- `EventSink` — trait que desacopla o runtime de quem consome eventos (Tauri
  hoje; SQLite na Fase 6).

IDs são UUID v7 (ordenáveis por tempo), prontos para chave primária no SQLite.

## 6. Camada IPC

Detalhada em [`docs/ipc.md`](./docs/ipc.md) e
[ADR-0003](./docs/adr/0003-gateway-ipc-unico.md).

| Comando Tauri | Finalidade |
| ------------- | ---------- |
| `runtime_invoke(tool, args)` | gateway único para qualquer ferramenta; auditado |
| `runtime_tools()` | catálogo de ferramentas |
| `terminal_input(id, data)` | canal de digitação humana no terminal (streaming, não auditado por tecla) |
| `terminal_resize(id, cols, rows)` | redimensionamento do PTY |
| `history_recent(limit)` | eventos de auditoria recentes |
| `app_info()` | versão, SO, diretórios, projeto aberto |
| `pick_folder()` | seletor nativo de pasta (só UI; a pasta escolhida é aberta via `project.open`) |
| `providers_list`, `provider_inspect`, `provider_select` | registro de providers e provider ativo (Fase 3) |
| `sessions_list`, `session_start`, `session_get`, `session_send`, `session_cancel`, `session_close`, `session_resume`, `session_spawn` | sessões de provider (Fase 3) |

| Evento Tauri | Payload |
| ------------ | ------- |
| `runtime://stream` | `StreamEvent` (saída/término de terminal e processo, eventos de sessão) |
| `runtime://audit` | `AuditEvent` |

## 7. Modelo de dados planejado (Fase 6)

SQLite com as entidades: `projects`, `tasks`, `task_dependencies`, `agents`,
`agent_sessions`, `messages`, `tool_calls`, `memory`, `decisions`,
`artifacts`, `file_locks`, `git_operations`, `audit_events`. Os tipos de
`packages/core` já usam IDs e timestamps compatíveis com essas tabelas. Até lá,
a auditoria é gravada em JSONL (`<app-data>/audit.jsonl`).

## 8. Memória, contexto e handoff (Fases 6–7)

- **L1 Working Memory**: task atual, arquivos, últimos comandos/erros, objetivo.
- **L2 Project Memory**: arquitetura, stack, convenções, decisões, regras.
- **L3 Historical Memory**: sessões, mensagens, tool calls, commits, erros.
- **Context Builder**: monta `TASK + L1 + L2 relevante + arquivos relevantes +
  erros recentes + histórico relevante + estado do Git + handoff`. Nunca envia
  L3 inteiro, o repositório inteiro ou todas as mensagens.
- **HandoffPacket**: `goal, status, completed, remaining, files, commands,
  errors, decisions, tests, nextAction` — permite a outra IA continuar sem
  receber a conversa anterior.

Os buffers com offset do runtime (`terminal.read`/`process.read { since }`)
já existem para que o Context Builder envie apenas saída nova.

## 9. Providers (Fases 3–5)

```typescript
interface AIProvider {
  start(); resume(); execute(); stream(); cancel();
  spawnAgent(); inspect(); capabilities();
}
```

Implementado na Fase 3 como trait Rust em `packages/providers`
([ADR-0009](./docs/adr/0009-camada-de-providers-e-sessoes.md), referência em
[`docs/providers.md`](./docs/providers.md)):

- **`AIProvider`** — os oito métodos acima (+ `descriptor`), com padrões
  sensatos para `stream`, `resume`, `cancel` e `spawn_agent`.
- **`ProviderRegistry`** — providers registrados, provider ativo,
  `PROVIDER_SWITCHED`.
- **`SessionManager`** — a sessão pertence ao Orchestrator: um turno por vez,
  transcript numerado, uso de tokens/custo, cancelamento com prazo,
  encerrar/retomar, subagentes (mesmo provider ou outro), eventos ao vivo e
  histórico.
- **`TurnContext::call_tool`** — única saída do provider para o sistema; o
  Orchestrator executa pelo Tool Runtime com a sessão como origem.
- **`echo`** — provider de desenvolvimento sem IA, para testes e builds de
  desenvolvimento.

Adapters independentes (`openai` na Fase 4, `claude` na Fase 5) entram como
crates em `packages/providers/*`. Nenhuma chamada específica de fornecedor
fora do adapter. Novos providers (Gemini, modelos locais) entram da mesma
forma, sem alterar o núcleo.

## 10. Plataformas

| SO | Terminal | Shell padrão | Encerramento de árvore |
| -- | -------- | ------------ | ---------------------- |
| Windows | ConPTY | pwsh → powershell → cmd | `taskkill /T /F` |
| Linux | pty | `$SHELL` → bash → sh | `kill(-pgid)` |
| macOS | pty | `$SHELL` → zsh → bash → sh | `kill(-pgid)` |

CI em Linux, Windows e macOS: ver
[ADR-0006](./docs/adr/0006-ci-multiplataforma.md).

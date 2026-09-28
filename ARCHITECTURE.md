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
                │          runtime://audit)     │  terminal_input, …)
┌───────────────┴──────────────────────────────▼───────────────────────┐
│ Tauri (ponte IPC, sem lógica de domínio)   apps/desktop/src-tauri    │
└───────────────▲──────────────────────────────┬───────────────────────┘
                │ EventSink                     │ ToolRuntime::invoke(ToolCall)
┌───────────────┴──────────────────────────────▼───────────────────────┐
│ Rust Runtime                                                          │
│   Tool Runtime (packages/runtime)                                     │
│     filesystem · shell · terminal (PTY) · process manager             │
│     git (Fase 2) · github (Fase 10) · package managers · runtimes     │
├───────────────────────────────────────────────────────────────────────┤
│ Orchestrator Core                                                     │
│   core (contratos)  · orchestrator engine · agent manager ·           │
│   task manager · context builder · memory/history · provider layer    │
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
| Desktop UI | `apps/desktop/src` | React/TS | 1 | ✅ shell + painéis da Fase 1 |
| Ponte IPC | `apps/desktop/src-tauri` | Rust | 1 | ✅ |
| Core (contratos) | `packages/core` | Rust | 1 | ✅ `ToolCall`, `ToolResult`, `AuditEvent`, `StreamEvent`, `EventSink` |
| Tool Runtime | `packages/runtime` | Rust | 1 | ✅ filesystem, shell, terminal, process |
| Git | `packages/git` | Rust | 2 | planejado |
| Project Discovery / Profile | `packages/core` + `packages/git` | Rust | 2 | planejado |
| AIProvider / Registry / Sessions | `packages/providers` | Rust | 3 | planejado |
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

## 4. Tool Runtime (Fase 1)

Ponto único de execução de operações de sistema. Entrada: `ToolCall`
(`{ id, tool, args, origin }`). Saída: `ToolResult`
(`{ callId, tool, ok, output, error, startedAt, finishedAt, durationMs }`).

```text
ToolRuntime::invoke(call)
  ├── valida e desserializa args (camelCase, campos desconhecidos rejeitados)
  ├── despacha para o módulo (filesystem | shell | terminal | process)
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

`shell.list`, `terminal.list` e `process.read` são extensões registradas em
[ADR-0004](./docs/adr/0004-operacoes-auxiliares-do-tool-runtime.md).

Operações planejadas: `git.*` (Fase 2), `github.*` (Fase 10), `package.*` e
`runtime.*` (a partir da Fase 2, via detecção de projeto).

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

### 4.3 Autonomia e o runtime

Na Fase 1 não há provider conectado: o único chamador é o usuário, pela UI.
O `ToolRuntime::invoke` é o ponto onde, na Fase 9, entra o gate de autonomia:

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
  (`user` | `agent` | `system`), `ToolSpec`.
- `AuditEvent { id, at, kind, origin, summary, data }` — histórico durável,
  independente de provider. `EventKind` contém todos os eventos da seção 22 do
  documento mestre, mais `PROCESS_EXITED` e `TERMINAL_EXITED`
  ([ADR-0005](./docs/adr/0005-observabilidade-antes-do-sqlite.md)).
- `StreamEvent` — eventos de alta frequência e não duráveis (saída de
  terminal/processo, término).
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
| `app_info()` | versão, SO, diretórios |

| Evento Tauri | Payload |
| ------------ | ------- |
| `runtime://stream` | `StreamEvent` (saída/término de terminal e processo) |
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

Implementado como trait Rust em `packages/providers`, com adapters
independentes (`openai`, `claude`). Nenhuma chamada específica de fornecedor
fora do adapter. Novos providers (Gemini, modelos locais) entram como novos
crates sem alterar o núcleo.

## 10. Plataformas

| SO | Terminal | Shell padrão | Encerramento de árvore |
| -- | -------- | ------------ | ---------------------- |
| Windows | ConPTY | pwsh → powershell → cmd | `taskkill /T /F` |
| Linux | pty | `$SHELL` → bash → sh | `kill(-pgid)` |
| macOS | pty | `$SHELL` → zsh → bash → sh | `kill(-pgid)` |

CI em Linux, Windows e macOS: ver
[ADR-0006](./docs/adr/0006-ci-multiplataforma.md).

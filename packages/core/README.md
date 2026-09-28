# packages/core — `orchestrator-core`

Contratos do domínio, sem I/O. Tudo o que é trocado entre UI, runtime,
orchestrator e providers passa por estes tipos.

| Módulo | Conteúdo |
| ------ | -------- |
| `ids` | `ToolCallId`, `EventId`, `TerminalId`, `ProcessId` (UUID v7) |
| `tool` | `ToolCall`, `ToolResult`, `ToolError`, `ToolErrorKind`, `CallOrigin`, `ToolSpec` |
| `event` | `AuditEvent`, `EventKind`, `StreamEvent`, `EventSink` |

Serialização JSON em `camelCase`; `EventKind` em `SCREAMING_SNAKE_CASE`.

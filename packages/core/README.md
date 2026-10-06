# packages/core — `orchestrator-core`

Contratos do domínio, sem I/O. Tudo o que é trocado entre UI, runtime,
orchestrator e providers passa por estes tipos.

| Módulo | Conteúdo |
| ------ | -------- |
| `ids` | `ToolCallId`, `EventId`, `TerminalId`, `ProcessId`, `SessionId`, `TurnId` (UUID v7); `ProviderId` (texto estável) |
| `tool` | `ToolCall`, `ToolResult`, `ToolError`, `ToolErrorKind`, `CallOrigin`, `ToolSpec` |
| `event` | `AuditEvent`, `EventKind`, `StreamEvent`, `EventSink` |
| `project` | `ProjectProfile`, `ProjectCandidate`, `GitSummary`, `DockerInfo`, `RuntimeRequirement` |
| `session` | `SessionInfo`, `SessionEvent`, `SessionLogEntry`, `SessionStatus`, `TurnStatus`, `TokenUsage`, `NoticeLevel` (Fase 3, ADR-0009) |

Serialização JSON em `camelCase`; `EventKind` em `SCREAMING_SNAKE_CASE`.

# packages/memory

**Project Memory, History e banco local (SQLite).**

| Responsabilidade | Fase |
| ---------------- | ---- |
| SQLite: projects, tasks, task_dependencies, agents, agent_sessions, messages, tool_calls, memory, decisions, artifacts, file_locks, git_operations, audit_events | 6 |
| L1 Working Memory, L2 Project Memory, L3 Historical Memory | 6 |
| History (eventos independentes de provider) — substitui o `audit.jsonl` da Fase 1 | 6 |
| Decisions | 6 |
| Recuperação seletiva de contexto (nunca enviar L3 inteiro) | 6–7 |

A memória pertence ao projeto, não ao provider.

Ainda não implementado — vira crate Rust na Fase 6 (ver ADR-0001).

# packages/orchestrator

**Orchestrator Engine** — núcleo que coordena projetos, tasks, agentes e
providers.

| Responsabilidade | Fase |
| ---------------- | ---- |
| Context Builder (monta o contexto mínimo e relevante por execução) | 7 |
| HandoffPacket (goal, status, completed, remaining, files, commands, errors, decisions, tests, nextAction) | 7 |
| Task Manager (TODO, IN_PROGRESS, BLOCKED, REVIEW, DONE, CANCELLED; dependências) | 8 |
| Gate de autonomia: Assistido, Autônomo, Acesso Irrestrito | 9 |
| Controles globais: Pause, Cancel, Stop All Agents | 9 |
| Otimização de tokens, cache, compactação, agent scheduling | 11 |

Depende de: `orchestrator-core`, `orchestrator-runtime`, `memory`, `providers`.

Ainda não implementado — vira crate Rust na Fase 7 (ver ADR-0001).

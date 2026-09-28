# packages/agents

**Agent Manager** — ciclo de vida de agentes temporários.

| Responsabilidade | Fase |
| ---------------- | ---- |
| Modelo `Agent { id, provider, task, session, status, tools, context, result }` | 8 |
| Subagentes e delegação de subtarefas | 8 |
| File Lock Manager (dois agentes não alteram os mesmos arquivos sem coordenação) | 8 |
| Geração de handoff ao final de uma execução | 8 |

Agentes são descartáveis; o conhecimento fica no projeto (`packages/memory`).

Ainda não implementado — vira crate Rust na Fase 8 (ver ADR-0001).

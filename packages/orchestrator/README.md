# packages/orchestrator

**Orchestrator Engine** — crate `orchestrator-engine` (Fase 7,
[ADR-0013](../../docs/adr/0013-context-builder-e-handoff.md); Task Manager
na Fase 8a, [ADR-0014](../../docs/adr/0014-task-manager.md)). Referências:
[`docs/context.md`](../../docs/context.md) e
[`docs/tasks.md`](../../docs/tasks.md).

| Módulo | Conteúdo |
| ------ | -------- |
| `builder.rs` | `ContextBuilder`: seções `TASK`, `WORKING MEMORY`, `PROJECT MEMORY`, `RELEVANT FILES`, `RECENT ERRORS`, `RELEVANT HISTORY`, `GIT STATE`, `HANDOFF`, escolhidas por regras e busca FTS, com orçamento de tokens e corte por prioridade; `ContextPack` (seções, tokens, o que ficou de fora, texto); implementa o `ContextSource` dos providers |
| `settings.rs` | `ContextSettings` (`autoAttach`, `budgetTokens`) em `<app-data>/context.json` |
| `tools.rs` | `EngineTools`: envolve o executor do app e oferece às IAs `memory.working`, `memory.search`, `memory.list`, `memory.save`, `decision.list` e `decision.save`, auditadas como as outras ferramentas |
| `packet.rs` | `HandoffPacket`: limites, fatos do histórico, pedido e leitura tolerante da narrativa da IA, mistura, texto para a próxima IA e a primeira mensagem |
| `handoff.rs` | `HandoffService` (`prepare`, `create`, `start`, `list`, `get`) e `ContextBuilder::preview` |
| `task.rs` | `TaskService`: transições válidas, dependências sem ciclo, subtasks, contexto e sessão a partir da task (`open_session`, que o Agent Manager usa para conduzir os turnos) |
| `persistence.rs` | `StoreSessions`: o `SessionStore` dos providers sobre o banco local (movido do app) |
| `text.rs` | estimativa de tokens (~4 caracteres por token), cortes, caminhos relativos e datas |

Depende de `orchestrator-core`, `orchestrator-providers`,
`orchestrator-memory` e `orchestrator-git`. Recebe o executor de ferramentas
do app (`ToolExecutor`), então não depende do runtime. O app
(`src-tauri`) só liga as peças: instala o `ContextBuilder` no
`SessionManager`, envolve o `RuntimeTools` com o `EngineTools` e expõe os
comandos de contexto e handoff.

Regras:

- Montar o contexto não chama IA: regras e a busca do banco.
- Arquivos entram pelo caminho, nunca pelo conteúdo; nunca vão o histórico
  inteiro nem o repositório.
- A IA grava memória com origem `agent`, não altera o que o usuário ou o
  detector escreveram e não apaga nada.
- A sessão que assume um handoff recebe o pacote, nunca a conversa
  anterior.
- As regras das tasks moram aqui: a UI só oferece o que este crate
  aceitaria (`TaskView.can`), e uma task só inicia quando o que ela espera
  está concluído.

O Agent Manager e as travas de arquivo ficam no
[`packages/agents`](../agents/README.md), que depende deste crate
(ADR-0015).

| Próximas responsabilidades | Fase |
| -------------------------- | ---- |
| Gate de autonomia: Assistido, Autônomo, Acesso Irrestrito; Pause | 9 |
| Otimização de tokens, cache, compactação, agent scheduling | 11 |

Testes: `cargo test -p orchestrator-engine`. Os unitários cobrem o corte
por prioridade, caminhos citados, a configuração, os schemas das
ferramentas, os limites e a leitura do pacote, e o `StoreSessions`. Os de
integração (`tests/engine.rs`, com o Tool Runtime real, um repositório Git
temporário e o provider `echo`) cobrem:

- contexto relevante e dentro do orçamento, só com caminhos;
- o contexto no primeiro turno e as ferramentas de memória pelas sessões;
- um handoff completo, sem vazar a conversa anterior, aceito uma vez;
- o rascunho só com os fatos quando a IA não responde.

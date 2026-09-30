# packages/memory

**Banco local, memória do projeto e histórico** — crate
`orchestrator-memory` (Fase 6,
[ADR-0012](../../docs/adr/0012-sqlite-memoria-e-historico.md); handoffs na
Fase 7, [ADR-0013](../../docs/adr/0013-context-builder-e-handoff.md)).
Referência:
[`docs/memory.md`](../../docs/memory.md).

Um banco SQLite por instalação (`<app-data>/orchestrator.db`), embutido
(`rusqlite` com `bundled`), em WAL, com migrações por `PRAGMA user_version`.

| Módulo | Conteúdo |
| ------ | -------- |
| `db.rs` | abrir, pragmas, migrações; esquema v1 (tabelas, view `tool_calls`, índice FTS5) e v2 (`handoffs`) |
| `model.rs` | tipos públicos: `Project`, `HistoryQuery`/`HistoryPage`, `MemoryEntry`, `Decision`, `StoredSession`, `WorkingMemory`, `MemoryOverview`, `SearchHit` |
| `store.rs` | `MemoryStore`: gravar `AuditEvent` marcando o projeto (e gerar `PROJECT_CREATED` e a stack), importar o `audit.jsonl`, histórico paginado, projetos recentes |
| `notes.rs` | memória L2 (inclui a entrada "Stack detectada") e decisões, com `MEMORY_SAVED`, `MEMORY_REMOVED` e `DECISION_SAVED` |
| `working.rs` | L1 derivada do histórico: sessões, arquivos, comandos e erros; `overview` com as contagens |
| `search.rs` | índice e busca L3 (FTS5 sem acento, por prefixo, com trecho marcado); `search_related`, em que qualquer palavra significativa conta, para o Context Builder |
| `handoffs.rs` | handoffs (gravar, listar, aceitar uma vez) e `session_facts`, o que o histórico diz que uma sessão fez |
| `tasks.rs` | tasks e suas dependências (Fase 8a); as linhas de dependência são a verdade, e a task é lida com o que elas dizem |
| `sessions.rs` | sessões de provider e transcripts (o app liga ao `SessionStore` dos providers) |
| `deliberations.rs` | deliberações do Conselho e cache entre execuções (o app liga ao `DeliberationStore` do roteador) |

Depende só de `orchestrator-core`. Providers e roteador definem traits
(`SessionStore`, `DeliberationStore`), implementados com este crate pelo
`orchestrator-engine` (`StoreSessions`) e pelo app
(`src-tauri/src/persistence.rs`, `StoreDeliberations`).

A memória pertence ao projeto, não ao provider: tudo o que é de um projeto
tem `project_id`, e o banco nunca escreve dentro da pasta do projeto.

Testes: `cargo test -p orchestrator-memory`. Os unitários cobrem migração,
recusa de esquema mais novo, FTS e consultas. Os de integração
(`tests/store.rs`) cobrem:

- registro de projetos, stack e recentes;
- marcação e paginação do histórico;
- importação do JSONL;
- L1, L2, decisões e busca;
- sessões e deliberações reabertas;
- um arquivo inutilizável;
- handoffs, fatos de uma sessão e a busca por relevância (Fase 7);
- tasks com dependências, ordem do painel e busca (Fase 8a).

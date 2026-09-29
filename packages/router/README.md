# packages/router

**Roteador de modelos e Conselho de IAs** — crate `orchestrator-router`
(Fase 5, [ADR-0011](../../docs/adr/0011-roteador-de-modelos-e-conselho.md)).
Referência: [`docs/router.md`](../../docs/router.md).

Escolhe o modelo de cada tarefa entre todos os providers registrados:

- o **roteador** dá nota por regras, sem gastar tokens, e explica cada nota;
- o **Conselho** (1 a 5 modelos) delibera sobre os melhores candidatos por
  `AIProvider::complete`, sem sessão e sem ferramentas, e vota.

| Módulo | Conteúdo |
| ------ | -------- |
| `activity.rs` | atividades, perfis (etiquetas afins, ferramentas, preferência), detecção por palavras-chave, normalização sem acentos |
| `catalog.rs` | modelos de todos os providers registrados e disponibilidade (`inspect` em paralelo, válida por 5 min) |
| `score.rs` | `rank`: filtros com motivo, critérios, pesos por preferência, ranking com motivos |
| `council.rs` | pergunta aos membros, leitura tolerante do JSON, contagem de Borda ponderada, `Deliberation` |
| `cache.rs` | deliberações reaproveitadas enquanto nada relevante mudou |
| `settings.rs` | `CouncilSettings` (modo, membros, opções), validação, `council.json` |
| `service.rs` | `RouterService`: recomendar, deliberar (membros em paralelo, prazo, abstenções), cache, histórico, `COUNCIL_*`/`ROUTE_DECIDED`, abrir a sessão escolhida (modo Full sozinho) |
| `store.rs` | trait `DeliberationStore`: deliberações e cache guardados entre execuções (Fase 6; o app usa o banco local) |

Depende só de `orchestrator-core` e `orchestrator-providers`: funciona com
qualquer `AIProvider`, não só com as conexões de API.

Testes: `cargo test -p orchestrator-router`. Os de integração
(`tests/council.rs`) usam providers roteirizados e o `SessionManager` real,
e cobrem:

- os modos Desligado, Sugerir e Full;
- votos e abstenções, e o prazo de um membro lento;
- cache e requisitos;
- configuração e histórico;
- deliberações e cache que sobrevivem a um reinício (`DeliberationStore`).

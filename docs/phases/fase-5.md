# Fase 5 — Roteador de modelos e Conselho de IAs

## STATUS

✅ **Concluída.** O Orchestrator escolhe o modelo de cada tarefa entre
todas as APIs cadastradas:

- **roteador:** nota de 0 a 100 para cada modelo, por regras, **sem gastar
  tokens**, com o motivo de cada nota e de cada exclusão;
- **Conselho de 1 a 5 IAs:** com um membro, ele é o "gerenciador". Os
  membros votam em paralelo sobre os melhores candidatos, com motivo e
  confiança, sem sessão e sem ferramentas;
- **modos:** *Desligado* (só o roteador), *Sugerir* (o usuário aprova ou
  escolhe outro) e *Full* (o Conselho abre a sessão e envia a tarefa
  sozinho);
- **custo e cache:** o custo de cada deliberação fica visível, e a mesma
  pergunta não gasta tokens de novo;
- **histórico:** `COUNCIL_CONFIGURED`, `COUNCIL_DELIBERATED` e
  `ROUTE_DECIDED`, com a origem `council` quando o Conselho agiu sozinho.

![Deliberação do Conselho: decisão, votos e ranking](../assets/fase-5-conselho.png)

![Configuração do Conselho](../assets/fase-5-configuracao.png)

## Decisões registradas antes do código

- [ADR-0011](../adr/0011-roteador-de-modelos-e-conselho.md):
  - crate novo `packages/router`, que depende só de `core` e `providers`
    (`packages/orchestrator` continua reservado à Fase 7);
  - `AIProvider::complete`: resposta avulsa sem sessão, histórico nem
    ferramentas, com a capacidade `completion`;
  - atividades com perfis e detecção por palavras-chave;
  - pontuação com filtros, critérios e pesos por preferência;
  - Conselho: lista curta com ids `c1…cK`, resposta JSON, leitura tolerante,
    Borda ponderada pela confiança, abstenções e queda para o roteador;
  - cache por pergunta, candidatos e membros;
  - modos Desligado, Sugerir e Full. O Full só aplica decisões do Conselho
    e não dispensa o gate de autonomia da Fase 9;
  - `council.json` e eventos `COUNCIL_CONFIGURED`, `COUNCIL_DELIBERATED`,
    `ROUTE_DECIDED`;
  - origem `council` em `CallOrigin`;
  - comandos Tauri;
  - privacidade: os membros não recebem arquivos, chaves nem ferramentas.
- ADR-0006 atualizada (CI com o crate novo).

## Arquivos criados

**Rust — `packages/router` (`orchestrator-router`, crate novo)**
- `src/activity.rs`: `Activity` e perfis; `detect` com o verbo inicial
  valendo em dobro; `normalize`/`words` (sem acentos).
- `src/catalog.rs`: `CatalogModel`, catálogo de todos os providers
  registrados e `Availability` (`inspect` em paralelo, válido por 5 min).
- `src/score.rs`: `rank`, `Preference`, `RouteRequest`, `Candidate` com
  critérios e motivos, `Excluded`, `Recommendation`.
- `src/council.rs`: instruções (`SYSTEM`), `prompt`, `parse_ballot`,
  `tally` (Borda ponderada), `Vote`, `Decision`, `Deliberation`.
- `src/cache.rs`: `DeliberationCache` (validade e capacidade).
- `src/settings.rs`: `CouncilMode`, `CouncilMember`, `CouncilSettings`,
  validação, leitura e gravação atômica de `council.json`.
- `src/service.rs`: `RouterService`:
  - `recommend`, `deliberate`, `run`, `start_session`;
  - configuração, histórico e eventos.
- `src/lib.rs`, `tests/council.rs`, `Cargo.toml`, `README.md`.

**Rust — desktop**
- `apps/desktop/src-tauri/src/router_commands.rs`: `router_recommend`,
  `council_get`, `council_save`, `council_run`, `council_history`,
  `route_start_session`.

**TypeScript — `apps/desktop/src`**
- `components/RouteView.tsx`: aba "Nova sessão com o Conselho":
  - tarefa, atividade, preferência, ferramentas, contexto mínimo e envio da
    tarefa;
  - decisão, votos, ranking com "Usar este" e excluídos;
  - aprovar, deliberar de novo e o aviso da sessão aberta pelo Conselho.
- `components/CouncilEditor.tsx`: modo, membros (provider + modelo, até 5,
  com aviso de repetido), opções e deliberações recentes.
- `lib/council.ts` (+ teste), `lib/useCouncil.ts`.

**Documentação**
- ADR-0011, `docs/router.md`, `docs/phases/fase-5.md`,
  `docs/assets/fase-5-conselho.png`, `docs/assets/fase-5-configuracao.png`.

## Arquivos modificados

- `packages/core`:
  - `ids.rs`: `DeliberationId`;
  - `tool.rs`: `CallOrigin::Council { deliberationId? }` (+ teste);
  - `event.rs`: `COUNCIL_CONFIGURED`, `COUNCIL_DELIBERATED`,
    `ROUTE_DECIDED`;
  - `lib.rs`.
- `packages/providers`:
  - `provider.rs`: `AIProvider::complete`, `CompletionRequest`,
    `Completion`, capacidade `completion`;
  - `echo.rs`: `complete` (+ teste);
  - `lib.rs`, `README.md`.
- `packages/providers/api`:
  - `provider.rs`: `complete` com a mesma chamada HTTP dos turnos, sem
    ferramentas nem conversa, com custo pelos preços;
  - `tests/api.rs`: teste novo;
  - `README.md`.
- `apps/desktop/src-tauri`:
  - `lib.rs`: `RouterService` no `AppState`;
  - `provider_commands.rs`: salvar ou remover uma conexão descarta a
    disponibilidade guardada;
  - `Cargo.toml`.
- `apps/desktop/src`:
  - `App.tsx`: abas `council` e `route`, tela inicial;
  - `ProvidersPanel.tsx`: seção CONSELHO e capacidade "conselho";
  - `HistoryPanel.tsx`: filtros novos e origem "Conselho (Full)";
  - `ConnectionEditor.tsx`: dica das etiquetas;
  - `lib/types.ts`, `lib/runtime.ts` (`councilApi`), `styles.css`.
- Documentação e configuração:
  - `ARCHITECTURE.md`, `README.md`, `docs/providers.md`, `docs/ipc.md`,
    `docs/api-connections.md`;
  - ADR-0006 e o índice de ADRs;
  - `.github/workflows/ci.yml`: clippy e testes do crate novo nos 3 SOs;
  - `Cargo.toml` (workspace), `Cargo.lock`.

## Dependências instaladas

Nenhuma. O crate novo usa só dependências que o workspace já tinha
(`chrono`, `parking_lot`, `serde`, `serde_json`, `tokio`, `tokio-util`; e
`async-trait` e `tempfile` nos testes).

## Comandos executados

```bash
cargo test -p orchestrator-router                 # unitários + integração
cargo test --workspace
pnpm check        # tsc + cargo fmt --check + cargo clippy -D warnings (workspace)
pnpm test:web
cargo clippy -p orchestrator-core -p orchestrator-providers -p orchestrator-provider-api \
  -p orchestrator-router -p orchestrator-runtime -p orchestrator-git -p orchestrator-desktop \
  --all-targets --target x86_64-pc-windows-gnu -- -D warnings
cargo check -p orchestrator-providers -p orchestrator-router --all-targets --target x86_64-apple-darwin
pnpm tauri dev                  # app real sob Xvfb
pnpm tauri build --no-bundle    # release, validado num perfil limpo
```

Validação no app com uma API compatível com OpenAI simulada localmente. O
servidor Python exige a chave, faz streaming e responde às perguntas do
Conselho como um membro faria: o `gpt-grande` escolhe pela atividade e o
`gpt-mini` escolhe o mais barato.

## Testes executados

| Suíte | Qtde | Cobre |
| ----- | ---- | ----- |
| `orchestrator-router` (unit) | 15 | normalização; detecção de atividade (inclui "Resuma o README", "Escreva testes"); perfis; ranking de código (etiquetas, ferramentas obrigatórias, excluído com motivo); preferência muda o ranking (qualidade, custo com modelo grátis, velocidade); filtros com motivo (indisponível, ferramentas desligadas, contexto); dados desconhecidos neutros e explicados; etiquetas citadas na tarefa e tamanho de parâmetros; formatação; leitura de votos com cercas, números e confiança em %; respostas inválidas com motivo; Borda ponderada, empate e gerenciador; validação e arquivo de configuração; título da sessão |
| `orchestrator-router` (integração) | 9 | modo Desligado sem tokens nem evento; votos com abstenções (`echo` sem JSON, provider removido) e o que o membro recebe; cache (acerto, forçar, outra pergunta, nova configuração); sem votos válidos → roteador e Full não aplica (erro da API, sem `completion`, provider indisponível); membro lento estoura o prazo sem segurar os outros; Full abre a sessão, envia a tarefa e registra a origem `council`; usuário segue ou troca a recomendação (`ROUTE_DECIDED`); um ou nenhum candidato; configuração validada, persistida e registrada |
| `orchestrator-provider-api` (integração, novo) | 1 | `complete` na Anthropic e OpenAI: sem ferramentas, sem histórico entre chamadas, modelo escolhido, uso e custo, nenhuma sessão nem turno |
| `orchestrator-providers` (unit, novo) | 1 | `complete` do `echo` e modelo desconhecido |
| `orchestrator-core` (ampliado) | — | serialização da origem `council` |
| Frontend (vitest, novos) | 5 | ação por modo, membros possíveis e rótulos, membros repetidos, contexto (`128k`, `1M`), preços, fonte da decisão e resumo dos votos |
| Manual (app real, dev e release) | — | ver abaixo |

Validação manual:

- **Modo Desligado:** atividade detectada, ranking com motivos, excluídos
  e sessão iniciada com o recomendado.
- **Configuração:**
  - modo, membros e aviso de membro repetido (Salvar bloqueado);
  - salvo em `council.json` e mantido após reiniciar o app.
- **Modo Sugerir:**
  - dois membros votando pela API HTTP, com custo real (US$ 0,0182) e
    concordância de 50%;
  - "Aprovar e iniciar sessão": título tirado da tarefa e tarefa enviada;
  - "Usar este" em outro modelo: `ROUTE_DECIDED` "diferente da
    recomendação";
  - mesma pergunta respondida pelo cache, sem requisição nova e com a
    economia exibida.
- **Modo Full:**
  - o Conselho escolheu gpt-mini para um resumo e abriu a sessão sozinho,
    com a origem "Conselho (Full)" no HISTORY;
  - no release, a Borda escolheu gpt-medio (1º de um membro e 2º do outro)
    à frente de gpt-grande e gpt-mini.
- **Histórico e segurança:**
  - deliberação antiga reaberta pelo histórico;
  - nenhum arquivo do app contém a chave;
  - SIGTERM encerra o release normalmente.

## Resultado dos testes

- Rust: **178/178** (core 13, desktop 3, git 19, provider-api 31,
  providers 21, router 24, runtime 67; 1 teste de inspeção ignorado de
  propósito).
- Frontend: **37/37**; `tsc` limpo.
- `cargo clippy -D warnings` limpo no Linux e no alvo Windows (incluindo o
  crate Tauri); `cargo fmt --check` limpo.
- Checagem cruzada para macOS OK em `orchestrator-providers` e
  `orchestrator-router`.

## Problemas encontrados

| Problema | Resolução |
| -------- | --------- |
| Nos primeiros pesos, o modelo barato genérico passava o modelo com etiqueta "código" numa tarefa de código (modo equilíbrio), e o modelo grátis não vencia no modo custo | Uma etiqueta da atividade passou a valer 0,7 (duas, 1). O mais barato pago vale 0,9 e o grátis, 1. Pesos rebalanceados. Os testes fixam a ordem esperada |
| O bônus do modelo padrão da conexão (+2) decidia empates técnicos contra modelos mais adequados | Bônus reduzidos a desempate (+1 padrão, +0,5 provider ativo) |
| "Resuma o README" era detectado como documentação (empate com "readme") | O verbo inicial conta em dobro; "escrev" deixou de indicar documentação ("Escreva testes" é testes) |
| Lista curta menor que a esperada no teste | Não era bug: os modelos dos próprios jurados também são candidatos. O teste foi corrigido |
| A etiqueta "no Conselho" aparecia no modo Desligado | Só marca candidatos quando houve votos |
| "o mais barato entre os candidatos" aparecia com modelo grátis na lista | Com modelo grátis, o texto vira "o mais barato entre os pagos" (teste) |
| Seção "Modo" apertada (opções em 2×2) e campo de contexto desalinhado | Modo e membros em largura total; campo com a altura dos selects |
| A tela inicial ainda listava a Fase 5 como próxima | Conselho em "O que já funciona" |

**Limitações conhecidas:**

- O cache e o registro de deliberações ficam em memória até a Fase 6. O
  `audit.jsonl` guarda tudo.
- A pontuação é uma heurística: depende de etiquetas, preços e contexto
  bem preenchidos. A UI diz isso e mostra os motivos.
- Não houve teste com chaves reais: o Conselho foi validado com a API
  simulada e os protocolos, na Fase 4.
- O Conselho escolhe o modelo de sessões novas. Tasks e agentes (Fases
  8–9) usarão o mesmo serviço.
- Nenhum orçamento máximo por deliberação além do prazo e do tamanho da
  lista curta. Otimização de tokens é a Fase 11.
- Detecção de atividade por palavras-chave em português e inglês. Texto em
  outro idioma cai em "Geral", e o usuário pode escolher a atividade.

## Próxima fase

**Fase 6 — SQLite, memória e histórico.**

- Banco local do projeto, com projetos, sessões, mensagens, turnos, eventos
  de auditoria, conexões (sem segredos), configuração e deliberações do
  Conselho.
- Memória L1/L2/L3 e decisões.
- Sessões e conversas das APIs retomáveis após reiniciar o app.
- Cache do Conselho persistente.
- A arquitetura será registrada em ADR própria antes do código.

# Fase 11 — Tokens, cache, compactação de contexto e escalonamento de agentes

## STATUS

✅ **Concluída.** O que as IAs gastam passa a ser medido como o fornecedor
cobra, a parte repetida das requisições vem do cache, conversas longas
viram um resumo feito pela própria IA e os agentes respeitam limites de
custo:

- **custo real** com o cache: leituras a preço de cache, gravações a 1,25×
  ou 2× (Anthropic), e quanto o cache economizou;
- **cache de prompt** com prefixo estável: três marcadores `cache_control`
  na Anthropic (ferramentas compartilhadas entre agentes do mesmo modelo),
  `prompt_cache_key` por sessão na OpenAI oficial;
- **compactação de contexto** automática (limite por tokens e por % da
  janela) e pelo botão "Compactar", igual nos quatro protocolos;
- **retentativas** de 429, 5xx, 529 e falhas de conexão, com `retry-after`;
- **escalonamento de agentes:** fila por prioridade sem bloqueio de quem
  está atrás, limite por provider, teto de custo por agente, orçamento
  diário do projeto, subagentes configuráveis;
- **"Tokens e custo":** gasto de hoje na barra de status e no painel
  AGENTS, aba com 1, 7 ou 30 dias por provider e modelo;
- **supervisão de processos** contra queda do app (herdada da ADR-0016):
  Job Object no Windows, órfãos encerrados ao abrir no Linux e no macOS.

Com ela, as Fases 0–11 do documento mestre estão concluídas.

![A aba Tokens e custo e o orçamento diário acabado no painel AGENTS](../assets/fase-11-custo.png)

![Sessão compactada automaticamente, retentativa depois de um 429 e leitura do cache](../assets/fase-11-compactacao.png)

## Andamento

A fase foi feita em sete passos, cada um commitado e enviado ao terminar,
para poder ser retomada de onde parou.

1. ✅ ADR-0018 e este arquivo.
2. ✅ Custo real e cache de prompt: `TokenUsage` (gravação no cache,
   economia), preço do cache por modelo, marcadores `cache_control`
   (Anthropic), `prompt_cache_key` (OpenAI), retentativas; testes.
3. ✅ Compactação de contexto: adapter das APIs, `SessionManager`,
   `CONTEXT_COMPACTED`, `context.json`; testes.
4. ✅ Escalonamento de agentes: ordem da fila, limite por provider, tetos
   de custo, orçamento diário, `maxSubagents`; testes.
5. ✅ Supervisão de processos: Job Object (Windows) e processos órfãos
   (Linux/macOS); testes.
6. ✅ Desktop e UI: sessão, conexão, contexto, agentes, aba "Tokens e
   custo".
7. ✅ Validação no app real, documentação e publicação.

## Decisões registradas antes do código

- [ADR-0018](../adr/0018-tokens-cache-compactacao-e-escalonamento.md):
  - **nenhuma solução só de um fornecedor:** a compactação é feita pela
    própria IA no adapter das conexões, igual nos quatro protocolos; o
    cache de cada fornecedor é otimização a mais;
  - **custo** = o que o fornecedor cobra: `cachedInputPrice` por modelo
    (sem ele, preço cheio — na Anthropic, 10% da entrada), gravação 1,25×
    (5 min) ou 2× (1 h);
  - **prefixo estável:** instruções congeladas depois do primeiro turno,
    ferramentas fixas e conversa só acrescentada, até a compactação;
  - **compactação simples:** a requisição do resumo é a própria conversa
    com o pedido no fim (lê o cache); depois, só o resumo abre a próxima
    mensagem; nunca com ferramenta pendente; sem compactação do servidor
    nem "manter o fim" (invalidam o cache e as assinaturas de raciocínio);
  - **escalonamento:** prioridade e chegada, sem *head-of-line blocking*;
    `providerLimits`; `maxCostUsd` por agente; `dailyBudgetUsd` por projeto
    somando todas as IAs, mas parando só agentes — **as sessões do usuário
    nunca são bloqueadas**;
  - **retentativas:** até duas, `retry-after` até 60 s ou 2 s e 4 s;
  - **supervisão:** Job Object "kill on job close" no Windows; no Unix,
    registro `processes.json` e encerramento ao abrir, conferindo o
    horário de início do pid;
  - `CONTEXT_COMPACTED`; `TURN_COMPLETED` com o `model`; `AGENT_FINISHED`
    com o `reason`; **nenhuma migração** (esquema 4).

## Arquivos criados

**Rust**
- `packages/providers/api/src/cost.rs`: `Prices`, preço por modelo da
  conexão e o cálculo de custo e economia com leituras e gravações do
  cache.
- `packages/providers/api/src/compaction.rs`: o pedido de resumo, a
  estimativa do tamanho do prompt, quando compactar (com a guarda das
  instruções e ferramentas que sozinhas passam do limite), a requisição do
  resumo, o texto do resumo e a mensagem que o leva adiante.
- `packages/providers/api/tests/tokens.rs`: prefixo estável marcado para o
  cache, opções do cache, retentativas e as quatro situações de
  compactação, contra APIs falsas.
- `packages/memory/src/spend.rs`: `SpendReport`/`SpendRow`, o gasto lido do
  histórico por provider e modelo.
- `packages/runtime/src/supervisor.rs`: `Supervisor` (Job Object no
  Windows; registro, identidade do processo e encerramento de grupos no
  Unix) e `Orphan`.
- `packages/runtime/tests/supervision.rs`: órfãos de uma queda encerrados
  na próxima abertura; pid reaproveitado nunca é morto.
- `apps/desktop/src-tauri/src/cost_commands.rs`: `spend_report`,
  `agents_budget`, `session_compact`.

**TypeScript — `apps/desktop/src`**
- `components/CostView.tsx`: aba "Tokens e custo".
- `components/DraftInput.tsx`: campo numérico que só grava ao confirmar.
- `lib/cost.ts` (+ teste): proporção do cache e rótulos do período.

**Documentação**
- ADR-0018, `docs/tokens.md`, este relatório e
  `docs/assets/fase-11-custo.png`, `fase-11-compactacao.png`.

## Arquivos modificados

- `packages/core`: `session.rs` (`TokenUsage.cacheWriteTokens` e
  `cacheSavedUsd`, `SessionEvent::Compacted`), `event.rs`
  (`CONTEXT_COMPACTED`), `agent.rs` (`maxCostUsd`).
- `packages/providers`: `provider.rs` (`CompactionPolicy`,
  `TurnInput.compaction`/`compact`, `capabilities().compaction`),
  `context.rs` (`TurnContext::compacted`), `manager.rs` (política da
  compactação, `compact`, `CONTEXT_COMPACTED`, `model` no
  `TURN_COMPLETED`), `echo.rs`.
- `packages/providers/api`: `config.rs` (`cachedInputPrice`,
  `promptCache`, `cacheTtl`), `protocol.rs` (`cache_key`), `anthropic.rs`
  (marcadores, gravações no uso, tabela de referência com o preço do
  cache), `openai.rs` (`prompt_cache_key`), `http.rs` (retentativas),
  `provider.rs` (`ModelCall`, compactação antes do turno e entre rodadas,
  custo com cache), `conversation.rs` (resumo e tamanho do último
  prompt), `presets.rs`, testes de apoio.
- `packages/agents`: `settings.rs` (`maxSubagents`, `providerLimits`,
  `maxCostUsd`, `dailyBudgetUsd`), `service.rs` (fila por prioridade,
  limite por provider, tetos, orçamento, `BudgetView`, custo e posição de
  cada agente), `tools.rs` (descrição de `agent.delegate`), testes.
- `packages/memory`: `lib.rs`, `agents.rs`.
- `packages/orchestrator/src/settings.rs`: `ContextSettings.compaction`.
- `packages/runtime`: `process.rs` e `shell.rs` (supervisão), `lib.rs`
  (`open_process_registry`), `package.rs`, `Cargo.toml` (`windows-sys`).
- `apps/desktop/src-tauri`: `lib.rs` (comandos, compactação e registro de
  processos ao iniciar), `context_commands.rs`.
- `apps/desktop/src`: `App.tsx` (aba de custo, texto de boas-vindas),
  `SessionView.tsx` ("Compactar", conversa compactada), `ConnectionEditor.tsx`
  ("$ cache", "Cache de prompt", "Validade"), `ContextView.tsx`
  (compactação), `AgentsPanel.tsx` (gasto de hoje, limites, custo e
  posição), `TaskView.tsx` (teto do agente), `StatusBar.tsx` (gasto de
  hoje), `HistoryPanel.tsx`, `lib/types.ts`, `lib/runtime.ts`,
  `lib/transcript.ts`, `lib/connections.ts`, `lib/format.ts`,
  `lib/agents.ts`, `lib/useAgents.ts`, `styles.css`.
- Documentação: `README.md`, `ARCHITECTURE.md`, `docs/agents.md`,
  `docs/context.md`, `docs/tool-runtime.md`, `docs/providers.md`,
  `docs/api-connections.md`, `docs/ipc.md`, índices de ADRs e de fases.

## Dependências instaladas

- `windows-sys` 0.59 no `orchestrator-runtime`, só no Windows (Job Object).
  Já estava no `Cargo.lock` como dependência indireta.

## Comandos executados

```bash
cargo test -p orchestrator-provider-api -p orchestrator-agents -p orchestrator-runtime
cargo test --workspace
cargo clippy --workspace --all-targets --target x86_64-pc-windows-gnu -- -D warnings
pnpm check        # tsc + cargo fmt --check + cargo clippy -D warnings (workspace)
pnpm test:web
pnpm tauri dev    # app real sob Xvfb, sobre o perfil da Fase 10, API de IA simulada
```

## Testes executados

| Suíte | Qtde | Cobre |
| ----- | ---- | ----- |
| `orchestrator-provider-api` unitários (novos) | 9 | custo sem preço é desconhecido; entrada e saída simples; leituras, gravações (5 min e 1 h) e economia na Anthropic; outros fornecedores leem a preço cheio sem `cachedInputPrice`; contagens incoerentes não estouram; `prompt_cache_key` só na OpenAI oficial e só em sessões; o resumo sai das tags; o limite é o menor entre tokens e % da janela; a requisição do resumo termina com o pedido e o resumo abre o que vem depois |
| `orchestrator-provider-api` integração (`tests/tokens.rs`) | 7 | duas requisições seguidas repetem o prefixo, com os três marcadores e o uso de leitura/gravação; cache desligado e validade de 1 h; 429 com `retry-after`, 529 e conexão recusada repetidos, aviso na sessão, desistência depois de duas; compactação automática entre turnos (resumo lendo o cache, só o resumo depois, evento e histórico); "Compactar" sob pedido; resumo que falha deixa a conversa como estava; compactação entre rodadas de ferramentas sem perder a chamada |
| `orchestrator-agents` integração (novos) | 4 | prioridade e limite por provider sem bloquear quem está atrás; teto de custo para com handoff e `reason: "costCeiling"`, e o teto dado ao executar vence o das configurações; orçamento diário para os agentes no fim do turno e segura a fila até subir; `maxSubagents: 0` desliga a delegação |
| `orchestrator-memory` (novo) | 1 | gasto por provider e modelo, deliberações do Conselho (sem as vindas do cache), chamadas sem preço, compactações, filtro por projeto e por data |
| `orchestrator-runtime` integração (`tests/supervision.rs`) | 2 | o grupo que uma "queda" deixou é encerrado na próxima abertura, com `PROCESS_EXITED` `orphan`; pid reaproveitado nunca é morto; processos encerrados saem do registro |
| `orchestrator-runtime` (Windows) | 1 | fechar o job encerra os processos dele (roda só no Windows) |
| `orchestrator-engine` (ampliados) | — | `context.json` com a compactação, valores fora da faixa recusados |
| Desktop | 1 | o começo de "hoje" e dos últimos N dias |
| Frontend (novos e ampliados) | 4 | conversa compactada e avisos de retentativa no transcript; preço do cache no editor da conexão; valores em US$ com décimos de centavo; posição na fila e custo do agente; proporção do cache e períodos |
| Manual (app real) | — | ver abaixo |

Validação manual (dev, sobre o perfil da Fase 10; uma API de IA simulada
compatível com a OpenAI que informa tokens lidos do cache, responde 429
sob pedido e tem roteiros de agente lento e sem fim):

- **Preço do cache:** a coluna "$ cache" e a opção "Cache de prompt"
  foram salvas na conexão "Nuvem Cache".
- **Custo com cache:** um turno com 16.000 tokens lidos do cache mostrou
  "16.000 do cache · cache economizou US$ 0.0432".
- **Retentativa:** com "teste de limite", a API respondeu 429; a sessão
  mostrou "rate limit or quota exceeded (HTTP 429): … — tentando de novo
  em 1 s (1 de 2)" e o turno terminou bem.
- **Compactação automática:** com o limite em 30.000 tokens na aba
  Contexto (gravado em `context.json`), o turno seguinte compactou: "6
  mensagens · ~30.060 → ~9.350 tokens". Na API, a requisição do resumo leu
  30.000 tokens do cache, e a seguinte levou só as instruções e uma
  mensagem com o resumo.
- **"Compactar":** "Conversa compactada a pedido · 2 mensagens · ~16.060 →
  ~9.327 tokens".
- **Teto de custo:** o agente "Trabalho sem fim" (teto US$ 0,10) parou
  depois de 3 turnos, com handoff, `reason: "costCeiling"`.
- **Limite por provider:** com `nuvem-cache: 1`, o segundo agente esperou
  com "1º na fila · a conexão nuvem-cache já tem 1 agente (limite)".
- **Orçamento diário:** com US$ 0,30 e US$ 0,315 gastos, o painel avisou
  que o orçamento acabou, a barra de status ficou vermelha ("hoje: US$
  0,315 de US$ 0,30 +?") e o agente da fila mostrou "o orçamento diário do
  projeto acabou (US$ 0,315 de US$ 0,300)". Subir para US$ 1 iniciou o
  agente na hora. Com US$ 0,58, o agente "sem fim" parou no fim do 3º
  turno com handoff, `reason: "dailyBudget"`. As sessões do usuário
  continuaram respondendo.
- **Tokens e custo:** hoje, US$ 0,585, "61% da entrada veio do cache
  (257.000 tokens) · economizou US$ 0,694", 25 chamadas, 2 compactações;
  o `echo`, sem preço, aparece como "sem preço" e soma "12 chamadas sem
  preço".
- **Queda do app:** com um `process.start` (`sleep`) rodando, o app foi
  morto com `kill -9`; o `sleep` sobreviveu, e na abertura seguinte foi
  encerrado (`PROCESS_EXITED` com `reason: "orphan"`) e o
  `processes.json` ficou vazio.
- Banco no esquema 4, `integrity_check` ok; nenhuma chave de API no banco,
  nos arquivos de configuração ou nos logs.

## Resultado dos testes

- Rust: **312/312** (agents 17, core 22, desktop 3, engine 42, git 34,
  memory 24, provider-api 49, providers 25, router 25, runtime 71; 1 teste
  de inspeção ignorado de propósito). `cargo clippy` também para
  `x86_64-pc-windows-gnu` (Job Object).
- Frontend: **68/68**; `tsc` limpo.
- `cargo clippy -D warnings` e `cargo fmt --check` limpos.

## Problemas encontrados

| Problema | Resolução |
| -------- | --------- |
| Chamadas avulsas (teste de conexão, Conselho) recebiam marcadores de cache, que só gastam gravação | Marcadores só em requisições de sessão (`cache_key` presente) |
| Instruções e ferramentas que sozinhas passam do limite fariam a compactação repetir a cada turno | Guarda: não compacta quando o que fica depois do resumo já passa do limite |
| Num teste, a API falsa respondia à requisição depois da compactação como se fosse a anterior, em laço | O teste distingue as requisições pela primeira mensagem do usuário |
| A CI de um commit falhou em todos os jobs em 2 s, sem logs (runner 0) | Falha da infraestrutura do GitHub; o push seguinte passou |
| No Windows, o teste do Job Object esperava código de saída diferente de zero, mas o Windows encerra os processos de um job fechado com código 0 (o `ping -n 60` terminou em milissegundos, logo o job funcionou) | O teste confere que o processo segue vivo meio segundo depois de começar e termina em segundos quando o job fecha, sem olhar o código |
| Da etapa 4 em diante a CI falhou no clippy: o Rust estável da CI (1.98) traz o lint `unnecessary_sort_by`, que o 1.94 local não tinha, na ordenação da fila de agentes | `sort_by_key`; clippy e rustfmt do 1.98 rodados localmente no workspace e no alvo Windows antes do push |
| O valor de hoje na barra de status alargava o layout e escondia "Encerrar" | A barra não passa da janela: o caminho do projeto encolhe com reticências |
| A linha do orçamento no painel AGENTS se sobrepunha ao link "Tokens e custo" | Valor e link em linhas separadas |
| O campo "teto US$" na aba da task apertava os seletores de provider e modelo | O campo foi para junto de "Executar com um agente" e os seletores têm largura mínima |
| "gastou US$ 0.10" escondia US$ 0,104 e o motivo usava ponto decimal | Décimos de centavo abaixo de US$ 1 e vírgula decimal, como na UI |
| A tabela de custo alinhava os títulos das colunas numéricas à esquerda e mostrava "US$ 0,00 +?" para modelos sem preço | Títulos à direita; "sem preço" quando nenhuma chamada tem preço |
| A troca de código do backend durante o `tauri dev` reinicia o app e volta o provider ativo para o `echo` | Comportamento anterior do modo dev; o agente que começou no `echo` falhou como esperado e o provider foi reativado |

**Limitações conhecidas:**

- A compactação perde detalhes: o resumo é tudo o que a IA tem do que veio
  antes (a tela, o histórico e a memória do projeto continuam com tudo).
- Compactar recomeça o cache das mensagens.
- Sem compactação ou edição de contexto do lado do servidor e sem cache
  explícito do Gemini.
- Tetos conferidos entre turnos (um turno pode passar do teto); orçamento
  só por dia e por projeto.
- Chamadas sem preço não contam para os tetos (a UI mostra quantas são).
- No Linux e no macOS, processos que sobram de uma queda vivem até o app
  abrir de novo.
- O valor por turno na sessão continua em quatro casas com ponto decimal
  (formato da Fase 4); os totais usam vírgula.

## Próxima fase

As Fases 0–11 do documento mestre estão concluídas. O que ficou para
depois está nas seções "Fica para depois" das ADRs — da 0018: compactação
do lado do servidor, cache explícito do Gemini, orçamentos por mês,
provider ou task, escalonamento por previsão de custo e busca semântica no
contexto.

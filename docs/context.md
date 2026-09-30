# Context Builder e Handoff entre IAs

Referência da Fase 7. Decisão em
[ADR-0013](./adr/0013-context-builder-e-handoff.md); código em
[`packages/orchestrator`](../packages/orchestrator/README.md) (crate
`orchestrator-engine`). A memória consultada aqui está em
[memory.md](./memory.md).

> Máxima eficiência com mínimo de tokens: cada IA recebe a parte relevante
> do projeto, nunca o histórico inteiro nem o repositório. O resto ela
> busca quando precisar.

```text
primeiro turno de uma sessão
   │
   ▼
SessionManager ──ContextSource::build──▶ ContextBuilder ──▶ MemoryStore (L1, L2, decisões, L3)
   │                                          │           └▶ git status
   │◀──────── texto + resumo ─────────────────┘
   ├─ TurnInput.context ──▶ provider (API: instruções de sistema; echo: /context)
   ├─ SessionEvent::ContextAttached (transcript)
   └─ CONTEXT_BUILT (histórico, só o resumo)
```

## O contexto de uma sessão

No **primeiro turno** de cada sessão, o Orchestrator monta o contexto com
a mensagem como tarefa e o entrega ao provider. Vale para todos os
caminhos: nova sessão, Conselho no modo Full, subagente e handoff.

- **Uma vez por sessão:** os turnos seguintes não repetem o contexto. Nas
  conexões de API ele fica nas instruções de sistema da conversa, iguais
  em todas as requisições da sessão e guardadas com ela.
- **Sem IA para montar:** a escolha é por regras e pela busca do banco
  (FTS5). Montar o contexto não gasta tokens.
- **Só caminhos:** arquivos entram pelo caminho, nunca pelo conteúdo. Para
  ler, a IA usa `filesystem.read`.
- **Rótulos em inglês:** as seções têm os nomes do documento mestre, e o
  conteúdo vem como está no projeto.
- **Se falhar** (banco ou Git indisponível), o turno segue sem contexto e
  o transcript recebe o aviso "contexto do projeto indisponível: …".

### Seções

Na ordem do documento mestre. Uma seção vazia não entra.

| Seção | Conteúdo | Como é escolhido | Limite |
| ----- | -------- | ---------------- | ------ |
| `TASK` | numa sessão comum, só a indicação de que a tarefa é a primeira mensagem (que já vai na conversa, então não se repete); num handoff, "assuma o trabalho do HANDOFF pela NEXT ACTION" | sempre | — |
| `WORKING MEMORY` | outras sessões do projeto (provider, estado, turnos, última atividade) e os últimos comandos com o código de saída | L1, os mais recentes; a própria sessão fica de fora | 4 sessões, 5 comandos |
| `PROJECT MEMORY` | entradas L2 e decisões | fixadas sempre; as demais quando casam com a tarefa; decisões que casam com a tarefa e as aceitas mais recentes | 6 fixadas + 4, 3 + 3 decisões |
| `RELEVANT FILES` | caminhos | citados na tarefa (e que existem no projeto) e alterados recentemente pelo L1, sem repetir os do Git | 5 citados + 6 alterados |
| `RECENT ERRORS` | falhas com o detalhe | L1 dos últimos 7 dias: ferramentas, turnos, comandos e processos | 4 |
| `RELEVANT HISTORY` | trechos de mensagens, eventos e handoffs anteriores | busca L3 pelas palavras da tarefa, sem a própria sessão e sem repetir `RECENT ERRORS` | 4 |
| `GIT STATE` | branch, upstream, à frente/atrás, arquivos alterados em palavras ("modified (not staged)", "untracked") | `git status` do projeto | 10 arquivos + "… and N more" |
| `HANDOFF` | o pacote do handoff | quando a sessão assume um handoff | limites do pacote |

- **Relevância:** qualquer palavra significativa da tarefa conta (busca
  `OR`, ordenada pelo ranking), sem palavras curtas, números e palavras
  comuns em português e inglês. Num handoff, a "tarefa" é o objetivo, a
  próxima ação e o que falta.
- **Handoff sem vazamento:** no contexto de quem assume um handoff, as
  mensagens da sessão de origem e o próprio handoff não voltam como
  histórico. O pacote é tudo o que vem daquela conversa.
- **Projeto sem memória** (nunca aberto no app): só tarefa, Git e handoff,
  com uma nota na prévia.
- **Cabeçalho:** o texto começa com `# PROJECT CONTEXT`, o projeto e o
  aviso de que há mais memória pelas ferramentas `memory.search`,
  `memory.list`, `memory.working` e `decision.list`, e arquivos por
  `filesystem.read`. Se o provider da sessão não pede ferramentas (conexão
  com ferramentas desligadas), o aviso diz que elas não estão disponíveis
  e que a IA deve pedir ao usuário o que precisar.

### Orçamento

- **Estimativa:** ~4 caracteres por token, a mesma do provider `echo`.
- **Padrão:** 1.500 tokens; de 300 a 8.000. Cada sessão pode ter o seu.
- **Corte:** acima do orçamento, saem itens (o último de cada seção
  primeiro) nesta ordem: `RELEVANT HISTORY`, `RELEVANT FILES`,
  `RECENT ERRORS`, L2 não fixada e decisões, `WORKING MEMORY`, L2 fixada e
  `GIT STATE`. `TASK` e `HANDOFF` nunca são cortados.
- **Transparência:** o que saiu vai para `omitted`
  ("RELEVANT HISTORY: 2 itens (orçamento)"), no transcript, no histórico e
  na prévia.

### A tarefa vem da task (Fase 8a)

Quando a sessão é aberta a partir de uma task
([tasks.md](./tasks.md)), o título, a descrição e os arquivos dela são a
tarefa do contexto, e a mesma mensagem abre a conversa. É o *task-scoped
context* da seção 23 do documento mestre: o contexto passa a seguir o
trabalho planejado, não só a frase que o usuário digitou.

Um agente (Fase 8b, [agents.md](./agents.md)) abre essa mesma sessão e
recebe esse mesmo contexto — uma vez, no primeiro turno. Os turnos
seguintes só levam uma continuação curta: o contexto já foi.

### Configuração

`<app-data>/context.json`, editável na aba "Contexto do projeto":

```json
{ "version": 1, "context": { "autoAttach": true, "budgetTokens": 1500 } }
```

- `autoAttach`: anexar o contexto à primeira mensagem de toda sessão nova.
- `budgetTokens`: orçamento padrão.
- Arquivo inválido: o app usa o padrão e mostra o aviso na aba.

**Por sessão** (`StartRequest.context` ou `session_context_set`, antes da
primeira mensagem): `enabled` (desligar ou ligar só nesta), `budget` e
`handoffId`. Uma sessão que assume um handoff sempre recebe o contexto.

### Registro

| Onde | O quê |
| ---- | ----- |
| Transcript | `contextAttached { turnId, summary }`: tokens, orçamento, seções (itens e tokens), o que ficou de fora e o handoff |
| Histórico | `CONTEXT_BUILT` (origem `system`): `sessionId`, `provider`, `turnId`, `projectPath`, `tokens`, `budget`, `sections`, `omitted`, `handoffId` |

O texto do contexto não vai para o histórico. No `echo`, `/context` mostra
o texto recebido.

## Ferramentas de memória das IAs

O `EngineTools` envolve o executor de ferramentas do app e oferece às IAs,
com JSON Schema gerado dos tipos Rust:

| Ferramenta | Argumentos | Faz | Somente leitura |
| ---------- | ---------- | --- | --------------- |
| `memory.working` | — | L1: sessões, arquivos, comandos e erros recentes | sim |
| `memory.search` | `query`, `limit?` (8, até 20) | busca por qualquer palavra em memória, decisões, mensagens, handoffs e eventos | sim |
| `memory.list` | `kind?` | entradas L2, fixadas primeiro | sim |
| `memory.save` | `id?`, `kind`, `title`, `content`, `tags?`, `pinned?` | cria uma entrada L2 ou altera uma criada por IA | não |
| `decision.list` | `status?` | decisões, mais novas primeiro | sim |
| `decision.save` | `id?`, `title`, `context?`, `decision`, `consequences?`, `status?` (padrão `proposed`) | registra uma decisão ou muda uma (ex.: o estado) | não |

- **Mesmo caminho das outras ferramentas:** cada chamada gera
  `TOOL_CALLED` com origem `agent` (argumentos longos cortados no
  registro), mais `MEMORY_SAVED`/`DECISION_SAVED` com o mesmo `callId`.
- **Autoria:** o que uma IA grava tem origem `agent`. Entradas do usuário e
  do detector não são alteradas por IA; ela registra outra.
- **Sem remoção:** apagar memória continua só com o usuário; decisões nunca
  são apagadas.
- **Projeto:** o da sessão que chamou; sem ele, o projeto aberto.

## Handoff entre IAs

Uma IA assume o trabalho de outra sem receber a conversa anterior, pelo
`HandoffPacket`:

| Campo | Conteúdo | Limite |
| ----- | -------- | ------ |
| `goal` | o que o trabalho entrega (obrigatório) | 600 caracteres |
| `status` | onde está | 600 |
| `completed`, `remaining` | feito e a fazer | 15 itens de 300 caracteres |
| `files`, `commands`, `errors`, `decisions`, `tests` | fatos do trabalho | 15 itens de 300 caracteres |
| `nextAction` | o primeiro passo concreto de quem assume | 600 |

Itens em branco ou repetidos saem; marcadores ("-", "*", "•") são
removidos.

### 1. Preparar (`handoff_prepare`)

Gera um rascunho, sem gravar nada:

- **Fatos do histórico,** sem custo:
  - objetivo: a primeira mensagem da sessão; numa sessão que assumiu um
    handoff, o objetivo dele (o trabalho continua o mesmo);
  - arquivos que ela alterou (caminho relativo e o tipo de mudança);
  - comandos com o resultado ("ok", "saída 2");
  - testes: comandos de executores conhecidos (`pnpm test`, `cargo test`,
    `pytest`, `vitest`…), com "(passou)" ou "(falhou)";
  - erros da sessão;
  - decisões do projeto registradas enquanto ela estava aberta.
- **Narrativa da IA da sessão** (opcional, padrão ligado): um turno pede
  **só JSON** com `goal`, `status`, `completed`, `remaining`, `errors`,
  `decisions`, `tests` e `nextAction`, no idioma da conversa.
  - A leitura é tolerante: pega o primeiro objeto JSON do texto (aceita
    prosa e cerca de código em volta), nomes alternativos (`objective`,
    `done`, `todo`, `next_action`…) e itens como texto ou lista.
  - Os exemplos do próprio pedido ("what the work is for", "done
    items"…) não valem como resposta: uma IA que só repete o pedido não
    escreve a narrativa.
  - Uma sessão encerrada é retomada para isso. Com um turno em andamento,
    o pedido é recusado (a UI espera o turno terminar).
  - O turno aparece no transcript como "Orchestrator · pedido do resumo do
    handoff", com o custo.
- **Mistura:** a IA escreve a narrativa; arquivos, comandos e testes vêm do
  histórico, com o que a IA acrescentar.
- **Sem a IA** (provider fora do ar, resposta inválida, pedido desligado):
  o rascunho fica só com os fatos, com o motivo em `notes`, e o usuário
  completa.

### 2. Criar (`handoff_create`)

Valida (objetivo obrigatório), aplica os limites, grava e registra
`HANDOFF_CREATED`. O transcript da sessão de origem recebe o aviso "Handoff
criado: …".

### 3. Assumir (`handoff_start`)

- Abre uma sessão no mesmo projeto com o provider e o modelo escolhidos
  (qualquer um, inclusive outro modelo do mesmo provider), com o título
  "Handoff · <objetivo>".
- O contexto dela inclui a seção `HANDOFF`:

  ```text
  From session "Nuvem A #2" (nuvem-a/gpt-medio), 2026-09-29 19:40 UTC.
  GOAL: Retentativas no gateway de pagamentos
  STATUS: Política definida; falta aplicar no cliente do gateway
  COMPLETED:
  - Regra de 3 retentativas com backoff registrada na memória
  …
  NEXT ACTION: Criar config/pagamentos.yml com os limites de retentativa
  ```

- A primeira mensagem, em português, diz que o trabalho vem de outra sessão,
  que a conversa anterior não está disponível e que ela deve começar pela
  próxima ação.
- As duas sessões recebem `handedOff` no transcript, e o histórico registra
  `HANDOFF_ACCEPTED` (de, para, provider, modelo).
- **Uso único:** um handoff é assumido uma vez ("este handoff já foi
  assumido"). Para passar adiante de novo, cria-se outro a partir da nova
  sessão. A sessão de origem continua como está.
- Se a primeira mensagem falhar, a sessão fica aberta e `sendError` diz
  por quê.

### Eventos

| Evento | `data` |
| ------ | ------ |
| `HANDOFF_CREATED` | `handoffId`, `projectId`, `projectPath`, `fromSession`, `provider`, `model`, `goal`, `nextAction`, `byAgent`, contagens de `completed`, `remaining`, `files`, `errors` |
| `HANDOFF_ACCEPTED` | `handoffId`, `projectId`, `projectPath`, `fromSession`, `fromProvider`, `toSession`, `provider`, `model` |

## UI

- **Sessão, antes da primeira mensagem:** a barra "Anexar o contexto do
  projeto à primeira mensagem", com a estimativa (tokens e seções) para o
  que está sendo digitado e "Ver prévia". Numa sessão de handoff, a barra
  indica o handoff e não pode ser desligada.
- **Sessão, depois:** a linha "Contexto do projeto anexado · ~N tokens de
  M · K seções", que abre as seções com itens e tokens.
- **Aba "Contexto do projeto":**
  - configuração (anexar sempre, orçamento padrão);
  - prévia para uma tarefa, com orçamento próprio;
  - cada seção com os itens enviados ("3 de 5 itens" quando o orçamento
    cortou);
  - o que ficou de fora e o texto exato enviado à IA.
- **Botão "Handoff" da sessão** abre a aba Handoff:
  1. **Rascunho:** gerar com ou sem a IA da sessão, com o custo do resumo;
  2. **Revisar o pacote:** todos os campos editáveis, um item por linha;
  3. **Quem assume:** provider e modelo, "Sugerir com o roteador" (o
     roteador da Fase 5 recomenda pelo objetivo e o que falta), "Salvar
     para depois" ou "Passar para …".

  Depois de salvo: "Contexto da nova IA" mostra o que ela vai receber, e
  um handoff aceito leva à sessão que o assumiu.
- **MEMORY → Trabalho (L1):** os handoffs do projeto, pendentes e aceitos.
  A busca L3 também os encontra.
- **HISTORY:** filtros para `CONTEXT_BUILT`, `HANDOFF_CREATED` e
  `HANDOFF_ACCEPTED`.

## IPC

Ver [ipc.md](./ipc.md#contexto-e-handoff-fase-7-adr-0013).

## Limitações

- **Relevância por palavras** (FTS5), sem busca semântica: uma tarefa vaga
  recebe pouco contexto específico (fixadas, L1 e Git).
- **Contexto congelado:** é o do primeiro turno. Numa sessão longa, a IA
  deve consultar as ferramentas de memória.
- **Instruções maiores:** o contexto vai em todas as requisições da
  sessão. Cache de prompt e compactação ficam para a Fase 11.
- **Narrativa custa um turno;** os fatos não custam nada.
- **Handoff manual na conversa:** quem o pede é o usuário. O automático
  existe só quando um agente para no meio (Fase 8b).
- **Conselho:** os membros continuam sem contexto, arquivos e ferramentas
  (ADR-0011); só a sessão que o modo Full abre recebe o contexto.

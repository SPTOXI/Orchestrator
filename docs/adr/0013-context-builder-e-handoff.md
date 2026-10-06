# ADR-0013 — Context Builder e Handoff entre IAs

- **Estado:** Aceita
- **Fase:** 7

## Contexto

Desde a Fase 6 o projeto tem memória: histórico, L1, L2, decisões e busca
L3 (ADR-0012). Mas nada disso chega às IAs:

- uma sessão nova só recebe instruções genéricas e a mensagem do usuário;
- para trocar de IA, o usuário precisa reexplicar tudo ou colar a conversa.

O documento mestre pede, para a Fase 7:

- **Context Builder** (seção 16): montar para cada execução
  `TASK + WORKING MEMORY + RELEVANT PROJECT MEMORY + RELEVANT FILES + RECENT
  ERRORS + RELEVANT HISTORY + GIT STATE + HANDOFF`, e somente o relevante:
  "máxima eficiência com mínimo de tokens";
- **nunca enviar automaticamente** o histórico inteiro, o repositório, todos
  os arquivos ou todas as mensagens (seções 15 e 23);
- **Handoff** (seção 17): uma IA assume o trabalho de outra sem receber a
  conversa anterior, por um `HandoffPacket` com `goal, status, completed,
  remaining, files, commands, errors, decisions, tests, nextAction`;
- eventos `HANDOFF_CREATED` e `HANDOFF_ACCEPTED` (seção 22).

A ADR-0012 deixou para esta fase as ferramentas com que as IAs leem e
escrevem a memória.

## Decisão

### 1. Crate `packages/orchestrator` (`orchestrator-engine`)

- Contém o Context Builder, as ferramentas de memória das IAs e o serviço de
  handoff.
- Depende de `core`, `providers`, `memory` e `git`. Recebe o executor de
  ferramentas do app como `ToolExecutor`, então não depende do runtime.
- O app (Tauri) só liga as peças, como nas fases anteriores. O
  `StoreSessions` (o `SessionStore` sobre o banco, da Fase 6) muda do app
  para este crate, que já depende de `providers` e `memory`.

### 2. O Context Builder

`ContextBuilder::build(ContextRequest) → ContextPack`. O pedido traz o
projeto, a tarefa (texto), um handoff opcional e o orçamento. O pacote traz
as seções, a estimativa de tokens, o que ficou de fora e o texto final.

| Seção | Conteúdo | Como é escolhido |
| ----- | -------- | ---------------- |
| `TASK` | numa sessão comum, a indicação de que a tarefa é a primeira mensagem, que já vai na conversa e não se repete; num handoff, assumir o trabalho pela próxima ação | sempre |
| `WORKING MEMORY` | sessões recentes do projeto e últimos comandos com o código de saída | L1 (ADR-0012), os mais recentes |
| `PROJECT MEMORY` | entradas L2 e decisões | fixadas sempre; as demais só quando a busca pelas palavras da tarefa as encontra; decisões aceitas recentes e as que casam com a tarefa |
| `RELEVANT FILES` | **só caminhos**, nunca conteúdo | citados na tarefa (e que existem), alterados recentemente (L1) e alterados no Git |
| `RECENT ERRORS` | falhas recentes do projeto | L1: ferramentas e turnos que falharam, comandos com saída ≠ 0 |
| `RELEVANT HISTORY` | trechos de mensagens, eventos e handoffs anteriores | busca L3 pelas palavras da tarefa, sem repetir memória, decisões, a própria sessão e as falhas de `RECENT ERRORS`; num handoff, sem as mensagens da sessão de origem nem o próprio handoff |
| `GIT STATE` | branch, upstream, à frente/atrás e arquivos alterados | `git status` do projeto |
| `HANDOFF` | o pacote do handoff | quando a sessão assume um handoff |

- **Sem tokens:** a escolha é por regras e pela busca FTS da Fase 6.
  Nenhuma IA é chamada para montar o contexto.
- **Relevância:** qualquer palavra significativa da tarefa conta (consulta
  `OR`, ordenada pelo ranking), sem palavras curtas, números e palavras
  comuns em português e inglês. A busca da UI continua exigindo todas as
  palavras.
- **Orçamento:** estimativa de ~4 caracteres por token, padrão de 1.500
  tokens.
  - Cada seção tem um limite de itens e cada item, de caracteres.
  - Acima do orçamento, saem itens na ordem inversa de prioridade:
    histórico, arquivos, erros, L2 não fixada, L1 e Git.
  - `TASK` e `HANDOFF` nunca são cortados; o pacote já tem limites
    próprios.
  - O que saiu é informado com o motivo.
- **Recuperação sob demanda:** o texto termina dizendo que há mais
  memória acessível pelas ferramentas do item 4. A IA busca o resto quando
  precisar, em vez de receber tudo. Numa sessão cujo provider não pede
  ferramentas, o texto diz isso e não aponta para elas.
- **Rótulos em inglês:** as seções usam os nomes do documento mestre, e o
  conteúdo vem como está no projeto.

### 3. Onde o contexto entra

- **Um gancho no `SessionManager`:** `ContextSource`, instalado com
  `set_context_source`. No **primeiro turno** de cada sessão, o manager pede o
  contexto, usando a mensagem como tarefa. Isso vale para todos os
  caminhos: nova sessão pela UI, Conselho (inclusive o modo Full),
  subagentes e handoff.
- **Nos providers:**
  - `TurnInput` ganha `context: Option<String>`;
  - as conexões de API anexam o contexto às instruções de sistema da
    conversa, que ficam iguais nos turnos seguintes e vão junto na
    persistência;
  - o `echo` guarda o contexto e o mostra com `/context`.
- **Uma vez por sessão:** os turnos seguintes não repetem o contexto. Para
  dados novos, a IA usa as ferramentas de memória.
- **Opções por sessão** (`StartRequest.context`):
  - `enabled` (padrão: a configuração global);
  - `budget`;
  - `handoffId`.

  Elas são gravadas com a sessão e mudam só antes do primeiro turno
  (`session_context_get`/`session_context_set`).
- **Registro:**
  - no transcript, `SessionEvent::ContextAttached` (tokens, seções e o que
    ficou de fora);
  - no histórico, `CONTEXT_BUILT` (evento novo), com o mesmo resumo e sem o
    texto.
- **Se o contexto falhar** (banco ou Git indisponível), o turno segue sem
  ele, com um aviso no transcript.
- **Configuração:** `<app-data>/context.json` (`autoAttach`,
  `budgetTokens`), como o `council.json` (ADR-0012: configuração em
  arquivo).
- **O Conselho não recebe contexto:** os membros continuam sem arquivos,
  chaves nem ferramentas (ADR-0011).

### 4. Ferramentas de memória para as IAs

`EngineTools` envolve o executor do app e acrescenta:

| Ferramenta | Faz | Somente leitura |
| ---------- | --- | --------------- |
| `memory.working` | L1 do projeto | sim |
| `memory.search` | busca L3 | sim |
| `memory.list` | entradas L2 (filtro por tipo) | sim |
| `memory.save` | cria uma entrada L2, ou altera uma criada por IA | não |
| `decision.list` | decisões do projeto | sim |
| `decision.save` | registra uma decisão ou muda o estado de uma | não |

- **Mesmo caminho das outras ferramentas:** o JSON Schema dos argumentos
  sai dos tipos Rust. Cada chamada gera `TOOL_CALLED` com origem `agent`,
  mais `MEMORY_SAVED`/`DECISION_SAVED`.
- **Autoria:** o que uma IA grava tem origem `agent` e aparece assim na UI.
  Entradas do usuário e do detector não são alteradas por IA; ela cria
  outra.
- **Sem remoção:** apagar memória continua sendo só do usuário. Decisões
  nunca são apagadas.
- **Projeto:** o da sessão que chamou (tabela `sessions`) ou o aberto.

### 5. `HandoffPacket` e o fluxo de handoff

- **Contrato no `core`:**
  - `HandoffPacket { goal, status, completed[], remaining[], files[],
    commands[], errors[], decisions[], tests[], nextAction }`, com textos
    simples e editáveis;
  - `Handoff { id, projectId, from, to?, packet, status, createdAt,
    acceptedAt? }`, com `status` `created` ou `accepted`.
- **Preparar** (`handoff_prepare`): gera um rascunho, sem gravar nada.
  - **Fatos do histórico da sessão:**
    - objetivo = a primeira mensagem (numa sessão que assumiu um handoff,
      o objetivo dele);
    - arquivos que ela alterou;
    - comandos com o código de saída;
    - testes (comandos de executores de teste conhecidos);
    - erros (ferramentas, turnos, comandos);
    - decisões do projeto no período.

    São confiáveis e não custam tokens.
  - **Narrativa da IA atual:** se a sessão está disponível e o usuário
    quiser, a IA recebe um turno pedindo **só JSON** com `goal`, `status`,
    `completed`, `remaining`, `errors`, `decisions`, `tests` e
    `nextAction`. A leitura é tolerante, como a dos votos do Conselho. O
    turno aparece no transcript como pedido do Orchestrator (não do
    usuário), com o custo. Os exemplos do próprio pedido não valem como
    resposta.
  - **Sem a IA** (provider fora do ar, que muitas vezes é o motivo do
    handoff, ou resposta inválida): o rascunho fica só com os fatos, com
    aviso, e o usuário completa.
  - **Mistura:** a IA escreve a narrativa; arquivos, comandos e testes vêm
    do histórico, com o que a IA acrescentar.
- **Criar** (`handoff_create`): valida (objetivo obrigatório), limita o
  tamanho de cada campo, grava e registra `HANDOFF_CREATED`. O transcript
  da sessão de origem recebe um aviso.
- **Assumir** (`handoff_start`):
  - abre uma sessão no mesmo projeto com o provider e o modelo escolhidos
    (qualquer um, inclusive outro modelo do mesmo provider);
  - o contexto dessa sessão inclui a seção `HANDOFF`, e objetivo +
    próxima ação servem de tarefa para a relevância;
  - a primeira mensagem pede para continuar pela próxima ação;
  - registra `HANDOFF_ACCEPTED` (de, para, provider, modelo) e marca o
    handoff como aceito.

  A nova IA **nunca recebe a conversa anterior**. A sessão de origem
  continua como está; o usuário decide se a encerra.
- **Uso único:** um handoff é aceito uma vez. Para passar adiante de novo,
  cria-se outro a partir da nova sessão.

### 6. Persistência

- Migração 2 do banco:
  - tabela `handoffs` (projeto, sessões de origem e destino, pacote JSON,
    estado, datas);
  - handoffs no índice de busca L3.
- As opções de contexto vão com a sessão (coluna `spec`).

### 7. Comandos Tauri

- **Contexto:** `context_preview` (o que seria enviado para uma tarefa ou
  handoff, sem enviar), `context_settings_get` e `context_settings_save`.
- **Handoff:** `handoff_prepare`, `handoff_create`, `handoff_start`,
  `handoffs_list` e `handoff_get`.

### 8. UI

- **Sessão:**
  - antes da primeira mensagem, a estimativa do contexto, com "Ver" e a
    opção de não anexar;
  - depois, uma linha no transcript com o que foi anexado.
- **Aba Contexto:** seções, tokens, itens, o que ficou de fora, o texto
  exato e o orçamento.
- **Aba Handoff**, aberta a partir da sessão:
  - gerar o rascunho, com ou sem a IA atual, e editar os campos;
  - escolher a IA de destino, com a recomendação do roteador como dica;
  - ver o contexto que ela vai receber;
  - salvar ou passar agora.
- **MEMORY → Trabalho (L1):** handoffs do projeto, pendentes e aceitos.

## Consequências

- Toda sessão começa sabendo o essencial do projeto, pelo custo de um
  orçamento visível e limitado. O resto vem sob demanda pelas ferramentas.
- Trocar de IA não exige a conversa anterior: o pacote tem o que a próxima
  precisa, e o histórico prova os fatos.
- **Instruções maiores:** o contexto fica nas instruções de sistema e vai
  em todas as requisições da sessão. O cache de prompt dos fornecedores e a
  compactação ficam para a Fase 11.
- **Heurística de relevância:** busca por palavras, sem embeddings. Uma
  tarefa vaga recebe pouco contexto específico: fixadas, L1 e Git.
- **Contexto congelado:** o contexto é o do primeiro turno. Numa sessão
  longa, o L1 envelhece e a IA deve consultar as ferramentas.
- **Custo da narrativa:** pedir a narrativa à IA atual custa um turno. Os
  fatos não custam nada.
- **Tasks e agentes (Fase 8)** vão usar o mesmo Context Builder, com a task
  no lugar da primeira mensagem. O handoff automático ao fim de um agente é
  da Fase 8.

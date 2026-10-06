# Tasks

Referência da Fase 8a. Decisão em
[ADR-0014](./adr/0014-task-manager.md); contratos no
`orchestrator-core`, persistência no
[`packages/memory`](../packages/memory/README.md) e regras no
[`packages/orchestrator`](../packages/orchestrator/README.md).

> Toda atividade relevante é uma task. A task guarda o objetivo, o estado e
> a ordem do trabalho — e vira o contexto da IA que trabalha nela.

```text
TASKS (painel)            aba da task                  sessão da task
  por estado        →   editar, estado, dependências  →  contexto montado
  prioridade            arquivos, quem assume            a partir da task
```

## A task

Campos da seção 12 do documento mestre:

| Campo | Conteúdo |
| ----- | -------- |
| `title` | uma linha, obrigatório (até 200 caracteres) |
| `description` | o que precisa ser feito (até 4.000) |
| `status` | `TODO`, `IN_PROGRESS`, `BLOCKED`, `REVIEW`, `DONE`, `CANCELLED` |
| `priority` | `LOW`, `NORMAL`, `HIGH`, `URGENT` |
| `provider` / `model` | a IA escolhida (pelo usuário ou pelo roteador) |
| `agent` | quem executa; o Agent Manager (ADR-0015) põe o agente aqui |
| `parentTask` | subtask de outra task |
| `dependencies` | tasks que precisam terminar antes (até 20) |
| `files` | caminhos relevantes (até 30) |
| `sessions` | sessões abertas para a task |
| `result` | o que a task entregou (até 4.000) |
| datas | `createdAt`, `updatedAt`, `startedAt`, `finishedAt` |

- **Tasks não são apagadas:** são canceladas, como as decisões
  (ADR-0012). O que foi decidido e desfeito continua no histórico.
- Passar de um limite é erro com a razão, nunca corte silencioso.

## Estados

```text
        ┌──────────────── reabrir ─────────────────┐
        ▼                                          │
     TODO ──iniciar──▶ IN_PROGRESS ──revisar──▶ REVIEW ──concluir──▶ DONE
        │                  │  ▲                   │
        │                  │  └─── retomar ───────┘
        └──bloquear──▶ BLOCKED ──desbloquear──▶ TODO
                           │
   qualquer estado ────────┴──cancelar──▶ CANCELLED ──reabrir──▶ TODO
```

| De | Pode ir para |
| -- | ------------ |
| `TODO` | `IN_PROGRESS`, `BLOCKED`, `CANCELLED` |
| `IN_PROGRESS` | `REVIEW`, `DONE`, `BLOCKED`, `TODO`, `CANCELLED` |
| `BLOCKED` | `IN_PROGRESS`, `TODO`, `CANCELLED` |
| `REVIEW` | `DONE`, `IN_PROGRESS`, `CANCELLED` |
| `DONE` | `TODO` (reabrir) |
| `CANCELLED` | `TODO` (reabrir) |

- A UI só mostra os botões dos estados válidos: quem decide é o motor
  (`TaskView.can`), então a tela e a regra não podem divergir.
- **`IN_PROGRESS` exige task liberada:** todas as dependências em `DONE`.
  Tentar antes é recusado com o nome do que falta.
- **`BLOCKED` é sempre do usuário**, para impedimento externo ("esperando
  a chave da API"). Nada desbloqueia sozinho, e um agente não é posto numa
  task bloqueada.
- **`REVIEW` é onde um agente deixa a task** ao chamar `agent.finish`
  (ADR-0015): quem marca `DONE` é o usuário.
- **Reabrir** limpa `finishedAt`. Vindo de `IN_PROGRESS`, voltar para
  `TODO` é "voltar para a fila", não reabrir.

## Dependências e subtasks

- **Dependência** é do mesmo projeto. São recusadas: depender de si mesma,
  dependência repetida, de outro projeto e **ciclo** (verificado por busca
  em profundidade antes de gravar, em qualquer comprimento).
- **Dependência não bloqueia o estado:** a task fica em `TODO` marcada
  como *não liberada* ("espera 2" no painel). "Ainda não posso" e "estou
  impedido" são coisas diferentes.
- **`parentTask`** é hierarquia, não dependência: a mãe mostra o progresso
  das filhas ("2 de 5 subtasks"). Uma task não pode ser sua própria
  ancestral. Concluir a mãe não conclui as filhas.

## A task e o resto do Orchestrator

- **Contexto da task** ([context.md](./context.md)): título, descrição e
  arquivos da task viram a tarefa do Context Builder. É o *task-scoped
  context* da seção 23 do documento mestre.
- **Sessão da task:** abrir uma sessão a partir da task leva esse
  contexto, envia a task como primeira mensagem, guarda a sessão na task e
  a leva para `IN_PROGRESS`. Os arquivos da task entram em
  `RELEVANT FILES` por serem citados no texto.
- **Agentes** ([agents.md](./agents.md)): "Executar com um agente" põe uma
  IA para conduzir a task sozinha. Uma task aceita um agente vivo por vez, e
  os arquivos dela são travados enquanto ele trabalha.
- **Roteador** ([router.md](./router.md)): "Sugerir com o roteador"
  recomenda provider e modelo pelo texto da task, sem gastar tokens.
- **Busca (L3)** ([memory.md](./memory.md)): as tasks entram no índice
  (tipo `task`), então a busca do projeto e o `RELEVANT HISTORY` do
  contexto as encontram.
- **Barra superior:** `TASK` mostra a task da sessão aberta ou a task em
  andamento, como pede a seção 24.

## Eventos

| Evento | Quando | `data` |
| ------ | ------ | ------ |
| `TASK_CREATED` | task criada | `taskId`, `projectId`, `title`, `status`, `priority`, `provider`, `model`, `dependencies` (quantidade) |
| `TASK_STARTED` | entrou em `IN_PROGRESS`, inclusive ao abrir uma sessão para ela | idem, mais `from`/`to` ou `sessionId` |
| `TASK_COMPLETED` | chegou a `DONE` | idem, mais `from`/`to` |
| `TASK_UPDATED` | qualquer outra mudança: edição, prioridade, dependências, `BLOCKED`, `REVIEW`, `CANCELLED`, reabertura | idem |

O texto da descrição e do resultado não vai para o histórico; eles ficam
na task e no índice de busca.

## Persistência (migração 3)

| Tabela | Conteúdo |
| ------ | -------- |
| `tasks` | a task em JSON, com projeto, mãe, estado, prioridade e datas em colunas para ordenar e filtrar |
| `task_dependencies` | `task_id` → `depends_on`, apagadas junto com a task |

As linhas de dependência são a verdade: a task é sempre lida com o que
elas dizem, então as duas não divergem. Um banco da Fase 7 é atualizado ao
abrir, sem perder nada.

## UI

- **Painel TASKS:** "Nova task", busca sem acento, contagem de abertas e
  as tasks agrupadas por estado (em andamento primeiro), com prioridade,
  "espera N", subtasks e provider.
- **Aba da task:** título, descrição, prioridade, os botões dos estados
  válidos, arquivos, dependências, quem assume (com o roteador), as
  sessões e o resultado. "Ver contexto" mostra o que a IA receberia.
- **Agent Board** (seção 25 do documento mestre): as tasks em colunas, com
  o agente de cada uma ([agents.md](./agents.md)).

## IPC

| Comando | Argumentos | Retorno | Histórico |
| ------- | ---------- | ------- | --------- |
| `tasks_list` | `projectId?` (padrão: o aberto) | `TaskView[]` em ordem do painel | — |
| `task_get` | `id` | `TaskView \| null` | — |
| `task_save` | `input: TaskInput` (sem `id` cria) | `Task` | `TASK_CREATED` ou `TASK_UPDATED` |
| `task_status` | `id`, `status` | `Task` | `TASK_STARTED`, `TASK_COMPLETED` ou `TASK_UPDATED` |
| `task_start_session` | `request: { taskId, provider?, model?, budget? }` | `{ task, session, turnId, sendError }` | `SESSION_STARTED`, `TASK_STARTED`, `CONTEXT_BUILT` e o primeiro turno |
| `task_context` | `id` | `ContextPack` | — |

`TaskView` é a task mais o que o motor calcula: `waitingFor` (dependências
pendentes), `subtasks` (`done`/`total`) e `can` (estados válidos agora).

Ver também [ipc.md](./ipc.md#tasks-fase-8a-adr-0014).

## Limitações

- **Sem agente, a task depende de quem a atualiza:** abrir a sessão pela
  task deixa a condução com o usuário, e um estado desatualizado é um
  estado errado. Com agente, o estado anda sozinho (ADR-0015).
- **Dependência protege só o começo:** ela impede iniciar cedo, não impede
  mexer nos arquivos por fora. Quem coordena arquivos é o File Lock
  Manager, e só entre agentes.
- Sem estimativa e sem prazo.

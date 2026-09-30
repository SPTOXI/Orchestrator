# ADR-0014 — Task Manager

- **Estado:** Aceita
- **Fase:** 8a

## Contexto

O documento mestre diz que **toda atividade relevante deve ser
representada por uma task** (seção 12), com estados, dependências,
prioridade, provider, agente, subtasks, arquivos e resultado. O painel
TASKS (seção 24) e o Agent Board (seção 25) mostram esse trabalho.

Hoje o Orchestrator tem tudo o que uma task precisa em volta, mas não a
task:

- sessões de IA, roteador e Conselho (Fases 3–5);
- memória do projeto e histórico durável (Fase 6, ADR-0012);
- contexto por sessão e handoff entre IAs (Fase 7, ADR-0013). O Context
  Builder já monta o contexto a partir de um texto de tarefa — hoje a
  primeira mensagem da sessão.

O que falta é o trabalho ter nome, estado e ordem: sem isso, "o que está
em andamento", "o que depende do quê" e "o que já terminou" só existem na
cabeça do usuário e nas conversas.

A Fase 8 do documento mestre pede Task Manager, Agent Manager, subagentes
e File Locks. A pedido do usuário, ela vai em duas entregas:

- **8a (esta ADR):** o Task Manager e o painel TASKS, com a task ligada ao
  que já existe (contexto, roteador, sessões).
- **8b:** agentes, subagentes, File Lock Manager, execução em paralelo,
  Agent Board completo e handoff automático ao fim de um agente.

## Decisão

### 1. Onde o código fica

- **Contratos** (`Task`, `TaskStatus`, `TaskPriority`, `TaskId`) no
  `orchestrator-core`, como os de sessão e handoff.
- **Persistência** no `orchestrator-memory` (migração 3): a task pertence
  ao projeto, como a memória.
- **Regras** (transições, dependências, contexto, roteador, sessão da
  task) no `orchestrator-engine`, junto do Context Builder e do handoff.
- O app (Tauri) só liga as peças e expõe os comandos.

Nenhum crate novo: a Fase 8a usa os três que já existem.

### 2. A task

Campos da seção 12 do documento mestre:

| Campo | Conteúdo |
| ----- | -------- |
| `id` | `TaskId` (UUID v7) |
| `projectId` | projeto dono da task |
| `title` | uma linha, obrigatório |
| `description` | o que precisa ser feito (texto livre) |
| `status` | `TODO`, `IN_PROGRESS`, `BLOCKED`, `REVIEW`, `DONE`, `CANCELLED` |
| `priority` | `LOW`, `NORMAL`, `HIGH`, `URGENT` |
| `provider` / `model` | a IA escolhida para a task (pelo usuário ou pelo roteador) |
| `agent` | quem executa; sempre vazio na 8a, preenchido na 8b |
| `parentTask` | subtask de outra task |
| `dependencies` | outras tasks que precisam terminar antes |
| `files` | caminhos relevantes (entram no contexto da task) |
| `sessions` | sessões de IA abertas para esta task |
| `result` | o que a task entregou, escrito ao concluir |
| `createdAt` / `updatedAt` / `startedAt` / `finishedAt` | datas |

- **Limites:** título 200 caracteres, descrição e resultado 4.000, até 30
  arquivos e 20 dependências. Passar do limite é erro, não corte
  silencioso.
- **Tasks não são apagadas:** são canceladas, como as decisões (ADR-0012).
  O histórico do projeto não perde o que foi decidido e desfeito.

### 3. Estados e transições

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

- **`IN_PROGRESS` só entra se a task estiver liberada:** todas as
  dependências em `DONE`. Tentar iniciar antes é recusado com o nome do
  que falta — é isso que faz a dependência valer alguma coisa.
- **`BLOCKED` é sempre do usuário**, para impedimento externo
  ("esperando a chave da API"). Sair dele também é do usuário: nada
  desbloqueia sozinho, para não reabrir trabalho que o usuário parou de
  propósito.
- **Dependência não é bloqueio automático:** uma task com dependência
  pendente fica em `TODO`, marcada como *não liberada* na UI. A diferença
  entre "ainda não posso" e "estou impedido" é informação, não ruído.
- **`DONE` e `CANCELLED`** podem ser reabertos (`TODO`), porque na prática
  trabalho volta. Reabrir limpa `finishedAt` e registra o evento.
- Transição inválida é erro com a razão (ex.: "uma task cancelada só pode
  ser reaberta").

### 4. Dependências e subtasks

- **`task_dependencies`** (tabela): a task espera outra do **mesmo
  projeto**.
- **Recusadas:** depender de si mesma, dependência repetida, dependência
  em outro projeto e **ciclo** (verificado por busca em profundidade
  antes de gravar).
- **`parentTask`** é hierarquia, não dependência: a task-mãe mostra o
  progresso das filhas (quantas em `DONE`). Uma task não pode ser sua
  própria ancestral.
- Concluir a mãe não conclui as filhas, nem o contrário: o usuário decide.

### 5. A task e o que já existe

É aqui que a task deixa de ser uma lista e passa a valer:

- **Contexto da task** (ADR-0013): o Context Builder recebe título,
  descrição e os arquivos da task como tarefa, em vez da primeira
  mensagem. É o *task-scoped context* da seção 23 do documento mestre.
- **Sessão da task:** abrir uma sessão a partir da task já leva esse
  contexto, envia a task como primeira mensagem, guarda a sessão na task
  e leva a task para `IN_PROGRESS`.
- **Roteador (ADR-0011):** sugere provider e modelo pelo texto da task,
  sem gastar tokens. O usuário aceita ou escolhe outro.
- **Busca (ADR-0012):** as tasks entram no índice L3 (tipo `task`), então
  a busca do projeto e o `RELEVANT HISTORY` do contexto as encontram.
- **Handoff (ADR-0013):** um handoff criado numa sessão da task guarda a
  task, e a sessão que assume continua na mesma task.

### 6. Persistência (migração 3)

| Tabela | Conteúdo |
| ------ | -------- |
| `tasks` | os campos acima; `project_id` com índice, `status` e `priority` para ordenar |
| `task_dependencies` | `task_id` → `depends_on`, as duas apagadas junto com a task-mãe |

- Um banco da Fase 7 é atualizado ao abrir, sem perder nada.
- `sessions` e `files` são JSON na linha da task: são listas curtas, lidas
  sempre junto com a task.

### 7. Eventos

| Evento | Quando |
| ------ | ------ |
| `TASK_CREATED` | task criada (já previsto na seção 22) |
| `TASK_STARTED` | entrou em `IN_PROGRESS` (idem) |
| `TASK_COMPLETED` | chegou a `DONE` (idem) |
| `TASK_UPDATED` | **novo:** qualquer outra mudança (edição, prioridade, dependências, `BLOCKED`, `REVIEW`, `CANCELLED`, reabertura) |

`TASK_UPDATED` estende a lista da seção 22 pelo mesmo motivo de
`PROCESS_EXITED` na ADR-0005: sem ele, metade das mudanças de estado some
do histórico. Os dados trazem o que mudou (de → para), nunca o texto
inteiro da descrição.

### 8. Comandos Tauri

`tasks_list`, `task_get`, `task_save` (cria e edita), `task_status`,
`task_dependencies_set`, `task_start_session` e `task_context` (prévia do
contexto da task).

### 9. UI

- **Painel TASKS** (barra lateral): contagem por estado, tasks agrupadas
  por estado (`IN_PROGRESS` primeiro), prioridade e provider visíveis,
  marca de *não liberada* para quem espera dependência, campo de busca e
  "Nova task".
- **Aba da task:** título, descrição, estado com os botões das transições
  válidas, prioridade, provider/modelo com a sugestão do roteador,
  dependências, subtasks, arquivos, resultado, as sessões da task e os
  botões "Abrir sessão para esta task" e "Ver contexto".
- **Barra superior:** `TASK` deixa de ser um traço e passa a mostrar a
  task da sessão aberta (ou a task em andamento), como pede a seção 24.
- **Agent Board** (seção 25) fica para a 8b: sem agentes, as colunas são
  as do painel lateral, e o quadro por agente não teria o que mostrar.

## Consequências

- O trabalho passa a ter estado durável e ordem explícita, e o contexto
  de cada IA pode vir da task em vez da conversa.
- **Mais um lugar para manter:** uma task que ninguém atualiza mente.
  Enquanto a execução for manual (8a), o estado depende do usuário; a 8b
  move a task sozinha quando o agente trabalha.
- **Dependência só protege o começo:** ela impede iniciar cedo, não impede
  o usuário de mexer nos arquivos por fora. Coordenação de verdade é o
  File Lock Manager da 8b.
- **Sem execução automática:** nesta fase, iniciar uma task abre uma
  sessão; quem conduz é o usuário. Agente, fila e paralelismo são da 8b.
- A migração 3 é aditiva: quem já usa o app não perde nada.

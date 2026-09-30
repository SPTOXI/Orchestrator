# Agentes, subagentes e travas de arquivo

Referência da Fase 8b. Decisão em
[ADR-0015](./adr/0015-agentes-subagentes-e-file-locks.md); contratos no
`orchestrator-core`, persistência no
[`packages/memory`](../packages/memory/README.md) e regras no
[`packages/agents`](../packages/agents/README.md).

> A IA é substituível. O projeto é permanente. Um agente é temporário: ele
> executa uma task, deixa o resultado na task, o que aprendeu na memória e
> os fatos no histórico — e desaparece.

```text
task liberada  →  agente na fila  →  trava os arquivos  →  sessão da task
                                                              ↓ turnos
   task em revisão  ←  agent.finish  ←  ferramentas auditadas ─┘
```

## O agente

Campos da seção 14 do documento mestre:

| Campo | Conteúdo |
| ----- | -------- |
| `id` | `AgentId` (UUID v7) |
| `task` | a task que ele executa (obrigatória) |
| `provider` / `model` | a IA que ele usa |
| `session` | a sessão em que trabalha; vazia enquanto está na fila |
| `parentAgent` | o agente que o delegou (subagente) |
| `status` | `QUEUED`, `RUNNING`, `DONE`, `FAILED`, `STOPPED` |
| `tools` | ferramentas oferecidas a ele quando começou |
| `context` | o contexto que recebeu: seções e tokens estimados |
| `turns` / `maxTurns` | turnos gastos e o teto dele |
| `files` | caminhos que ele travou |
| `result` | o que entregou (`agent.finish`) |
| `error` | por que falhou |
| `handoff` | handoff criado se parou no meio |
| datas | `createdAt`, `updatedAt`, `startedAt`, `finishedAt` |

- **Agentes não são apagados**, como tasks e decisões: o registro do que uma
  IA fez é do projeto.
- **`DONE`, `FAILED` e `STOPPED` são finais.** Um agente não recomeça; a
  task recomeça, com outro agente.

## Como um agente executa

1. **Fila.** Ele nasce em `QUEUED`. A task precisa estar liberada
   (dependências concluídas) e não pode estar encerrada nem bloqueada, e ela
   só aceita um agente vivo por vez.
2. **Vaga.** Ele começa quando há vaga (`maxParallel`) e os arquivos
   declarados na task estão livres. Enquanto não, o painel diz o motivo
   ("`src/api.ts` está com \"…\"", "2 agentes em execução").
3. **Travas.** Ao começar, trava os arquivos declarados na task.
4. **Sessão.** Abre a sessão da task (ADR-0014): contexto montado a partir
   dela, a task como primeira mensagem, `IN_PROGRESS`.
5. **Turnos.** Cada turno é o laço modelo → ferramentas → modelo dos
   providers por API. Entre turnos, o Orchestrator manda só uma
   continuação: o contexto já foi.
6. **Fim.** `agent.finish` encerra o agente. Sem isso, o teto de turnos
   encerra.
7. **Soltar.** Em qualquer saída as travas caem e a fila anda.

**O agente não conclui a task.** `agent.finish` grava o resultado na task e
a leva para `REVIEW`; quem marca `DONE` é o usuário: o trabalho de uma IA
passa pelos olhos de alguém em qualquer modo de autonomia.

**O que o agente pode fazer é o modo de autonomia** (Fase 9,
[autonomy.md](./autonomy.md)): o do projeto, ou o que o usuário deu a ele
ao pô-lo para executar a task (`autonomy`, herdado pelos subagentes). Em
Assistido, cada ação dele espera a sua autorização e o painel diz
"esperando sua autorização".

**Parou no meio, sai handoff.** `FAILED` (erro ou teto) e `STOPPED`
(usuário) geram um handoff montado pelos fatos da sessão, sem gastar turno
de IA. A task continua `IN_PROGRESS` e outra IA continua de onde parou.
Quem termina bem não gera handoff: o resultado na task é o registro.

## Ferramentas do agente

| Ferramenta | O que faz |
| ---------- | --------- |
| `agent.finish` | encerra o agente com o resultado, que vai para a task (até 4.000 caracteres) |
| `agent.delegate` | cria uma subtask da task do agente e enfileira um subagente para ela |

São ferramentas como as outras: auditadas com `TOOL_CALLED` e só existem
dentro de uma sessão conduzida por um agente.

- **Profundidade 2** (agente → subagente) e **até 5 subagentes por agente**.
- O subagente entra na mesma fila e no mesmo teto: delegar não fura a fila.
- O pai **não espera** o filho; a subtask aparece como progresso na
  task-mãe.

## File Lock Manager

- **Granularidade: o arquivo**, pelo caminho relativo ao projeto.
- **Só agentes travam e só agentes são travados.** O usuário nunca é
  bloqueado no próprio projeto; a UI mostra as travas para ele saber onde
  alguém está trabalhando.
- **Tomada** ao iniciar o agente (arquivos declarados na task) e na hora em
  que ele escreve num arquivo que ninguém declarou.
- **Vale para** `filesystem.write`, `filesystem.move` (origem e destino) e
  `filesystem.delete`. **Leitura nunca trava.**
- **Solta** quando o agente termina, de qualquer maneira. Ao abrir o app,
  travas de agentes que não estão mais rodando são liberadas.
- **Negada:** a ferramenta é recusada com `LOCKED` e o motivo
  ("`src/api.ts` está com o agente \"…\""). Sem espera e sem deadlock: a IA
  recebe um erro que entende e decide o que fazer.

A exclusão é do banco: a chave `(projeto, caminho)` faz de tomar uma trava
um `INSERT` que ou grava, ou diz quem já tem.

## Limites e controles

`<app-data>/agents.json`:

| Ajuste | Padrão | Faixa |
| ------ | ------ | ----- |
| `maxParallel` | 2 | 1–8 |
| `maxTurns` | 12 | 1–50 |

Controles do usuário (seção 11 do documento mestre), no painel AGENTS e na
aba da task:

- **Parar** um agente (`Cancel`): cancela o turno, encerra como `STOPPED` e
  deixa handoff.
- **Parar todos** (`Stop All Agents`): o mesmo para todos os agentes vivos
  do projeto, inclusive os da fila.
- **Pausar / Retomar** um agente (`Pause`, Fase 9): ele para na próxima
  chamada de ferramenta ou no próximo turno, mantendo a vaga e as travas.
- **Pausar IAs / Retomar IAs:** o mesmo para todas as IAs, e a fila não
  inicia ninguém enquanto isso.

O teto de turnos **não é política de permissão**: diz quanto um agente
roda, não o que ele pode fazer (isso é o modo de autonomia). É visível,
configurável e igual em todos os modos.

## Eventos

| Evento | Quando | `data` |
| ------ | ------ | ------ |
| `AGENT_STARTED` | o agente saiu da fila e abriu a sessão | `agentId`, `projectId`, `taskId`, `title`, `provider`, `model`, `status`, `parentAgent`, `sessionId`, `files` |
| `AGENT_FINISHED` | o agente encerrou | idem, mais `outcome`, `turns`, `result` (cortado), `error`, `handoffId` |

A Fase 8b **não acrescenta tipo de evento**: os dois já existiam desde a
Fase 2. Travas negadas aparecem como `TOOL_CALLED` com `ok: false`, e as
mudanças da task, como `TASK_*`.

## Persistência (migração 4)

| Tabela | Conteúdo |
| ------ | -------- |
| `agents` | o agente em JSON, com projeto, task, sessão, pai, estado e datas em colunas |
| `file_locks` | `(project_id, path)` como chave: um dono por arquivo, com agente, task e quando foi tomada |

Um banco da Fase 8a é atualizado ao abrir, sem perder nada.

## UI

- **Painel AGENTS:** agentes por estado, com provider, progresso, motivo da
  espera, arquivos travados, "Parar", "Parar todos" e os dois limites.
- **Agent Board** (seção 25): colunas `A fazer`, `Em andamento` e
  `Em revisão`, cartões com task, agente, provider, estado e progresso.
- **Aba da task:** "Executar com um agente" e a lista dos agentes que já
  trabalharam nela.
- **Barra superior:** o chip `AGENT` mostra o agente da sessão aberta, ou
  quantos estão em execução e na fila.

## IPC

| Comando | Argumentos | Retorno | Histórico |
| ------- | ---------- | ------- | --------- |
| `agents_list` | `projectId?` (padrão: o aberto) | `AgentView[]` em ordem do painel | — |
| `agent_get` | `id` | `AgentView \| null` | — |
| `agent_start` | `request: { taskId, provider?, model?, maxTurns? }` | `Agent` (em `QUEUED`) | `AGENT_STARTED` quando começa |
| `agent_stop` | `id` | `Agent` | `AGENT_FINISHED` |
| `agents_stop_all` | `projectId?` | quantidade | `AGENT_FINISHED` de cada um |
| `agent_locks` | `projectId?` | `FileLock[]` | — |
| `agent_settings_get` / `agent_settings_save` | — / `settings` | `AgentSettings` | — |

`AgentView` é o agente mais o que o serviço calcula: `waiting` (por que
está na fila), `taskTitle` e `taskStatus`.

Ver também [ipc.md](./ipc.md#agentes-fase-8b-adr-0015) e
[tasks.md](./tasks.md).

## Limitações

- **Custo é real:** cada turno é uma chamada paga ao provider. O teto de
  turnos protege, mas um agente caro continua caro.
- **Um pedido de autorização sem resposta segura o agente** (e a vaga) até
  o usuário decidir.
- **Pausar não interrompe o modelo:** a resposta que ele já estava gerando
  termina; o que ele pedir para fazer espera.
- **A trava protege os arquivos, não o repositório:** dois agentes ainda
  podem rodar comandos que mexem no mesmo estado (`git`, builds). Nada
  serializa comandos.
- **O agente conduz uma sessão só.** Ele não troca de provider no meio nem
  retoma a sessão de outro agente.

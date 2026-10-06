# ADR-0015 — Agentes, subagentes e File Locks

- **Estado:** Aceita
- **Fase:** 8b

## Contexto

O documento mestre pede, na seção 14, que o trabalho seja executado por
**agentes temporários**:

```text
Agent
├── id · provider · task · session
├── status · tools · context
└── result
```

e que um agente possa executar a tarefa, criar subagentes, delegar
subtarefas, consultar a memória, usar ferramentas, produzir resultado e
gerar handoff. A seção 13 acrescenta que agentes podem trabalhar em
paralelo **quando não houver conflito** e exige um `File Lock Manager` (ou
mecanismo equivalente) para que dois agentes não alterem os mesmos
arquivos ao mesmo tempo. A seção 25 pede o Agent Board, e a 11, os
controles `Pause`, `Cancel` e `Stop All Agents`.

O que já existe depois da Fase 8a:

- **tasks** com estado, prioridade, dependências, subtasks e arquivos, e a
  sessão aberta a partir da task com o contexto montado dela
  (ADR-0014);
- **sessões e subagentes** no `SessionManager` (`spawn`, ADR-0009), com
  cancelamento, transcrição e retomada;
- **ferramentas** pelo Tool Runtime, sempre auditadas, e as ferramentas de
  memória para as IAs (ADR-0013);
- **handoff** entre IAs a partir dos fatos da sessão (ADR-0013);
- `AGENT_STARTED` e `AGENT_FINISHED` já reservados no `EventKind` desde a
  Fase 2.

O que falta é quem **conduz**: hoje o usuário abre a sessão da task e
dirige a conversa turno a turno. Nada executa uma task sozinho, nada
enfileira trabalho, e nada impede duas sessões de escreverem no mesmo
arquivo ao mesmo tempo.

Esta ADR decide o Agent Manager, os subagentes e o File Lock Manager. O
**gate de autonomia** (Assistido, Autônomo, Acesso Irrestrito) é a Fase 9:
até lá, o que limita um agente é o que está descrito aqui — teto de turnos,
teto de paralelismo e os controles do usuário — e não uma política de
permissões.

## Decisão

### 1. Onde o código fica

O `packages/agents` deixa de ser um README e vira o crate
**`orchestrator-agents`**, como a ADR-0001 previu:

```text
orchestrator-agents
├── service.rs   Agent Manager: fila, execução, parada, subagentes
├── locks.rs     File Lock Manager
├── tools.rs     ferramentas do agente + trava dos arquivos
└── settings.rs  agents.json (paralelismo e teto de turnos)
```

- **Contratos** (`Agent`, `AgentStatus`, `AgentId`, `FileLock`) no
  `orchestrator-core`, como os de task e sessão.
- **Persistência** no `orchestrator-memory` (migração 4): agentes e travas
  pertencem ao projeto.
- **Regras** no crate novo, que depende do `orchestrator-engine`
  (Context Builder, `TaskService`, `HandoffService`), do `providers`
  (`SessionManager`) e do `memory`.
- O app (Tauri) liga as peças e expõe os comandos.

O crate novo em vez de mais um módulo no `orchestrator-engine` porque o
motor já tem contexto, handoff e tasks, e porque a execução autônoma é a
parte que mais vai crescer (Fases 9 e 11).

### 2. O agente

| Campo | Conteúdo |
| ----- | -------- |
| `id` | `AgentId` (UUID v7) |
| `projectId` | projeto dono do agente |
| `task` | a task que ele executa (obrigatória: agente sem task não existe) |
| `provider` / `model` | a IA que o agente usa |
| `session` | a sessão em que ele trabalha; `null` enquanto está na fila |
| `parentAgent` | o agente que o delegou (subagente) |
| `status` | `QUEUED`, `RUNNING`, `DONE`, `FAILED`, `STOPPED` |
| `tools` | nomes das ferramentas oferecidas a ele quando começou |
| `context` | o que ele recebeu de contexto: seções e tokens estimados |
| `turns` / `maxTurns` | turnos gastos e o teto dele |
| `files` | caminhos que ele travou |
| `result` | o que ele entregou (`agent.finish`) |
| `error` | por que falhou, quando falhou |
| `handoff` | handoff criado quando ele parou no meio |
| datas | `createdAt`, `updatedAt`, `startedAt`, `finishedAt` |

- **Agentes são descartáveis; a task e o projeto não.** Nada que o
  Orchestrator precise depois mora só no agente: o resultado vai para a
  task, o conhecimento para a memória, os fatos para o histórico.
- **Agentes não são apagados**, como tasks e decisões: o registro do que
  uma IA fez é do projeto.

Estados, e o que move cada um:

```text
       ┌──── vaga livre e arquivos livres ────┐
QUEUED ─┤                                      ├─▶ RUNNING ──agent.finish──▶ DONE
       └──── parar ──▶ STOPPED                 │
                                               ├── erro / teto de turnos ──▶ FAILED
                                               └── parar (usuário) ────────▶ STOPPED
```

`DONE`, `FAILED` e `STOPPED` são finais: um agente não recomeça. O que
recomeça é a task, com **outro** agente — o princípio do documento mestre.

### 3. Como um agente executa

1. **Enfileirar.** `agent_start(taskId, provider?, model?)` cria o agente
   em `QUEUED`. A task precisa estar liberada (dependências concluídas,
   ADR-0014) e não pode estar em estado final.
2. **Escalonar.** O Agent Manager inicia o agente mais antigo da fila
   quando há vaga (teto de paralelismo) **e** os arquivos declarados na
   task estão livres. Sem vaga ou com arquivo travado, ele fica na fila com
   o motivo visível ("espera o agente X").
3. **Travar.** Ao começar, o agente trava os arquivos declarados na task.
4. **Abrir a sessão.** Sessão da task (ADR-0014): contexto montado a
   partir dela, título da task, provider/modelo do agente.
5. **Trabalhar.** Turno a turno (`execute`), cada turno já é o laço
   modelo → ferramentas → modelo dos providers por API (ADR-0010). Entre
   turnos o Orchestrator manda uma continuação curta, não um resumo: o
   contexto já foi para a sessão.
6. **Terminar.** O agente chama `agent.finish` com o resultado. Sem isso,
   ele para no teto de turnos.
7. **Soltar.** Em qualquer saída, as travas caem e a fila anda.

**O agente não conclui a task.** `agent.finish` leva a task para
`REVIEW` com o resultado do agente; quem marca `DONE` é o usuário. Enquanto
não existe o gate de autonomia (Fase 9), o trabalho de uma IA passa pelos
olhos de alguém antes de virar "pronto" — e `REVIEW` é exatamente a coluna
que o Agent Board pede.

**Parou no meio (`FAILED` ou `STOPPED`): sai um handoff automático.** O
motor monta o pacote pelos fatos da sessão (ADR-0013, sem gastar um turno
de IA) e registra o resultado parcial e o erro. A task continua
`IN_PROGRESS` e o handoff fica ligado a ela: outra IA continua de onde
parou, que é a razão de o handoff existir. Um agente que termina bem não
gera handoff — o resultado na task já é o registro.

### 4. Subagentes e delegação

A ferramenta `agent.delegate` cria uma **subtask** da task do agente (com
título, descrição e arquivos) e enfileira um subagente para ela.

- O subagente nasce com `parentAgent` preenchido e, quando o provider é o
  mesmo, a sessão é `spawn` da sessão do pai (ADR-0009), então o provider
  sabe que é um subagente dele.
- **Profundidade máxima 2** (agente → subagente) e **até 5 subagentes por
  agente**. Decompor é útil; uma árvore infinita de IAs gastando tokens não
  é.
- O subagente entra na mesma fila e no mesmo teto de paralelismo dos
  outros: delegar não fura a fila.
- O pai **não espera** o filho. Ele continua ou termina; a subtask aparece
  na task-mãe como progresso ("2 de 5 subtasks", ADR-0014).

### 5. File Lock Manager

- **Granularidade: o arquivo**, pelo caminho relativo ao projeto. Diretório
  inteiro seria bloqueio demais para o que a seção 13 pede; linha seria
  precisão que o Tool Runtime não tem.
- **Só agentes travam e só agentes são travados.** O usuário nunca é
  bloqueado pelo Orchestrator — o projeto é dele (regra de ouro do
  documento mestre). A UI mostra as travas para ele saber onde alguém está
  trabalhando.
- **Quando a trava é tomada:** ao iniciar o agente, para os arquivos
  declarados na task; e na hora em que o agente escreve em um arquivo que
  ninguém declarou (`filesystem.write`, `filesystem.move` — origem e
  destino — e `filesystem.delete`).
- **Quando a trava cai:** quando o agente termina, de qualquer maneira. Na
  abertura do app, travas de agentes que não estão mais rodando são
  liberadas: o app fechado no meio não deixa o projeto trancado.
- **Trava negada = ferramenta recusada,** com o motivo ("`src/api.ts` está
  com o agente <título> (task <título>)"). Sem espera, sem fila de I/O,
  sem deadlock: a IA recebe um erro que ela entende e decide o que fazer.
  A recusa já é auditada como qualquer ferramenta que falha
  (`TOOL_CALLED`, `ok: false`).
- **Leitura nunca é travada.** Duas IAs lendo o mesmo arquivo não é
  conflito.

A trava é verificada no caminho por onde todas as chamadas passam: o
executor de ferramentas do crate de agentes envolve o do motor
(`RuntimeTools` → `EngineTools` → `AgentTools`), então nenhuma ferramenta
escapa e o Tool Runtime não precisa saber que agentes existem.

### 6. Limites e controles

`<app-data>/agents.json`, como `context.json` e `council.json`:

| Ajuste | Padrão | Faixa | Para quê |
| ------ | ------ | ----- | -------- |
| `maxParallel` | 2 | 1–8 | agentes rodando ao mesmo tempo |
| `maxTurns` | 12 | 1–50 | turnos de um agente antes de parar sozinho |

Controles da seção 11, do usuário, não do Orchestrator:

- **Parar agente** (`Cancel`): cancela o turno e encerra o agente como
  `STOPPED`, com handoff.
- **Parar todos** (`Stop All Agents`): o mesmo para todos os agentes vivos,
  inclusive os da fila.
- `Pause` fica para a Fase 9, junto do gate de autonomia: pausar sem um
  modelo de retomada seria um botão que mente.

O teto de turnos **não é uma política de permissão**: é o que impede um
agente de rodar para sempre enquanto não existe o gate da Fase 9. Ele é
visível, configurável e some do caminho quando o usuário o aumenta.

### 7. Persistência (migração 4)

| Tabela | Conteúdo |
| ------ | -------- |
| `agents` | o agente em JSON, com projeto, task, sessão, pai, estado e datas em colunas para listar e filtrar |
| `file_locks` | `(project_id, path)` como chave: um dono por arquivo, com agente, task e quando foi tomada |

A chave primária de `file_locks` é o que garante a exclusão mútua: tomar
uma trava é um `INSERT` que ou grava, ou diz quem já tem. Não existe
"verificar e depois gravar".

### 8. Histórico

`AGENT_STARTED` e `AGENT_FINISHED` já existem no `EventKind` desde a Fase 2
e bastam: o começo, o fim e o desfecho (`DONE`, `FAILED`, `STOPPED`, com
resultado, erro, turnos e arquivos travados). Diferente da Fase 8a, esta
fase **não acrescenta nenhum tipo de evento** — as travas negadas já
aparecem como `TOOL_CALLED` com `ok: false`, e as mudanças de task, como
`TASK_*`.

### 9. Interface

- **Painel AGENTS** (hoje um placeholder): agentes rodando e na fila, com
  provider, task, turnos gastos, motivo da espera, os arquivos travados do
  projeto, "Parar" e "Parar todos", mais os dois ajustes.
- **Agent Board** (seção 25) como aba: colunas `A FAZER`, `EM ANDAMENTO` e
  `EM REVISÃO`, cartões com task, agente, provider, estado e progresso
  (turnos e subtasks).
- **Aba da task:** "Executar com um agente" ao lado de "Abrir sessão", e a
  lista dos agentes que já trabalharam nela.
- **Barra superior:** o chip `AGENT`, que era um traço, mostra o agente da
  sessão aberta ou quantos estão rodando (seção 24).

## Consequências

**Ganhos**

- Uma task é executada sem o usuário conduzir turno a turno, e mais de uma
  ao mesmo tempo quando não há conflito — o que as seções 13 e 14 pedem.
- Dois agentes não alteram o mesmo arquivo: a trava é do banco, não da
  boa vontade das IAs.
- Um agente que morre no meio não leva o trabalho junto: sai um handoff, e
  outra IA continua.
- O agente é descartável de verdade — tudo o que ele produziu está na
  task, na memória e no histórico.

**Custos e riscos**

- **Um agente trabalha sozinho, e a Fase 9 é quem traz o gate.** Até lá o
  que existe é o teto de turnos, o teto de paralelismo, as travas e os
  botões de parar. Quem liga um agente num projeto real precisa saber
  disso; a UI diz.
- **Custo de tokens é real:** cada turno é uma chamada paga. O teto de
  turnos protege, o painel mostra o gasto da sessão, mas um agente caro é
  um agente caro.
- Recusar a trava em vez de esperar faz a IA encontrar erro em vez de
  bloquear. É a troca certa (não trava o sistema), mas exige que o erro
  diga o que está acontecendo — e ele diz.
- O teto de turnos interrompe trabalho legítimo de vez em quando. É
  configurável e o handoff torna a interrupção recuperável.
- Mais um crate para compilar e testar em três sistemas operacionais.

**Fica para depois**

- Gate de autonomia, `Pause` e políticas por ferramenta (Fase 9).
- Escalonamento por custo/afinidade e cache entre agentes (Fase 11).
- Reaproveitar uma sessão encerrada como novo agente (hoje cada agente
  abre a sua).

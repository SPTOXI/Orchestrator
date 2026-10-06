# packages/agents

**Agent Manager, subagentes e File Lock Manager** — crate
`orchestrator-agents` (Fase 8b,
[ADR-0015](../../docs/adr/0015-agentes-subagentes-e-file-locks.md); pausa e
modo por agente na Fase 9,
[ADR-0016](../../docs/adr/0016-autonomia-e-pause.md)).
Referência: [`docs/agents.md`](../../docs/agents.md).

| Módulo | Conteúdo |
| ------ | -------- |
| `service.rs` | `AgentService`: fila, execução turno a turno, teto de turnos, pausar e retomar (um e todos), parar (um e todos), delegação, o modo de autonomia dado ao agente (herdado pelos subagentes), handoff automático de quem parou no meio, e `AgentView` (agente + o que o painel calcula: espera, pausa, pedido pendente, modo) |
| `locks.rs` | `LockManager`: trava por arquivo, tomada ao iniciar e ao escrever, solta quando o agente termina |
| `tools.rs` | `AgentTools`: `agent.finish` e `agent.delegate` para as IAs, a verificação das travas no caminho de toda chamada de ferramenta, e o `TOOL_CALLED` do que ele mesmo responde |
| `settings.rs` | `AgentSettings` (`maxParallel`, `maxTurns`) em `<app-data>/agents.json` |

Depende de `orchestrator-core`, `orchestrator-engine`,
`orchestrator-providers` e `orchestrator-memory`. O app (`src-tauri`) só
liga as peças: envolve o executor de ferramentas com o `AgentTools` (e este
com o `AutonomyGate` do motor), cria o `AgentService` e expõe os comandos.

Regras:

- **Um agente é uma task, uma sessão e um desfecho.** Ele não recomeça: o
  que recomeça é a task, com outro agente.
- **O agente não conclui a task:** `agent.finish` leva a task para revisão,
  e quem marca concluída é o usuário.
- **Quem para no meio deixa handoff** montado pelos fatos da sessão, sem
  gastar um turno de IA.
- **Só agentes travam e só agentes são travados.** O usuário nunca é
  bloqueado no próprio projeto, e leitura nunca trava.
- **Trava negada recusa a ferramenta com o motivo** — sem espera, sem
  deadlock.
- O que um agente pode fazer é o modo de autonomia (gate do motor); o teto
  de turnos diz quanto ele roda.
- **Pausado não é um estado gravado:** o agente continua `RUNNING`, com a
  vaga e as travas, e espera antes do próximo turno e no gate.

| Próximas responsabilidades | Fase |
| -------------------------- | ---- |
| Agent scheduling por custo e afinidade, cache entre agentes | 11 |

Testes: `cargo test -p orchestrator-agents`. Os unitários cobrem os
schemas das ferramentas, quais ferramentas a trava vigia, a normalização
dos caminhos e a configuração. Os de integração (`tests/agents.rs`, com o
Tool Runtime real, um repositório Git temporário e um provider roteirizado)
cobrem:

- um agente executando a task de ponta a ponta, com contexto, resultado e
  a task em revisão;
- o teto de turnos parando o agente e deixando handoff;
- dois agentes e o mesmo arquivo: a fila espera, o usuário para o primeiro
  e o segundo anda;
- a escrita recusada com `LOCKED` no meio de um turno;
- delegação: subtask criada e subagente executando depois do pai, com o
  modo herdado;
- um agente em Assistido esperando a autorização e terminando depois dela;
- parar um agente cancela o pedido que ele esperava;
- pausar um agente segura o próximo turno até retomar;
- com as IAs pausadas, a fila não inicia ninguém.

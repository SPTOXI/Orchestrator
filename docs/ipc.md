# Camada IPC (React ↔ Tauri ↔ Rust)

```text
React (apps/desktop/src)
   │  invoke(...)                       ▲ listen("runtime://stream" | "runtime://audit")
   ▼                                    │
Tauri (apps/desktop/src-tauri) ── ponte, sem lógica de domínio
   │  ToolRuntime::invoke(ToolCall)     ▲ EventSink (DesktopSink)
   │  SessionManager (providers)        │
   ▼                                    │
Rust Runtime (packages/runtime) ──▶ Orchestrator Core (packages/core)
Provider Layer (packages/providers) ──tool_call──▶ ToolRuntime::invoke
```

Decisões registradas em [ADR-0003](./adr/0003-gateway-ipc-unico.md),
[ADR-0009](./adr/0009-camada-de-providers-e-sessoes.md),
[ADR-0010](./adr/0010-providers-por-api-com-cadastro-livre.md) e
[ADR-0013](./adr/0013-context-builder-e-handoff.md) e
[ADR-0014](./adr/0014-task-manager.md).

## Comandos

| Comando | Argumentos | Retorno | Auditado |
| ------- | ---------- | ------- | -------- |
| `runtime_invoke` | `tool: string`, `args?: object` | `ToolResult` | sim (`TOOL_CALLED` + eventos de domínio) |
| `runtime_tools` | — | `ToolSpec[]` | não (somente leitura do catálogo) |
| `terminal_input` | `id: string`, `data: string` | `void` | não por tecla (ADR-0003) |
| `terminal_resize` | `id: string`, `cols: number`, `rows: number` | `void` | não |
| `history_recent` | `limit?: number` (padrão 200) | `AuditEvent[]` (mais antigo primeiro), lidos do banco | não |
| `app_info` | — | `{ version, os, arch, baseDir, dataDir, database, databaseWarning, defaultShell }` (`baseDir`: projeto aberto ou pasta do usuário; `database`: caminho do banco ou `(memória)`) | não |
| `pick_folder` | — | `string \| null` — seletor nativo de pasta; só interação de UI, a pasta é aberta depois com `project.open` (auditado) | não |

`runtime_invoke` sempre resolve com um `ToolResult` (falhas vêm em
`ok: false` + `error`); a UI sempre chama com `origin = user`. O cliente
tipado está em `apps/desktop/src/lib/runtime.ts`.

### Providers e sessões (Fase 3, ADR-0009)

Não são ferramentas do sistema operacional, então não passam por
`runtime_invoke`: cada comando repassa ao `ProviderRegistry` /
`SessionManager`, que gravam o histórico. Erros rejeitam a promessa com
`{ kind, message }` (`ProviderCallError` no cliente). Referência completa:
[`providers.md`](./providers.md).

| Comando | Argumentos | Retorno | Histórico |
| ------- | ---------- | ------- | --------- |
| `providers_list` | — | `{ providers: ProviderInfo[], active }` | — |
| `provider_inspect` | `id` | `ProviderStatus` | — |
| `provider_select` | `id` | `{ providers, active }` | `PROVIDER_SWITCHED` |
| `sessions_list` | — | `SessionInfo[]` (mais nova primeiro) | — |
| `session_start` | `request?: { provider?, title?, model?, instructions?, context? }` | `SessionInfo` (no projeto aberto) | `SESSION_STARTED` |
| `session_get` | `id` | `{ info, entries, lastSeq, truncated }` | — |
| `session_send` | `id`, `input` | `turnId`; o progresso chega por eventos | `TURN_COMPLETED` ao terminar |
| `session_cancel` | `id` | `SessionInfo` | `TURN_COMPLETED` (`cancelled`) |
| `session_close` | `id` | `SessionInfo` | `SESSION_CLOSED` |
| `session_resume` | `id` | `SessionInfo` | `SESSION_RESUMED` |
| `session_spawn` | `parentId`, `request?` | `SessionInfo` do subagente | `SESSION_STARTED` com `parentSessionId` |
| `session_context_get` | `id` | `ContextOptions` (`{ enabled?, budget?, handoffId? }`) | — |
| `session_context_set` | `id`, `options: ContextOptions` | `ContextOptions`; recusado depois do primeiro turno (o contexto já foi) | — |

Ferramentas pedidas pelo provider passam pelo mesmo `ToolRuntime::invoke` e
geram `TOOL_CALLED` com `origin = agent { agentId, sessionId, provider }`.
Desde a Fase 7, as ferramentas de memória (`memory.*`, `decision.*`) são
atendidas pelo `EngineTools` antes do runtime, com o mesmo registro.

No primeiro turno de cada sessão, o `SessionManager` anexa o contexto do
projeto (se ligado) e grava `CONTEXT_BUILT` com origem `system`.

### Conexões de API (Fase 4, ADR-0010)

Cadastro das APIs que viram providers. Mesmo formato de erro
(`{ kind, message }`). Referência: [`api-connections.md`](./api-connections.md).

| Comando | Argumentos | Retorno | Histórico |
| ------- | ---------- | ------- | --------- |
| `connections_list` | — | `{ connections: ConnectionView[], presets, vault, warnings }` | — |
| `connection_save` | `request: { connection, apiKey?, clearKey?, previousId? }` | `ConnectionView` | `CONNECTION_SAVED` (e `PROVIDER_SWITCHED` se o ativo sair) |
| `connection_delete` | `id` | `void` | `CONNECTION_REMOVED` |
| `connection_test` | `request: { connection, apiKey?, model? }` | `TestReport` | — |
| `connection_models` | `request: { connection, apiKey? }` | `ModelEntry[]` descobertos | — |

- `ConnectionView = { connection, key: { source, present, detail } }`: a
  chave **nunca** volta para a webview, só se ela existe.
- `apiKey` vai direto para o cofre do sistema (`connection_save`) ou é usada
  só naquela chamada (`connection_test`, `connection_models`), para testar
  uma configuração antes de salvar.
- `previousId` indica a conexão editada quando o id muda (renomear).

### Roteador e Conselho (Fase 5, ADR-0011)

Escolha do modelo de cada tarefa. Mesmo formato de erro. Referência:
[`router.md`](./router.md).

| Comando | Argumentos | Retorno | Histórico |
| ------- | ---------- | ------- | --------- |
| `router_recommend` | `request: { task, activity?, preference?, needsTools?, minContext? }` | `Recommendation` (ranking + excluídos; sem tokens) | — |
| `council_get` | — | `{ settings, activities, maxMembers, warning }` | — |
| `council_save` | `settings: CouncilSettings` | `CouncilSettings` | `COUNCIL_CONFIGURED` |
| `council_run` | `request: { …router_recommend, force? }` | `{ deliberation, started, startError }`. Com o Conselho ligado, os membros analisam e um escreve o plano; no Full, `started` traz a sessão aberta pelo 1º da fila (ADR-0024) | `COUNCIL_DELIBERATED`; no Full também `SESSION_STARTED`, `ROUTE_DECIDED` e `TURN_COMPLETED` com `origin = council` |
| `council_execute` | `deliberationId` | `{ session, turnId, sendError, skipped }`: o 1º da fila que consegue abre a sessão, com os outros membros como reservas, e recebe a demanda e o plano | `SESSION_STARTED`, `ROUTE_DECIDED` (com `reserves`) e o turno; se o turno falhar, `SESSION_FAILOVER` |
| `council_history` | — | `Deliberation[]` (últimas 50) | — |
| `route_start_session` | `request: { deliberationId?, provider, model?, title?, task?, sendTask? }` | `{ session, turnId, sendError, skipped: [] }` — modelo escolhido à mão | `SESSION_STARTED`, `ROUTE_DECIDED` (e o turno da tarefa) |

### Histórico, projetos e memória (Fase 6, ADR-0012)

Leem e escrevem o banco local. Erros rejeitam a promessa com uma mensagem.
As escritas têm origem `user`. Referência: [`memory.md`](./memory.md).

| Comando | Argumentos | Retorno | Histórico |
| ------- | ---------- | ------- | --------- |
| `history_query` | `query?: { projectId?, kinds?, text?, hideReads?, before?, limit? }` | `{ events, next }`: página mais antigo primeiro; `next` é o cursor para `before` | — |
| `projects_recent` | `limit?` (padrão 8) | `Project[]` (último aberto primeiro) | — |
| `project_current` | — | `Project \| null` | — |
| `project_forget` | `id` | `void` (sai da lista; memória e histórico ficam) | — |
| `projects_import_recent` | `list: { path, name, openedAt }[]` | quantos entraram | — |
| `projects_open` | — | `Project[]`: os abertos lado a lado, na ordem da barra lateral (ADR-0023) | — |
| `project_close` | `id` | `Project \| null`: o próximo a mostrar; o projeto sai da barra lateral, nada é apagado | — |
| `projects_reorder` | `ids` | `void` | — |
| `project_links` | `id` | `ProjectLink[]` (`project`, `note`, `createdAt`): os relacionados | — |
| `project_link` | `id`, `other`, `note?` | `ProjectLink[]` de `id`; relaciona ou muda a nota | `PROJECT_LINKED` |
| `project_unlink` | `id`, `other` | `ProjectLink[]` de `id` | `PROJECT_LINKED` (`linked: false`) |
| `memory_overview` | `projectId?` (padrão: o aberto) | `MemoryOverview \| null`: L1 e contagens | — |
| `memory_list` | `projectId` | `MemoryEntry[]` (fixadas primeiro) | — |
| `memory_save` | `input: { id?, projectId, kind, title, content, tags, pinned }` | `MemoryEntry` | `MEMORY_SAVED` |
| `memory_delete` | `id` | `void` | `MEMORY_REMOVED` |
| `memory_search` | `projectId`, `text`, `limit?` (padrão 30) | `SearchHit[]` (L3, por relevância) | — |
| `decisions_list` | `projectId` | `ProjectDecision[]` | — |
| `decision_save` | `input: { id?, projectId, title, context, decision, consequences, status }` | `ProjectDecision` | `DECISION_SAVED` |

`council_history` também passa a vir do banco: as deliberações e o cache do
Conselho sobrevivem ao reinício.

### Contexto e handoff (Fase 7, ADR-0013)

O Context Builder e o handoff entre IAs. Os comandos de contexto rejeitam
com uma mensagem; os de handoff, com `{ kind, message }`. As escritas têm
origem `user`. Referência: [`context.md`](./context.md).

| Comando | Argumentos | Retorno | Histórico |
| ------- | ---------- | ------- | --------- |
| `context_preview` | `request?: { projectPath?, task?, handoffId?, budget?, sessionId? }` | `ContextPack` (seções, tokens, o que ficou de fora, texto exato); nada é enviado | — |
| `context_settings_get` | — | `{ settings: { autoAttach, budgetTokens }, minBudget, maxBudget, defaultBudget, warning }` | — |
| `context_settings_save` | `settings` | `ContextSettings` (grava `context.json`) | — |
| `handoff_prepare` | `request: { sessionId, askAgent? }` (padrão `true`) | `HandoffDraft` (`{ from, projectPath, packet, byAgent, notes, usage }`); nada é gravado | com `askAgent`, o turno da IA na sessão de origem (`TURN_COMPLETED`, origem `system`) |
| `handoff_create` | `request: { sessionId, packet, byAgent? }` | `Handoff` | `HANDOFF_CREATED` |
| `handoff_start` | `request: { handoffId, provider, model?, title?, budget? }` | `{ handoff, session, turnId, sendError }` | `SESSION_STARTED`, `HANDOFF_ACCEPTED`, `CONTEXT_BUILT` e o primeiro turno |
| `handoffs_list` | `projectId?` (sem ele, todos) | `Handoff[]` (mais novo primeiro, até 50) | — |
| `handoff_get` | `id` | `Handoff \| null` | — |

### Tasks (Fase 8a, ADR-0014)

O Task Manager. Mesmo formato de erro dos providers (`{ kind, message }`);
as escritas têm origem `user`. Referência: [`tasks.md`](./tasks.md).

| Comando | Argumentos | Retorno | Histórico |
| ------- | ---------- | ------- | --------- |
| `tasks_list` | `projectId?` (padrão: o projeto aberto) | `TaskView[]`, em ordem do painel | — |
| `task_get` | `id` | `TaskView \| null` | — |
| `task_save` | `input: TaskInput` (sem `id`, cria) | `Task` | `TASK_CREATED` ou `TASK_UPDATED` |
| `task_status` | `id`, `status` | `Task`; recusa transição inválida ou task não liberada | `TASK_STARTED`, `TASK_COMPLETED` ou `TASK_UPDATED` |
| `task_start_session` | `request: { taskId, provider?, model?, budget? }` | `{ task, session, turnId, sendError }` | `SESSION_STARTED`, `TASK_STARTED`, `CONTEXT_BUILT` e o primeiro turno |
| `task_context` | `id` | `ContextPack` da task (nada é enviado) | — |

`TaskView` é a task mais o que o motor calcula: `waitingFor`, `subtasks` e
`can` (os estados válidos agora) — a UI só oferece o que o motor aceitaria.

### Agentes (Fase 8b, ADR-0015)

O Agent Manager e as travas de arquivo. Referência:
[`agents.md`](./agents.md).

| Comando | Argumentos | Retorno | Histórico |
| ------- | ---------- | ------- | --------- |
| `agents_list` | `projectId?` (padrão: o projeto aberto) | `AgentView[]`, em ordem do painel | — |
| `agent_get` | `id` | `AgentView \| null` | — |
| `agent_start` | `request: { taskId, provider?, model?, maxTurns?, autonomy?, maxCostUsd? }` | `Agent` em `QUEUED`; recusa task encerrada, bloqueada, não liberada ou que já tem agente | `AGENT_STARTED` quando ele começa |
| `agent_stop` | `id` | `Agent` | `AGENT_FINISHED` |
| `agents_stop_all` | `projectId?` | quantos foram parados | `AGENT_FINISHED` de cada um |
| `agent_locks` | `projectId?` | `FileLock[]` (arquivos em uso agora) | — |
| `agent_settings_get` | — | `AgentSettings` | — |
| `agent_settings_save` | `settings` | `AgentSettings`; recusa fora da faixa | — |

| `agent_pause` / `agent_resume` | `id` | `AgentView`; só um agente em execução pode ser pausado | `EXECUTION_PAUSED` / `EXECUTION_RESUMED` |

`AgentView` é o agente mais o que o serviço calcula: `waiting` (por que
está na fila), `taskTitle`, `taskStatus`, `paused`, `approval` (o que ele
espera você autorizar) e `mode` (o modo de autonomia das chamadas dele).
`agent_start` aceita `autonomy` (o modo só para o agente e os subagentes
dele; ausente, o do projeto) e `maxCostUsd` (o teto de custo só para ele;
ausente, o das configurações — Fase 11). Desde a Fase 11, `AgentView` traz
também `costUsd` (o que o agente gastou) e `queuePosition` (a posição na
fila).

As IAs conduzidas por um agente recebem duas ferramentas a mais,
`agent.finish` e `agent.delegate`, pelo mesmo caminho auditado das outras
(`TOOL_CALLED`). Escritas em arquivo travado por outro agente voltam como
`ok: false` com `kind: "LOCKED"`.

### Autonomia (Fase 9, ADR-0016)

Referência: [`docs/autonomy.md`](./autonomy.md). Todos agem como o usuário;
nenhuma ferramenta de IA chega a eles.

| Comando | Argumentos | Retorno | Histórico |
| ------- | ---------- | ------- | --------- |
| `autonomy_get` | `projectId?` | `AutonomyOverview` (modo, modo do projeto, padrão, regras, regras do Assistido e padrão, pausa, pedidos, liberações, aviso) | — |
| `autonomy_set_mode` | `projectId?`, `mode?` | `AutonomyOverview`; `mode` nulo volta ao padrão; sem projeto, muda o padrão | `AUTONOMY_CHANGED` |
| `autonomy_set_default` | `projectId?`, `mode` | `AutonomyOverview` | `AUTONOMY_CHANGED` |
| `autonomy_save_rules` | `projectId?`, `rules` | `AutonomyOverview`; recusa regra inválida dizendo qual | `AUTONOMY_CHANGED` |
| `autonomy_reset_rules` | `projectId?` | `AutonomyOverview` | `AUTONOMY_CHANGED` |
| `autonomy_try` | `projectId?`, `mode?`, `rules?`, `tool`, `args?` | `Trial` (decisão, regra, motivo, cada alvo) | — |
| `approvals_pending` | — | `ApprovalView[]`, do mais antigo ao mais novo | — |
| `approval_answer` | `id`, `answer` (`approve`, `approveSession`, `deny`), `note?` | — | `APPROVAL_DECIDED` |
| `autonomy_revoke` | `grantId` | se havia a liberação | — |
| `execution_pause` / `execution_resume` | — | se mudou | `EXECUTION_PAUSED` / `EXECUTION_RESUMED` |

Chamadas das IAs que o gate recusa voltam como `ok: false` com
`kind: "DENIED"` (o usuário ou uma regra negou; a mensagem diz quem e por
quê) ou `kind: "CANCELLED"` (o turno foi cancelado enquanto esperava), e
ficam no histórico como `TOOL_CALLED`.

### GitHub (Fase 10, ADR-0017)

Referência: [`docs/github.md`](./github.md). As operações são ferramentas
(`github.*`, `git.fetch`, `git.remotes`) pelo `runtime_invoke`; só a
configuração e o token têm comandos próprios.

| Comando | Argumentos | Retorno | Histórico |
| ------- | ---------- | ------- | --------- |
| `github_settings_get` | — | `GitHubSetup` (host, API, se há token no cofre, variável de ambiente, `gh`, aviso) | — |
| `github_settings_save` | `settings: { host, apiUrl? }` | `GitHubSetup`; recusa valores inválidos | — |
| `github_token_save` | `token` | `GitHubSetup` (o token não volta) | — |
| `github_token_clear` | — | `GitHubSetup` | — |
| `open_url` | `url` (só `http(s)`) | abre no navegador do sistema | — |

### Tokens e custo (Fase 11, ADR-0018)

Referência: [`docs/tokens.md`](./tokens.md).

| Comando | Argumentos | Retorno | Histórico |
| ------- | ---------- | ------- | --------- |
| `spend_report` | `days` (1, 7, 30…), `projectId?`, `allProjects?` | `SpendReport` (gasto, economia do cache, tokens, chamadas, chamadas sem preço, compactações e `rows` por provider e modelo) | — |
| `agents_budget` | `projectId?` | `BudgetView` (`spentTodayUsd`, `budgetUsd`, `unpriced`, `exhausted`) | — |
| `session_compact` | `id` | `TurnResult`; recusa sessão ocupada, fechada ou sem conversa | `CONTEXT_COMPACTED` |

O transcript da sessão recebe o evento `compacted` (`automatic`,
`beforeTokens`, `afterTokens`, `messages`, `summary`) e os avisos de
retentativa como `notice`.

### Segredos (ADR-0020)

Chaves e tokens que as IAs usam pelo nome (`{{secret:NOME}}`). O valor vai
para o cofre do sistema (conta `secret:<NOME>` do serviço
`dev.orchestrator.desktop`) e para a memória do runtime; os nomes ficam em
`<app-data>/secrets.json`. **Nenhum comando devolve um valor.**

| Comando | Argumentos | Retorno |
| ------- | ---------- | ------- |
| `secrets_list` | — | `SecretsView` (nomes, data, se o cofre tem o valor, o marcador, nome do cofre, aviso); recarrega o runtime |
| `secret_save` | `name`, `value` | `SecretsView`; nome: letras sem acento, números, `_`, `-`, `.` (até 64) |
| `secret_delete` | `name` | `SecretsView` |

### Configurações (ADR-0021)

Referência: [`docs/settings.md`](./settings.md).

| Comando | Argumentos | Retorno |
| ------- | ---------- | ------- |
| `guidance_get` | — | `GuidanceView` (opções, regras do usuário, arquivos de instruções do projeto aberto, skills encontradas, pastas) |
| `guidance_settings_save` | `settings` | `GuidanceSettings` |
| `rules_save` | `text` | — ; até 40.000 caracteres |
| `skill_get` | `name` | `SkillDoc` (instruções e arquivos da pasta) |
| `skill_save` | `skill` (`name`, `description`, `body`, `previousName`) | `SkillInfo`; só as do Orchestrator |
| `skill_delete` | `name` | — ; só as do Orchestrator |
| `skill_set_enabled` | `name`, `enabled` | `GuidanceSettings` |
| `mcp_list` | — | `ServerView[]` (configuração, estado, erro, servidor, ferramentas, saída do servidor) |
| `mcp_save` | `server`, `previousId` | `ServerView[]`; conecta em segundo plano |
| `mcp_import` | `text` (JSON do Claude/Cursor) | ids adicionados |
| `mcp_restart` / `mcp_delete` | `id` | — / `ServerView[]` |
| `mcp_set_tool` | `id`, `tool`, `enabled` | `ServerView[]` |
| `clis_list` | — | `CliStatus[]` (instalado, versão, login, comandos de instalar e entrar, opções) |
| `cli_save` | `kind`, `settings` | `CliStatus[]`; liga ou desliga o provider |
| `offline_status` | — | `OfflineView` (Ollama, modelos, catálogo, conexão, downloads, comando de instalação) |
| `offline_start` | — | inicia `ollama serve` |
| `offline_pull` | `model` | termina com o download; progresso por `runtime://offline` |
| `offline_cancel` / `offline_delete` | `model` | — |
| `offline_use` | — | id da conexão `ollama`, criada ou atualizada |

### Atualizações (Fase 12, ADR-0019)

Referência: [`docs/release.md`](./release.md). O plugin de atualização fica
no Rust; a webview não tem as permissões dele.

| Comando | Argumentos | Retorno | Histórico |
| ------- | ---------- | ------- | --------- |
| `update_status` | — | `UpdateStatus` (versão, commit, sistema, tipo de instalação, se o build procura atualizações e por que não, procura automática, última procura e erro, fase, versão disponível) | — |
| `update_check` | — | `UpdateInfo \| null` (versão, data, notas); recusa em build sem a chave | — |
| `update_install` | — | — ; para os agentes (com handoff), faz o backup dos dados (sem ele, recusa), baixa, confere a assinatura e instala; o progresso chega por `runtime://update` | `AGENT_FINISHED` dos agentes parados; `APP_UPDATED` na abertura seguinte |
| `update_restart` | — | reinicia o app (abre a versão instalada) | — |
| `update_settings_save` | `autoCheck` | `UpdateStatus` | — |

### Dados e backups (ADR-0022)

Referência: [ADR-0022](./adr/0022-dados-preservados-nas-atualizacoes.md).

| Comando | Argumentos | Retorno | Histórico |
| ------- | ---------- | ------- | --------- |
| `backup_status` | — | `BackupStatus` (pasta de dados e de backups, quantos automáticos ficam, backups do mais novo ao mais antigo, avisos da abertura) | — |
| `backup_create` | `label?` | `BackupStatus`; banco por `VACUUM INTO`, configurações e skills | — |
| `backup_delete` | `id` | `BackupStatus` | — |
| `backup_restore` | `id` | — ; para os agentes (com handoff) e reinicia; a abertura seguinte guarda o estado atual e restaura | `AGENT_FINISHED` dos agentes parados |

`app_info.dataNotices` traz o que a abertura fez com os dados (backup,
restauração, arquivo guardado como `.unreadable-…`).

## Eventos

| Evento | Payload | Uso |
| ------ | ------- | --- |
| `runtime://stream` | `StreamEvent` | saída de terminal/processo ao vivo, término; eventos de sessão de provider |
| `runtime://audit` | `AuditEvent` | painel HISTORY, atualização de listas e do explorer |
| `runtime://update` | `UpdateEvent` (`checked`, `available`, `backedUp`, `progress`, `installed`, `failed`) | aba "Sobre e atualizações" e chip da barra de status (Fase 12) |
| `runtime://offline` | `OfflineEvent` (`progress`, `done`, `failed`) | downloads de modelos offline (ADR-0021) |

`StreamEvent`:

```ts
| { type: "terminalOutput"; terminalId; offset; data }
| { type: "terminalExited"; terminalId; exitCode }
| { type: "processOutput"; processId; stream: "stdout" | "stderr"; offset; data }
| { type: "processExited"; processId; exitCode; stopped }
| { type: "session"; sessionId; seq; event: SessionEvent }   // Fase 3
```

`offset` é a posição (bytes UTF-8) de `data` no fluxo completo — a mesma
coordenada de `since`/`next` em `terminal.read`/`process.read`.

## Sincronizando buffer e eventos sem perder saída

Ao abrir a visualização de um terminal/processo que já está rodando
(`apps/desktop/src/lib/outputSync.ts`):

1. inscrever-se em `runtime://stream` e **enfileirar** os eventos;
2. aguardar o listener Tauri estar registrado;
3. ler o buffer (`terminal.read`/`process.read`) → `data`, `next`;
4. escrever `data`; descartar da fila os eventos com `offset < next`;
5. a partir daí, escrever cada evento com `offset >= next` e avançar.

Sessões de provider usam a mesma ideia com `seq` em vez de offset
(`apps/desktop/src/lib/transcript.ts`): enfileirar eventos `session` da
sessão, aguardar o listener, ler `session_get` (`entries`, `lastSeq`), aplicar
as entradas e depois só eventos com `seq > lastSeq`. Trechos de texto que o
backend fundiu numa entrada carregam o `seq` do último trecho, então nada se
repete.

## Persistência

`DesktopSink` grava cada `AuditEvent` no banco local
`<app-data>/orchestrator.db` (Linux: `~/.local/share/dev.orchestrator.desktop/`,
Windows: `%APPDATA%\dev.orchestrator.desktop\`, macOS:
`~/Library/Application Support/dev.orchestrator.desktop/`), marca o projeto
e depois emite `runtime://audit`. Eventos derivados (`PROJECT_CREATED`, a
entrada "Stack" da memória) seguem o mesmo caminho
([ADR-0012](./adr/0012-sqlite-memoria-e-historico.md)).

- O `audit.jsonl` das fases anteriores é importado na primeira execução e
  renomeado para `audit.jsonl.imported`.
- Sessões de provider (com as opções de contexto), transcripts,
  deliberações do Conselho e handoffs também ficam no banco.
- A configuração do contexto fica em `<app-data>/context.json` (desde a
  Fase 11, com a compactação).
- A procura automática de atualizações e a última versão aberta ficam em
  `<app-data>/updates.json` (Fase 12).
- Os grupos de processos iniciados pelo runtime (Linux e macOS) ficam em
  `<app-data>/processes.json`, para o app encerrar o que sobrar de uma
  queda (Fase 11).
- As tasks do projeto ficam no banco (migração 3, ADR-0014).
- Agentes e as travas de arquivo ficam no banco (migração 4, ADR-0015), e a
  configuração deles em `<app-data>/agents.json`. Ao abrir o app, agentes
  que ficaram em execução são encerrados e as travas, liberadas.
- Os projetos abertos lado a lado (`projects.open_rank`) e os pares de
  projetos relacionados (`project_links`) ficam no banco (migração 5,
  ADR-0023).
- Backups ficam em `<app-data>/backups/` (ADR-0022): antes de uma
  atualização, ao abrir uma versão nova, antes de migrar o banco e quando
  o usuário pede. Os arquivos de configuração são gravados de forma
  atômica, e um que não pôde ser lido é guardado como
  `<nome>.unreadable-<data>` antes de qualquer gravação.
- Se o arquivo não abrir, o app usa um banco em memória e avisa
  (`app_info.databaseWarning`, na barra de status; o rodapé do HISTORY
  mostra `banco: (memória)`).

## Segurança da webview

- Capability `default`: apenas `core:default` (eventos, janela) e os comandos
  acima. Nenhum plugin com acesso ao SO é exposto à webview: todo acesso passa
  pelo runtime. O plugin de diálogo é usado só pelo Rust (`pick_folder`); a
  webview não recebe permissões dele.
- CSP: `default-src 'self' ipc: http://ipc.localhost`; `style-src 'self'
  'unsafe-inline'` com `dangerousDisableAssetCspModification: ["style-src"]`,
  necessário porque o xterm.js injeta elementos `<style>` (com nonce, o
  navegador ignoraria `'unsafe-inline'`).

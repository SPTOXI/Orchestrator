# Arquitetura do Orchestrator

Este documento descreve a arquitetura-alvo do Orchestrator, o que já foi
implementado e onde cada responsabilidade vive no repositório. Toda mudança
estrutural é registrada antes como ADR em [`docs/adr/`](./docs/adr).

## 1. Princípios (regra de ouro)

1. **IA é substituível.** Nenhum módulo do núcleo depende de OpenAI ou Anthropic.
2. **Projeto é permanente.** Memória, histórico e decisões pertencem ao projeto.
3. **Agentes são descartáveis.** Conhecimento vive no projeto, não no agente.
4. **Providers são intercambiáveis.** Todos implementam a mesma interface `AIProvider`.
5. **Workspace local é a fonte primária.** GitHub é remoto.
6. **O Orchestrator controla a execução.** Providers pedem `tool_call`; o runtime executa.
7. **O usuário decide o nível de autonomia** (Assistido, Autônomo, Acesso Irrestrito).
8. **Observabilidade não é restrição.** Logs, histórico e auditoria são sempre registrados,
   inclusive em Acesso Irrestrito.

## 2. Visão em camadas

```text
┌──────────────────────────────────────────────────────────────────────┐
│ Desktop UI (React + TypeScript)                  apps/desktop/src    │
│   painéis: PROJECT · AI PROVIDERS · TASKS · AGENTS · TERMINAL · GIT   │
│            MEMORY · HISTORY                                           │
└───────────────▲──────────────────────────────┬───────────────────────┘
                │ eventos (runtime://stream,    │ invoke (runtime_invoke,
                │          runtime://audit)     │  terminal_input, session_*, …)
┌───────────────┴──────────────────────────────▼───────────────────────┐
│ Tauri (ponte IPC, sem lógica de domínio)   apps/desktop/src-tauri    │
└───────────────▲──────────────────────────────┬───────────────────────┘
                │ EventSink                     │ ToolRuntime::invoke(ToolCall),
                │                               │ SessionManager (providers)
┌───────────────┴──────────────────────────────▼───────────────────────┐
│ Rust Runtime                                                          │
│   Tool Runtime (packages/runtime)                                     │
│     filesystem · shell · terminal (PTY) · process manager             │
│     git (Fase 2) · github (Fase 10) · package managers · runtimes     │
├───────────────────────────────────────────────────────────────────────┤
│ Orchestrator Core                                                     │
│   core (contratos)  · provider layer (Fase 3: AIProvider, registro,   │
│   sessões; Fase 4: conexões de API) · router/Conselho (Fase 5) ·      │
│   memory/history (Fase 6) · orchestrator engine · agent manager ·     │
│   task manager · context builder                                      │
├───────────────────────────────────────────────────────────────────────┤
│ Local Database (SQLite, Fase 6)        <app-data>/orchestrator.db     │
└───────────────────────────────────────────────────────────────────────┘
```

O fluxo de controle é sempre:

```text
React → Tauri → Rust Runtime → Orchestrator Core
```

e o fluxo de trabalho de uma IA é:

```text
AIProvider ──tool_call──▶ Orchestrator ──▶ Tool Runtime ──▶ SO / Workspace / Git ──▶ GitHub
```

**Nenhum provider acessa o sistema operacional diretamente.** Providers só
produzem `ToolCall`s; quem executa é o `ToolRuntime`, que registra cada
execução como evento de auditoria.

## 3. Módulos e localização

Todo o núcleo roda em Rust, no processo do Tauri (ver
[ADR-0001](./docs/adr/0001-monorepo-hibrido-nucleo-em-rust.md)). O React é
apenas apresentação.

| Módulo | Local | Linguagem | Fase | Estado |
| ------ | ----- | --------- | ---- | ------ |
| Desktop UI | `apps/desktop/src` | React/TS | 1–6 | ✅ shell, PROJECT, AI PROVIDERS (cadastro de APIs, Conselho) + sessões, GIT, terminal, processos, MEMORY, HISTORY |
| Ponte IPC | `apps/desktop/src-tauri` | Rust | 1–6 | ✅ (inclui o cofre do SO, `vault.rs`, e a ligação dos stores com o banco, `persistence.rs`) |
| Core (contratos) | `packages/core` | Rust | 1–6 | ✅ `ToolCall`, `ToolResult`, `ToolDefinition`, `AuditEvent`, `StreamEvent`, `EventSink`, `ProjectProfile`, `SessionInfo`, `SessionEvent`, `TokenUsage` |
| Tool Runtime | `packages/runtime` | Rust | 1–4 | ✅ filesystem, shell, terminal, process, project, git, package, runtime; JSON Schema dos argumentos (Fase 4) |
| Git local | `packages/git` | Rust | 2 | ✅ `git` do sistema (ADR-0007) |
| Project Discovery / Profile | `packages/core` (tipos) + `packages/runtime` (detecção) | Rust | 2 | ✅ (ADR-0008) |
| AIProvider / Registry / Sessions | `packages/providers` | Rust | 3, 5, 6 | ✅ trait, registro, sessões, provider `echo` de desenvolvimento (ADR-0009); respostas avulsas `complete` (ADR-0011); `snapshot` e `SessionStore` (ADR-0012) |
| Conexões de API (OpenAI e compatíveis, Anthropic, Gemini, perfil genérico) | `packages/providers/api` | Rust | 4, 6 | ✅ cadastro livre, cofre do SO, ferramentas nativas ou por prompt, custo, teste de conexão (ADR-0010); conversa retomável após reiniciar (ADR-0012) |
| Roteador de modelos e Conselho | `packages/router` | Rust | 5, 6 | ✅ ranking sem tokens, Conselho de 1 a 5 IAs com votos e cache, modos Desligado/Sugerir/Full (ADR-0011); deliberações e cache guardados (ADR-0012) |
| SQLite, Memory, History, Decisions | `packages/memory` | Rust | 6 | ✅ banco local, histórico por projeto, projetos, sessões, memória L1/L2/L3, decisões, deliberações (ADR-0012) |
| Context Builder, Handoff | `packages/orchestrator` | Rust | 7 | planejado |
| Task Manager, Agent Manager, Subagents, File Locks | `packages/orchestrator` + `packages/agents` | Rust | 8 | planejado |
| Autonomia (Assistido/Autônomo/Irrestrito) | `packages/orchestrator` | Rust | 9 | planejado |
| GitHub | `packages/git` | Rust | 10 | planejado |
| Otimização de tokens, cache, compactação, scheduling | `packages/orchestrator` | Rust | 11 | planejado |

Alterações à estrutura original:

- adição de `packages/runtime`
  ([ADR-0002](./docs/adr/0002-pacote-runtime-para-o-tool-runtime.md));
- `packages/providers/openai` e `packages/providers/claude` substituídos por
  `packages/providers/api`, com providers só por API e cadastro livre
  ([ADR-0010](./docs/adr/0010-providers-por-api-com-cadastro-livre.md));
- adição de `packages/router` para o roteador de modelos e o Conselho
  ([ADR-0011](./docs/adr/0011-roteador-de-modelos-e-conselho.md));
- `packages/memory` como crate `orchestrator-memory`, que depende só de
  `core`; providers e roteador expõem traits (`SessionStore`,
  `DeliberationStore`) que o app liga ao banco
  ([ADR-0012](./docs/adr/0012-sqlite-memoria-e-historico.md)).

## 4. Tool Runtime (Fases 1–2)

Ponto único de execução de operações de sistema. Entrada: `ToolCall`
(`{ id, tool, args, origin }`). Saída: `ToolResult`
(`{ callId, tool, ok, output, error, startedAt, finishedAt, durationMs }`).

```text
ToolRuntime::invoke(call)
  ├── valida e desserializa args (camelCase, campos desconhecidos rejeitados)
  ├── despacha para o módulo (filesystem | shell | terminal | process |
  │                          project | git | package | runtime)
  ├── emite AuditEvent TOOL_CALLED (sempre, com sucesso ou erro)
  ├── emite eventos de domínio (FILE_CHANGED, COMMAND_EXECUTED, …)
  └── retorna ToolResult
```

Catálogo implementado (detalhes em [`docs/tool-runtime.md`](./docs/tool-runtime.md)):

| Grupo | Operações |
| ----- | --------- |
| filesystem | `list`, `read`, `write`, `move`, `delete` |
| shell | `execute`, `list` |
| terminal | `create`, `write`, `read`, `close`, `list` |
| process | `start`, `stop`, `list`, `read` |
| project | `discover`, `profile`, `open` |
| git | `status`, `diff`, `log`, `branch`, `checkout`, `add`, `commit`, `pull`, `push`, `stash`, `reset` |
| package | `install`, `run` |
| runtime | `node`, `python`, `docker` |

Cada ferramenta é marcada no catálogo como consulta (`readOnly`) ou ação; a
marca vai no evento `TOOL_CALLED` (ADR-0008). Extensões ao catálogo da seção 9:
`shell.list`, `terminal.list`, `process.read`
([ADR-0004](./docs/adr/0004-operacoes-auxiliares-do-tool-runtime.md)) e
`project.*` ([ADR-0008](./docs/adr/0008-projeto-deteccao-e-diretorio-base.md)).

Operações planejadas: `github.*` (Fase 10).

### 4.1 Terminal real

- PTY nativo via `portable-pty` (ConPTY no Windows, pty Unix no Linux/macOS).
- Shells detectados: PowerShell 7 (`pwsh`), Windows PowerShell, CMD, WSL,
  Git Bash (Windows); bash, zsh, fish, sh, pwsh (Unix).
- A saída é decodificada em UTF-8 de forma incremental, publicada em tempo real
  (`StreamEvent::TerminalOutput`) e mantida num buffer circular endereçado por
  offset, para que uma IA leia apenas o que é novo (`terminal.read { since }`).

### 4.2 Shell e processos

- `shell.execute` roda um comando não interativo e devolve stdout, stderr,
  exit code, duração e `timedOut`. Exit code ≠ 0 **não** é erro da ferramenta:
  é informação para quem chamou.
- `process.start` mantém processos de longa duração; saída em tempo real e em
  buffer; `process.stop` encerra a **árvore** de processos (grupo de processos
  no Unix, `taskkill /T /F` no Windows).
- Ao fechar o app (inclusive por SIGTERM/SIGINT/SIGHUP no Unix), todos os
  terminais e processos gerenciados são encerrados.

### 4.3 Projeto (Fase 2)

- `project.open` define o projeto ativo: o **diretório base do runtime** passa
  a ser a raiz do projeto (caminhos relativos e `cwd` padrão de shell,
  terminais e processos) e é emitido `PROJECT_OPENED`.
- `project.profile` monta o PROJECT PROFILE: nome, caminho, Git (root, branch,
  upstream, remotes, contagem de alterações), linguagens, frameworks, package
  managers, runtimes (com versão pedida), Docker (Dockerfiles, compose,
  imagens), bancos (Prisma, compose, dependências), ferramentas, arquivos
  importantes, scripts e as **evidências** de cada conclusão. `.env` nunca é
  lido.
- `project.discover` varre raízes (padrão: pasta do usuário e pastas comuns
  como `C:\Projetos`) em largura, sem entrar em `node_modules`, `target`,
  pastas ocultas etc., e sem descer dentro de um projeto encontrado.

### 4.4 Git (Fase 2)

`packages/git` executa o `git` do sistema (credenciais, hooks e configuração
do usuário valem igual ao terminal) e lê formatos estáveis para máquinas
([ADR-0007](./docs/adr/0007-git-via-cli-do-sistema.md)). Sem prompts
interativos (`GIT_TERMINAL_PROMPT=0`); consultas não disputam o `index.lock`
(`GIT_OPTIONAL_LOCKS=0`). `git.commit` emite `GIT_COMMIT`; `git.push`, `GIT_PUSH`.
Falhas do Git viram `COMMAND_FAILED` com a saída do comando.

### 4.5 Autonomia e o runtime

Desde a Fase 3 há dois chamadores: o usuário, pela UI, e as sessões de
provider, por `TurnContext::call_tool` → `ToolRuntime::invoke` (origem
`agent`). Até a Fase 9 as chamadas de IA são executadas sem gate, com
auditoria completa. O caminho `call_tool` → `invoke` é o ponto onde, na
Fase 9, entra o gate de autonomia:

| Modo | Comportamento no gate |
| ---- | --------------------- |
| Assistido | pede autorização ao usuário quando a política exigir |
| Autônomo | aplica as políticas configuradas pelo usuário |
| Acesso Irrestrito | **nenhuma** política operacional; sem confirmações ocultas, sem lista de comandos proibidos |

Em todos os modos a auditoria continua ativa. O runtime **não** contém hoje
nenhuma lista de comandos proibidos nem bloqueio silencioso, e não deve passar
a conter fora do gate explícito da Fase 9.

## 5. Contratos e eventos (packages/core)

- `ToolCall`, `ToolResult`, `ToolError`, `ToolErrorKind`, `CallOrigin`
  (`user` | `agent { agentId, sessionId?, provider? }` | `system` |
  `council { deliberationId? }`, Fase 5), `ToolSpec`,
  `ToolDefinition` (com o JSON Schema dos argumentos, Fase 4).
- `SessionInfo`, `SessionEvent`, `SessionLogEntry`, `TokenUsage`,
  `SessionStatus`, `TurnStatus`, ids `SessionId`/`TurnId`/`ProviderId` —
  contratos das sessões de provider (Fase 3, ADR-0009).
- `ProjectProfile`, `ProjectCandidate`, `GitSummary` — contratos do projeto
  (Fase 2).
- `AuditEvent { id, at, kind, origin, summary, data }` — histórico durável,
  independente de provider. `EventKind` contém todos os eventos da seção 22 do
  documento mestre, mais `PROCESS_EXITED` e `TERMINAL_EXITED`
  ([ADR-0005](./docs/adr/0005-observabilidade-antes-do-sqlite.md)) e
  `SESSION_STARTED`, `SESSION_RESUMED`, `SESSION_CLOSED`, `TURN_COMPLETED`
  ([ADR-0009](./docs/adr/0009-camada-de-providers-e-sessoes.md)),
  `CONNECTION_SAVED`, `CONNECTION_REMOVED`
  ([ADR-0010](./docs/adr/0010-providers-por-api-com-cadastro-livre.md)),
  `COUNCIL_CONFIGURED`, `COUNCIL_DELIBERATED`, `ROUTE_DECIDED`
  ([ADR-0011](./docs/adr/0011-roteador-de-modelos-e-conselho.md)),
  `MEMORY_SAVED`, `MEMORY_REMOVED`, `DECISION_SAVED`
  ([ADR-0012](./docs/adr/0012-sqlite-memoria-e-historico.md)).
- `StreamEvent` — eventos de alta frequência e não duráveis (saída de
  terminal/processo, término, eventos de sessão de provider).
- `EventSink` — trait que desacopla o runtime de quem consome eventos. O
  sink do app grava no banco local e depois emite para a UI.

IDs são UUID v7 (ordenáveis por tempo) e servem de chave primária no banco.

## 6. Camada IPC

Detalhada em [`docs/ipc.md`](./docs/ipc.md) e
[ADR-0003](./docs/adr/0003-gateway-ipc-unico.md).

| Comando Tauri | Finalidade |
| ------------- | ---------- |
| `runtime_invoke(tool, args)` | gateway único para qualquer ferramenta; auditado |
| `runtime_tools()` | catálogo de ferramentas |
| `terminal_input(id, data)` | canal de digitação humana no terminal (streaming, não auditado por tecla) |
| `terminal_resize(id, cols, rows)` | redimensionamento do PTY |
| `history_recent(limit)`, `history_query(query)` | histórico do banco: janela recente ou página com filtros e cursor (Fase 6) |
| `app_info()` | versão, SO, diretórios, projeto aberto |
| `pick_folder()` | seletor nativo de pasta (só UI; a pasta escolhida é aberta via `project.open`) |
| `providers_list`, `provider_inspect`, `provider_select` | registro de providers e provider ativo (Fase 3) |
| `sessions_list`, `session_start`, `session_get`, `session_send`, `session_cancel`, `session_close`, `session_resume`, `session_spawn` | sessões de provider (Fase 3) |
| `connections_list`, `connection_save`, `connection_delete`, `connection_test`, `connection_models` | cadastro de APIs (Fase 4); a chave nunca volta para a webview |
| `router_recommend`, `council_*`, `route_start_session` | roteador e Conselho (Fase 5) |
| `projects_recent`, `project_current`, `project_forget`, `projects_import_recent` | projetos registrados no banco (Fase 6) |
| `memory_overview`, `memory_list`, `memory_save`, `memory_delete`, `memory_search`, `decisions_list`, `decision_save` | memória do projeto e decisões (Fase 6) |

| Evento Tauri | Payload |
| ------------ | ------- |
| `runtime://stream` | `StreamEvent` (saída/término de terminal e processo, eventos de sessão) |
| `runtime://audit` | `AuditEvent` |

## 7. Modelo de dados (Fase 6)

Um banco SQLite por instalação, `<app-data>/orchestrator.db`, embutido e em
WAL, com migrações por `PRAGMA user_version`
([ADR-0012](./docs/adr/0012-sqlite-memoria-e-historico.md), referência em
[`docs/memory.md`](./docs/memory.md)). Nada é gravado dentro da pasta do
projeto.

| Tabela | Conteúdo |
| ------ | -------- |
| `projects` | projetos abertos (caminho, nome, stack detectada) |
| `audit_events` | histórico durável, com `project_id` e `session_id`; `tool_calls` é uma *view* sobre ele |
| `sessions`, `session_entries` | sessões de provider (com a conversa nativa) e transcripts: o `agent_sessions`/`messages` do documento mestre |
| `memory_entries` | memória L2 |
| `decisions` | decisões do projeto |
| `deliberations` | deliberações e cache do Conselho |
| `search_index` | índice FTS5 da busca L3 |

As tabelas `tasks`, `task_dependencies`, `agents`, `artifacts`, `file_locks`
e `git_operations` entram com as migrações das Fases 8–10. Configuração
(`connections.json`, `council.json`) continua em arquivos, e segredos só no
cofre do SO.

## 8. Memória, contexto e handoff (Fases 6–7)

Implementado na Fase 6 (`packages/memory`):

- **L1 Working Memory**: derivada do histórico do projeto: sessões, arquivos
  alterados, comandos com código de saída e erros recentes. A task atual e o
  objetivo entram com as tasks (Fase 8).
- **L2 Project Memory**: entradas de arquitetura, stack, convenção, regra e
  nota, com etiquetas, fixação e origem (usuário, IA ou detector). A stack
  detectada vira uma entrada ao abrir o projeto.
- **Decisões**: contexto, decisão, consequências e estado. Nunca são
  apagadas.
- **L3 Historical Memory**: busca FTS5 na memória, decisões, mensagens das
  sessões e eventos notáveis (commits, comandos, falhas).

Fase 7:

- **Context Builder**: monta `TASK + L1 + L2 relevante + arquivos relevantes +
  erros recentes + histórico relevante + estado do Git + handoff`. Nunca envia
  L3 inteiro, o repositório inteiro ou todas as mensagens.
- **HandoffPacket**: `goal, status, completed, remaining, files, commands,
  errors, decisions, tests, nextAction` — permite a outra IA continuar sem
  receber a conversa anterior.

Os buffers com offset do runtime (`terminal.read`/`process.read { since }`)
já existem para que o Context Builder envie apenas saída nova.

## 9. Providers (Fases 3–6)

```typescript
interface AIProvider {
  start(); resume(); execute(); stream(); cancel();
  spawnAgent(); inspect(); capabilities();
  complete(); // Fase 5: resposta avulsa, sem sessão nem ferramentas
  snapshot(); // Fase 6: estado da sessão nativa para retomar após reiniciar
}
```

Implementado na Fase 3 como trait Rust em `packages/providers`
([ADR-0009](./docs/adr/0009-camada-de-providers-e-sessoes.md), referência em
[`docs/providers.md`](./docs/providers.md)):

- **`AIProvider`** — os métodos acima (+ `descriptor`), com padrões
  sensatos para `stream`, `resume`, `cancel`, `spawn_agent`, `complete` e
  `snapshot`.
- **`ProviderRegistry`** — providers registrados, provider ativo,
  `PROVIDER_SWITCHED`.
- **`SessionManager`** — a sessão pertence ao Orchestrator: um turno por vez,
  transcript numerado, uso de tokens/custo, cancelamento com prazo,
  encerrar/retomar, subagentes (mesmo provider ou outro), eventos ao vivo e
  histórico. Desde a Fase 6, com um `SessionStore`, sessões e transcripts
  sobrevivem ao reinício e voltam encerradas, prontas para retomar.
- **`TurnContext::call_tool`** — única saída do provider para o sistema; o
  Orchestrator executa pelo Tool Runtime com a sessão como origem.
- **`echo`** — provider de desenvolvimento sem IA, para testes e builds de
  desenvolvimento.

### 9.1 Conexões de API (Fase 4)

Os providers são **APIs cadastradas pelo usuário**, quantas ele quiser
([ADR-0010](./docs/adr/0010-providers-por-api-com-cadastro-livre.md),
referência em [`docs/api-connections.md`](./docs/api-connections.md)).
Crate `packages/providers/api`:

- **Tipos:** `openai` (e qualquer API compatível), `anthropic`, `gemini` e
  `generic`. O `generic` descreve qualquer API HTTP/JSON por um perfil:
  caminho, autenticação, modelo do corpo, SSE/NDJSON e onde ler texto e uso.
- **Ferramentas:** chamada de funções nativa, protocolo por prompt
  (`<tool_call>`, para qualquer modelo de texto) ou nenhuma. O catálogo vai
  ao modelo com o JSON Schema de cada ferramenta, e quem executa é sempre o
  Orchestrator.
- **Credenciais:** a chave fica no cofre do SO ou numa variável de
  ambiente. Nunca vai para arquivo, histórico, log ou UI.
- **Modelos:** descoberta pela API, preços informados pelo usuário (custo
  por turno), contexto e etiquetas usadas pelo roteador e pelo Conselho (9.2).
- **Sessões:** usam a instância registrada a cada turno. Editar uma conexão
  vale para as sessões abertas, sem perder a conversa. A conversa vai para
  o banco ao fim de cada turno e continua depois de reiniciar o app.

Nenhuma chamada específica de fornecedor fora do adapter. Uma IA nova entra
como conexão cadastrada (sem código) ou como um protocolo novo no crate.

### 9.2 Roteador e Conselho (Fase 5)

Escolha do modelo de cada tarefa
([ADR-0011](./docs/adr/0011-roteador-de-modelos-e-conselho.md), referência
em [`docs/router.md`](./docs/router.md)). Crate `packages/router`, que
depende só de `core` e `providers`:

- **Roteador:** detecta a atividade (código, depuração, revisão, testes,
  planejamento, documentação, resumo, geral) e dá nota de 0 a 100 a cada
  modelo registrado. A nota combina etiquetas, preço, contexto, ferramentas
  e perfil (qualidade/velocidade), com pesos pela preferência. Não gasta
  tokens e explica cada nota e cada exclusão.
- **Conselho:** de 1 a 5 membros (provider + modelo; com um, é o
  "gerenciador").
  - Os membros recebem a lista curta do roteador por
    `AIProvider::complete`, sem sessão e sem ferramentas, e respondem com
    JSON em paralelo.
  - Uma contagem de Borda ponderada pela confiança decide.
  - Abstenções (erro, prazo, JSON inválido) não travam a decisão.
- **Modos:**
  - *Desligado:* só o roteador;
  - *Sugerir:* o usuário aprova ou escolhe outro modelo;
  - *Full:* o Conselho abre a sessão e envia a tarefa, com origem
    `council`. O Full não dispensa o gate de autonomia da Fase 9.
- **Cache:** mesma pergunta, mesmos candidatos e mesmos membros = zero
  tokens, também entre execuções do app (Fase 6).
- **Histórico:** `COUNCIL_CONFIGURED`, `COUNCIL_DELIBERATED` e
  `ROUTE_DECIDED`; as deliberações ficam no banco.
- **Configuração:** `council.json` (configuração continua em arquivo,
  ADR-0012).

## 10. Plataformas

| SO | Terminal | Shell padrão | Encerramento de árvore |
| -- | -------- | ------------ | ---------------------- |
| Windows | ConPTY | pwsh → powershell → cmd | `taskkill /T /F` |
| Linux | pty | `$SHELL` → bash → sh | `kill(-pgid)` |
| macOS | pty | `$SHELL` → zsh → bash → sh | `kill(-pgid)` |

CI em Linux, Windows e macOS: ver
[ADR-0006](./docs/adr/0006-ci-multiplataforma.md).

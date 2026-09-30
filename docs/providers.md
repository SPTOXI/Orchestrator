# AI Provider Layer (Fases 3–7)

Referência de `packages/providers` (crate `orchestrator-providers`). Decisão
registrada em [ADR-0009](./adr/0009-camada-de-providers-e-sessoes.md).
Os providers reais são as **conexões de API** cadastradas pelo usuário
(Fase 4, [ADR-0010](./adr/0010-providers-por-api-com-cadastro-livre.md)):
ver [api-connections.md](./api-connections.md). O roteador e o Conselho
(Fase 5) escolhem entre eles: ver [router.md](./router.md). O contexto do
projeto no primeiro turno e o handoff entre IAs (Fase 7) estão em
[context.md](./context.md).

```text
UI ──session_send──▶ SessionManager ──stream()──▶ AIProvider (adapter)
                        │   ▲                          │
                        │   └── TurnContext ◀──────────┘ emit_text / report_usage /
                        │         │                      call_tool(tool, args)
                        │         ▼
                        │   ToolExecutor = ToolRuntime::invoke   (auditado: TOOL_CALLED …)
                        ▼
      transcript (seq) + StreamEvent::Session + SESSION_* / TURN_COMPLETED
```

O provider **nunca** age no sistema: a única saída para o mundo é
`TurnContext::call_tool`, que o Orchestrator executa pelo Tool Runtime com a
sessão como origem.

## Interface `AIProvider`

Trait async (`Send + Sync`), mapeada 1:1 na seção 18 do documento mestre.

| Método | Faz | Padrão |
| ------ | --- | ------ |
| `descriptor()` | id estável, nome, fornecedor, descrição | — (obrigatório) |
| `capabilities()` | streaming, ferramentas, retomada, cancelamento, subagentes nativos, raciocínio, uso de tokens, custo, respostas avulsas (`completion`), modelos | — (obrigatório) |
| `inspect()` | disponível?, versão, autenticado?, detalhe | — (obrigatório) |
| `start(spec)` | abre a sessão nativa → `NativeSession` | — (obrigatório) |
| `resume(native, spec)` | reabre a sessão nativa | `UNSUPPORTED` |
| `execute(native, input, ctx)` | um turno; devolve o texto completo | — (obrigatório) |
| `stream(native, input, ctx)` | um turno com saída incremental (`ctx.emit_text`) | chama `execute` e emite o texto de uma vez |
| `cancel(native)` | limpeza do lado do fornecedor | nada (cancelamento cooperativo pelo `ctx`) |
| `spawn_agent(parent, spec)` | sessão filha (subagente) | `start(spec)` |
| `complete(request, cancel)` | resposta avulsa: instruções + texto + modelo, **sem sessão, histórico nem ferramentas** → `Completion { text, model, usage }`; usada pelo Conselho (Fase 5) | `UNSUPPORTED` |
| `snapshot(native)` | a sessão nativa com o estado necessário para retomar depois de reiniciar o app (Fase 6) | devolve `native` como está |

- `NativeSession { reference, model, data }` é opaca para o núcleo: o
  Orchestrator só a guarda para chamar o provider de novo e, desde a Fase
  6, grava a versão de `snapshot` ao fim de cada turno. As conexões de API
  guardam ali a conversa inteira (`data.conversation`), e o `resume` a
  reconstrói.
- `SessionSpec { sessionId, projectPath, title, model, instructions }`:
  `instructions` são as instruções pedidas ao abrir a sessão. O contexto
  do projeto não vai aqui: ele chega no primeiro turno
  (`TurnInput.context`, abaixo).
- `TurnInput { text, context }` (Fase 7): `context` vem só no primeiro
  turno da sessão, com o texto montado pelo Context Builder. As conexões de
  API o anexam às instruções de sistema da conversa, que ficam iguais nos
  turnos seguintes e vão junto na persistência; o `echo` o guarda e mostra
  com `/context`.
- Em `stream`, o texto vai para o contexto; em `execute`, volta em
  `TurnOutput` e o Orchestrator o registra.
- `complete` não recebe `TurnContext`: sem ferramentas, o modelo não tem
  como agir no sistema. Quem implementa marca `capabilities().completion`;
  só esses providers podem ser membros do Conselho
  ([ADR-0011](./adr/0011-roteador-de-modelos-e-conselho.md)).

### `TurnContext`

| Método | Uso |
| ------ | --- |
| `call_tool(tool, args)` | pede uma ferramenta; devolve `ToolResult` (erros vêm como `ok: false`) |
| `tools()` | catálogo (`ToolDefinition[]`: nome, grupo, descrição, somente leitura e JSON Schema dos argumentos) que o provider oferece ao modelo |
| `emit_text` / `emit_reasoning` | saída incremental |
| `report_usage(TokenUsage)` | uso de tokens/custo; chamadas somam |
| `notice(level, message)` | aviso no transcript |
| `is_cancelled()` / `cancelled()` | cancelamento cooperativo |
| `project_path()` | workspace do projeto |

Regras de `call_tool`:

- a origem é sempre `agent { agentId, sessionId, provider }`, montada pelo
  Orchestrator; o provider não escolhe;
- a chamada roda numa tarefa própria: se o turno for abandonado, ela termina
  e o `TOOL_CALLED` é gravado mesmo assim;
- depois do cancelamento nenhuma ferramenta nova é iniciada: o resultado é
  `CANCELLED` (sem `TOOL_CALLED`, porque nada foi executado);
- o gate de autonomia (Fase 9, [autonomy.md](./autonomy.md)) está neste
  mesmo ponto: é o executor mais de fora, e recebe o token do turno por
  `ToolExecutor::execute_with` para que um pedido de autorização ou uma
  pausa possam ser cancelados junto com o turno.

## Provider Registry

- Ids estáveis (`echo` e os ids das conexões de API: `openai`,
  `anthropic`, `ollama`…); id repetido → `ALREADY_EXISTS`.
- O primeiro provider registrado vira o **ativo**; `select(id)` troca e grava
  `PROVIDER_SWITCHED { from, to }` (selecionar o que já está ativo não grava
  nada).
- `replace(provider)` troca a instância de um id (conexão editada) mantendo a
  posição; `unregister(id)` tira do registro (conexão removida ou
  desativada) e, se era o ativo, ativa o próximo com
  `PROVIDER_SWITCHED { from, to, reason: "removed" }`.
- `list()` → `ProviderInfo { id, name, vendor, description, capabilities, active }`.
- `inspect(id)` → `ProviderStatus { available, version, authenticated, detail, checkedAt }`.

## Provider Sessions (`SessionManager`)

A sessão pertence ao Orchestrator (`SessionId` UUID v7), não ao fornecedor.

```text
start ──▶ idle ──send/execute──▶ running ──(completed | cancelled | failed)──▶ idle
            ▲                                                                  │
            └──────────────── resume ◀── closed ◀──────── close ◀──────────────┘
```

- **Provider por turno.** A sessão guarda o id do provider e usa a instância
  registrada no início de cada turno (e ao retomar ou criar subagente):
  editar a conexão vale para as sessões abertas; se o id saiu do registro,
  o turno falha com `UNAVAILABLE` (`provider … is no longer registered`) e
  a sessão continua listada.
- **Um turno por vez.** `send` com turno em andamento → `BUSY`; sessão
  encerrada → `CLOSED`; entrada vazia → `INVALID_REQUEST`.
- **Falha não mata a sessão:** o turno termina `failed`, `lastError` guarda a
  mensagem e o próximo turno bem-sucedido a limpa.
- **Cancelar:** dispara o token do turno e chama `provider.cancel()`. Se o
  provider não parar em 5 s, o turno é abandonado e registrado como
  `cancelled`.
- **Encerrar** cancela o turno em andamento (se houver) e marca `closed`.
  **Retomar** chama `provider.resume()` e volta a `idle`.
- **Subagentes** (`spawn`): sessão filha no mesmo projeto, com `parentId`. Mesmo
  provider → `spawn_agent` (subagentes nativos quando existirem); outro
  provider → `start` (delegação para outra IA). O pai recebe
  `subagentSpawned` no transcript.
- **Uso de tokens:** somado por turno (`TURN_COMPLETED`) e por sessão
  (`SessionInfo.usage`). `estimated` indica valores estimados.
- **Transcript:** log numerado (`seq`) de `SessionEvent`, com trechos de texto
  consecutivos fundidos e limite de 5.000 entradas em memória.
  `annotate(id, event)` acrescenta um evento de fora de um turno (ex.: o
  handoff) e grava a sessão.

### Contexto do projeto (`ContextSource`, Fase 7)

`set_context_source(source)` instala quem monta o contexto
([ADR-0013](./adr/0013-context-builder-e-handoff.md)); no app, o
`ContextBuilder` do `orchestrator-engine`.

- **Quando:** no primeiro turno de cada sessão, qualquer que seja o
  caminho (nova sessão, Conselho no modo Full, subagente, handoff). O
  manager chama `build(ContextRequest { session, task, options, tools })`,
  com a mensagem como tarefa e `tools` = o provider pede ferramentas
  (`capabilities().toolCalls`), e passa o texto em `TurnInput.context`.
- **Uma vez:** os turnos seguintes não repetem o contexto; a IA consulta o
  resto pelas ferramentas de memória.
- **Opções por sessão** (`StartRequest.context: ContextOptions`):
  - `enabled` (`None` = a configuração global `autoAttach`);
  - `budget` (tokens estimados; `None` = o padrão);
  - `handoffId` (a sessão assume esse handoff; o contexto vai sempre).

  `context_options(id)` lê e `set_context_options(id, options)` muda, só
  antes do primeiro turno (depois: `INVALID_REQUEST`, "the project context
  was already sent with the first turn"). As opções são gravadas com a
  sessão.
- **Registro:** `SessionEvent::ContextAttached { turnId, summary }` no
  transcript e `CONTEXT_BUILT` no histórico, os dois com o resumo (tokens,
  seções, o que ficou de fora), nunca o texto.
- **Falha:** se o contexto não puder ser montado, o turno segue sem ele e
  o transcript recebe o aviso "contexto do projeto indisponível: …".

### Persistência (`SessionStore`, Fase 6)

`SessionManager::with_store(…, store)` recebe um `SessionStore`
([ADR-0012](./adr/0012-sqlite-memoria-e-historico.md)). O app usa o banco
local; os testes usam `MemorySessionStore`.

| Método | Quando o manager chama |
| ------ | ---------------------- |
| `save(PersistedSession)` | ao abrir, encerrar e retomar, e ao fim de cada turno: `info`, sessão nativa (de `snapshot`), instruções e modelo pedido |
| `append(id, entries)` | ao fim de cada turno e quando a sessão muda: só as entradas do transcript ainda não gravadas |
| `load()` | uma vez, ao criar o manager |

- **Ao reiniciar,** as sessões voltam **encerradas** (e são gravadas
  assim), com o transcript, o uso e o número de subagentes. "Retomar" chama
  `provider.resume()` com a sessão nativa guardada.
- **Falhas de gravação** são registradas pela implementação e nunca fazem
  um turno falhar.
- **Um turno interrompido** pela queda do app perde só o próprio andamento.

### Eventos ao vivo — `StreamEvent::Session { sessionId, seq, event }`

| `event.type` | Campos |
| ------------ | ------ |
| `turnStarted` | `turnId`, `input` |
| `textDelta` / `reasoningDelta` | `turnId`, `text` |
| `toolCallRequested` | `turnId`, `call: ToolCall` |
| `toolCallCompleted` | `turnId`, `result: ToolResult` |
| `usage` | `turnId`, `usage: TokenUsage` |
| `notice` | `turnId?`, `level`, `message` |
| `turnCompleted` | `turnId`, `status`, `error`, `usage`, `durationMs`, `toolCalls` |
| `statusChanged` | `status: idle \| running \| closed` |
| `subagentSpawned` | `childId`, `provider`, `title` |
| `contextAttached` | `turnId`, `summary: { tokens, budget, sections[], omitted[], handoffId }` (Fase 7) |
| `handedOff` | `handoffId`, `fromSession`, `toSession`, `provider` — gravado nas duas sessões (Fase 7) |

### Histórico (`AuditEvent`)

| Evento | `data` |
| ------ | ------ |
| `SESSION_STARTED` | `sessionId`, `provider`, `model`, `title`, `projectPath`, `parentSessionId`, `nativeRef` |
| `TURN_COMPLETED` | `sessionId`, `provider`, `turnId`, `status`, `error`, `durationMs`, `toolCalls`, `usage`, `inputChars` |
| `SESSION_CLOSED` | `sessionId`, `provider`, `turns`, `usage` |
| `SESSION_RESUMED` | `sessionId`, `provider`, `nativeRef` |
| `PROVIDER_SWITCHED` | `from`, `to`; `reason: "removed"` quando o ativo saiu do registro |
| `CONTEXT_BUILT` (Fase 7, origem `system`) | `sessionId`, `provider`, `turnId`, `projectPath`, `tokens`, `budget`, `sections`, `omitted`, `handoffId` |
| `TOOL_CALLED` (do runtime) | como na Fase 1, com `origin = agent { agentId, sessionId, provider }` |

O texto das mensagens fica no transcript (tabela `session_entries` do
banco), não no histórico de auditoria. As mensagens entram na busca L3 da
memória do projeto.

## IPC

Comandos Tauri `providers_list`, `provider_inspect`, `provider_select`,
`sessions_list`, `session_start`, `session_get`, `session_send`,
`session_cancel`, `session_close`, `session_resume`, `session_spawn`,
`session_context_get`, `session_context_set` — ver
[`ipc.md`](./ipc.md). Erros chegam como `{ kind, message }` com `kind` em
`NOT_FOUND`, `ALREADY_EXISTS`, `UNAVAILABLE`, `UNSUPPORTED`,
`INVALID_REQUEST`, `BUSY`, `CLOSED`, `CANCELLED`, `FAILED`, `INTERNAL`.

## Provider de desenvolvimento `echo`

Sem IA e sem rede. Existe para exercitar o contrato em testes e no app
(builds de desenvolvimento, ou `ORCHESTRATOR_ECHO_PROVIDER=1`).

| Entrada | Resultado |
| ------- | --------- |
| texto qualquer | `Eco: <texto>`, palavra por palavra |
| `/help` | lista os comandos |
| `/tool <ferramenta> [args JSON]` | pede a ferramenta ao Orchestrator e mostra o resultado, ex.: `/tool filesystem.list {"path": "."}` |
| `/wait <segundos>` | espera (cancelável) |
| `/fail [mensagem]` | faz o turno falhar |
| `/context` | mostra o contexto do projeto recebido no primeiro turno (Fase 7) |

`complete` devolve `Eco: <prompt>`: como membro do Conselho, o `echo` se
abstém (a resposta não é JSON), o que exercita as abstenções.

Uso de tokens estimado (≈ 4 caracteres por token), `estimated: true`.

## Adicionando um provider

Na maioria dos casos não há código: o usuário **cadastra uma conexão** no
painel AI PROVIDERS. Pode ser um tipo nativo, uma API compatível com a
OpenAI ou um perfil genérico que descreve qualquer API HTTP/JSON
([api-connections.md](./api-connections.md)).

Para um protocolo novo com suporte nativo:

1. Criar um módulo em `packages/providers/api/src/` implementando o trait
   `Protocol`:
   - `request`: monta a requisição;
   - `decoder`: lê SSE, NDJSON ou JSON;
   - `models_request` e `parse_models`: descoberta de modelos.
2. Adicionar o tipo em `ApiKind` e um preset em `presets.rs`.
3. Cobrir o protocolo com o servidor falso de `tests/api.rs`.

Um adapter fora de HTTP seria um crate em `packages/providers/<nome>`
implementando `AIProvider`, dependendo só de `orchestrator-providers` e
`orchestrator-core`. Ele precisa seguir três regras:

- executar ferramentas só com `ctx.call_tool`;
- reportar uso com `ctx.report_usage`;
- respeitar `ctx.cancelled()`.

Testes de contrato de referência: `packages/providers/tests/sessions.rs`.

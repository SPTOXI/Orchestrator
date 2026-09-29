# AI Provider Layer (Fase 3)

Referência de `packages/providers` (crate `orchestrator-providers`). Decisão
registrada em [ADR-0009](./adr/0009-camada-de-providers-e-sessoes.md).

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
| `capabilities()` | streaming, ferramentas, retomada, cancelamento, subagentes nativos, raciocínio, uso de tokens, custo, modelos | — (obrigatório) |
| `inspect()` | disponível?, versão, autenticado?, detalhe | — (obrigatório) |
| `start(spec)` | abre a sessão nativa → `NativeSession` | — (obrigatório) |
| `resume(native, spec)` | reabre a sessão nativa | `UNSUPPORTED` |
| `execute(native, input, ctx)` | um turno; devolve o texto completo | — (obrigatório) |
| `stream(native, input, ctx)` | um turno com saída incremental (`ctx.emit_text`) | chama `execute` e emite o texto de uma vez |
| `cancel(native)` | limpeza do lado do fornecedor | nada (cancelamento cooperativo pelo `ctx`) |
| `spawn_agent(parent, spec)` | sessão filha (subagente) | `start(spec)` |

- `NativeSession { reference, model, data }` é opaca para o núcleo: o
  Orchestrator só a guarda para chamar o provider de novo (e, a partir da
  Fase 6, persiste para retomar após reiniciar o app).
- `SessionSpec { sessionId, projectPath, title, model, instructions }` —
  `instructions` será preenchido pelo Context Builder (Fase 7).
- Em `stream`, o texto vai para o contexto; em `execute`, volta em
  `TurnOutput` e o Orchestrator o registra.

### `TurnContext`

| Método | Uso |
| ------ | --- |
| `call_tool(tool, args)` | pede uma ferramenta; devolve `ToolResult` (erros vêm como `ok: false`) |
| `tools()` | catálogo (`ToolSpec[]`) que o provider pode oferecer ao modelo |
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
- não há gate de autonomia na Fase 3; ele entra na Fase 9 neste mesmo ponto.

## Provider Registry

- Ids estáveis (`echo`, `openai-codex`, `claude-code`, …); id repetido →
  `ALREADY_EXISTS`.
- O primeiro provider registrado vira o **ativo**; `select(id)` troca e grava
  `PROVIDER_SWITCHED { from, to }` (selecionar o que já está ativo não grava
  nada).
- `list()` → `ProviderInfo { id, name, vendor, description, capabilities, active }`.
- `inspect(id)` → `ProviderStatus { available, version, authenticated, detail, checkedAt }`.

## Provider Sessions (`SessionManager`)

A sessão pertence ao Orchestrator (`SessionId` UUID v7), não ao fornecedor.

```text
start ──▶ idle ──send/execute──▶ running ──(completed | cancelled | failed)──▶ idle
            ▲                                                                  │
            └──────────────── resume ◀── closed ◀──────── close ◀──────────────┘
```

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
  consecutivos fundidos e limite de 5.000 entradas. Em memória até a Fase 6.

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

### Histórico (`AuditEvent`)

| Evento | `data` |
| ------ | ------ |
| `SESSION_STARTED` | `sessionId`, `provider`, `model`, `title`, `projectPath`, `parentSessionId`, `nativeRef` |
| `TURN_COMPLETED` | `sessionId`, `provider`, `turnId`, `status`, `error`, `durationMs`, `toolCalls`, `usage`, `inputChars` |
| `SESSION_CLOSED` | `sessionId`, `provider`, `turns`, `usage` |
| `SESSION_RESUMED` | `sessionId`, `provider`, `nativeRef` |
| `PROVIDER_SWITCHED` | `from`, `to` |
| `TOOL_CALLED` (do runtime) | como na Fase 1, com `origin = agent { agentId, sessionId, provider }` |

O texto das mensagens fica no transcript, não no histórico de auditoria
(vai para a tabela `messages` na Fase 6).

## IPC

Comandos Tauri `providers_list`, `provider_inspect`, `provider_select`,
`sessions_list`, `session_start`, `session_get`, `session_send`,
`session_cancel`, `session_close`, `session_resume`, `session_spawn` — ver
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

Uso de tokens estimado (≈ 4 caracteres por token), `estimated: true`.

## Adicionando um provider (Fases 4, 5 e futuras)

1. Criar o crate em `packages/providers/<nome>` dependendo só de
   `orchestrator-providers` e `orchestrator-core`.
2. Implementar `AIProvider`; chamadas do fornecedor ficam só no adapter.
3. Expor as ferramentas do Orchestrator ao modelo a partir de `ctx.tools()` e
   executar cada pedido com `ctx.call_tool`. CLIs que executam ferramentas
   por conta própria precisam delegá-las ao Orchestrator (decisão registrada
   em ADR na fase do provider).
4. Reportar uso com `ctx.report_usage` e respeitar `ctx.cancelled()`.
5. Registrar o provider em `apps/desktop/src-tauri/src/lib.rs`.

Testes de contrato de referência: `packages/providers/tests/sessions.rs`.

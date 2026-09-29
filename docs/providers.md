# AI Provider Layer (Fases 3–5)

Referência de `packages/providers` (crate `orchestrator-providers`). Decisão
registrada em [ADR-0009](./adr/0009-camada-de-providers-e-sessoes.md).
Os providers reais são as **conexões de API** cadastradas pelo usuário
(Fase 4, [ADR-0010](./adr/0010-providers-por-api-com-cadastro-livre.md)):
ver [api-connections.md](./api-connections.md). O roteador e o Conselho
(Fase 5) escolhem entre eles: ver [router.md](./router.md).

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

- `NativeSession { reference, model, data }` é opaca para o núcleo: o
  Orchestrator só a guarda para chamar o provider de novo (e, a partir da
  Fase 6, persiste para retomar após reiniciar o app).
- `SessionSpec { sessionId, projectPath, title, model, instructions }` —
  `instructions` será preenchido pelo Context Builder (Fase 7).
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
- não há gate de autonomia na Fase 3; ele entra na Fase 9 neste mesmo ponto.

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
| `PROVIDER_SWITCHED` | `from`, `to`; `reason: "removed"` quando o ativo saiu do registro |
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

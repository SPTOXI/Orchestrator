# ADR-0009 — Camada de providers: `AIProvider`, registro e sessões

- **Estado:** Aceita
- **Fase:** 3

## Contexto

A Fase 3 pede `AIProvider`, Provider Registry e Provider Sessions, sem
conectar ainda OpenAI/Codex (Fase 4) nem Claude Code (Fase 5). O documento
mestre define a interface conceitual
(`start · resume · execute · stream · cancel · spawnAgent · inspect ·
capabilities`, seção 18) e as regras:

- o núcleo não depende de nenhum fornecedor; novos providers entram sem
  reescrever o núcleo (seções 2 e 28);
- o provider nunca executa operações do sistema: pede `tool_call` e o
  Orchestrator executa (seção 9);
- memória e histórico pertencem ao projeto, não ao provider (seções 15 e 22);
- token e custo precisam ser acompanhados (seção 23);
- observabilidade não é restrição: tudo é registrado em todos os modos.

Falta decidir onde o código fica, como um provider pede ferramentas, quem é
dono da sessão, que eventos existem e como a UI conversa com isso.

## Decisão

1. **Pacotes.** `packages/providers` vira o crate `orchestrator-providers`
   (trait, registro, sessões). Os adapters `providers/openai` e
   `providers/claude` serão crates próprios nas Fases 4 e 5, dependentes
   dele. Os contratos que a UI e o histórico também usam (ids de sessão e
   turno, `SessionEvent`, `SessionInfo`, `TokenUsage`) ficam em
   `orchestrator-core` (`session.rs`), sem I/O.
2. **Trait `AIProvider`** (async, `Send + Sync`), mapeada 1:1 na interface
   do documento, mais `descriptor()` (id, nome, fornecedor):
   - `capabilities()` — streaming, ferramentas, retomada, cancelamento,
     subagentes nativos, raciocínio, uso de tokens, custo, modelos;
   - `inspect()` — disponibilidade (instalado, versão, autenticação);
   - `start(spec)` / `resume(native, spec)` — abrem ou reabrem a sessão
     **nativa** do fornecedor (ex.: thread do Codex) e devolvem uma
     `NativeSession` opaca para o núcleo;
   - `execute(...)` — um turno completo, resultado agregado;
   - `stream(...)` — um turno com saída incremental;
   - `cancel(native)` — limpeza do lado do fornecedor;
   - `spawn_agent(parent, spec)` — sessão filha (subagente).

   Padrões: `stream` chama `execute` e entrega o texto de uma vez;
   `spawn_agent` chama `start`; `resume` responde `UNSUPPORTED`; `cancel`
   não faz nada (o cancelamento cooperativo vem pelo contexto do turno).
3. **Ferramentas passam pelo Orchestrator.** Cada turno recebe um
   `TurnContext`. A única forma de o provider agir é `ctx.call_tool(tool,
   args)`: o `SessionManager` monta o `ToolCall` com origem
   `agent { agentId, sessionId, provider }` e o entrega a um `ToolExecutor`,
   que no app é o `ToolRuntime` (mesmo `invoke` auditado usado pela UI).
   O provider não escolhe a origem. Cada chamada roda numa tarefa própria:
   mesmo com o turno cancelado, a chamada termina e o `TOOL_CALLED` é
   gravado. Na Fase 3 não há gate de autonomia; ele entra na Fase 9 no
   mesmo ponto, sem mudar providers.
4. **A sessão pertence ao Orchestrator.** `SessionManager` cria a sessão
   (`SessionId` UUID v7, provider, projeto, título, modelo, sessão pai), guarda
   a referência nativa para retomada, permite um turno por vez
   (`idle → running → idle`, ou `closed`) e soma o uso de tokens. O
   transcript é um log de `SessionEvent` numerado (`seq`), com trechos de
   texto consecutivos fundidos e limite de tamanho. Fica em memória até a
   Fase 6, quando vai para `agent_sessions` e `messages` no SQLite.
5. **Eventos.**
   - Ao vivo: `StreamEvent::Session { sessionId, seq, event }` no mesmo canal
     `runtime://stream` (texto, raciocínio, ferramenta pedida/concluída, uso,
     avisos, início/fim de turno, mudança de estado, subagente criado).
   - Histórico (novos `EventKind`): `SESSION_STARTED`, `SESSION_RESUMED`,
     `SESSION_CLOSED` e `TURN_COMPLETED` (estado, duração, ferramentas
     chamadas e uso de tokens/custo). `PROVIDER_SWITCHED`, já previsto, é
     emitido quando o provider ativo muda. O texto das mensagens fica no
     transcript da sessão, não no histórico de auditoria.
   - `AGENT_STARTED`/`AGENT_FINISHED` ficam para o Agent Manager (Fase 8).
     Até lá, cada sessão atua como o próprio agente (`agentId` = id da
     sessão).
6. **Registro.** Ids estáveis em texto (`openai-codex`, `claude-code`, …).
   Registro rejeita id duplicado. O provider ativo começa no primeiro
   registrado e é trocado explicitamente (`PROVIDER_SWITCHED`). Preferência
   por projeto fica para a Fase 6 (persistência).
7. **IPC.** Operações de provider não são ferramentas do sistema
   operacional, então não passam por `runtime_invoke`. Ganham comandos
   Tauri próprios (`providers_list`, `provider_inspect`, `provider_select`,
   `sessions_list`, `session_start`, `session_get`, `session_send`,
   `session_cancel`, `session_resume`, `session_close`, `session_spawn`),
   que só repassam ao `SessionManager`. A auditoria acontece no manager, não
   na ponte. Erros chegam estruturados (`{ kind, message }`).
8. **Cancelamento.** `session_cancel` dispara o token do turno e chama
   `provider.cancel()`. Se o provider não encerrar em 5 s, o turno é
   abandonado e registrado como `cancelled`. Ferramentas em andamento
   terminam e são registradas (item 3).
9. **Provider de desenvolvimento `echo`.** Sem IA e sem rede, existe para
   exercitar o contrato de ponta a ponta (streaming, ferramentas via
   runtime, cancelamento, falha, retomada, subagentes, uso de tokens
   estimado) em testes e no app. Comandos: `/help`, `/tool <nome> [json]`,
   `/wait <s>`, `/fail [mensagem]`; qualquer outro texto volta como eco. O
   app o registra em builds de desenvolvimento ou com
   `ORCHESTRATOR_ECHO_PROVIDER=1`; o build de release só lista providers
   reais (a partir da Fase 4).
10. **Dependências novas:** `async-trait` (trait async com `dyn`) e
    `tokio-util` (`CancellationToken`, já presente no lockfile como
    dependência transitiva).

## Consequências

- OpenAI/Codex e Claude Code entram como adapters sem tocar no núcleo, na
  UI ou no runtime: registram-se no `ProviderRegistry` e usam
  `ctx.call_tool`. Para CLIs que executam ferramentas por conta própria, o
  adapter precisa expor as ferramentas do Orchestrator (ex.: via MCP ou
  callback de permissão), decisão das Fases 4 e 5.
- Toda ação de IA aparece no histórico com a sessão e o provider de origem.
- Sessões e transcripts se perdem ao fechar o app até a Fase 6; a
  `NativeSession` já é guardada para que a retomada funcione quando houver
  persistência.
- O espelho TypeScript dos contratos cresce (`types.ts`); a regra do
  ADR-0001 sobre geração automática continua valendo.

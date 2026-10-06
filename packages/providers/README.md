# packages/providers

**AI Provider Layer** — crate `orchestrator-providers` (Fase 3,
[ADR-0009](../../docs/adr/0009-camada-de-providers-e-sessoes.md)). Referência:
[`docs/providers.md`](../../docs/providers.md).

```text
AIProvider: start · resume · execute · stream · cancel · spawnAgent · inspect · capabilities
            complete (Fase 5: resposta avulsa, sem sessão nem ferramentas; usada pelo Conselho)
            snapshot (Fase 6: estado da sessão nativa para retomar após reiniciar)
TurnInput.context (Fase 7: contexto do projeto no primeiro turno, via ContextSource)
```

| Módulo | Conteúdo |
| ------ | -------- |
| `provider.rs` | trait `AIProvider`, `ProviderDescriptor`, `ProviderCapabilities`, `ProviderStatus`, `NativeSession`, `SessionSpec`, `CompletionRequest`/`Completion` |
| `context.rs` | `TurnContext` (saída, uso, cancelamento, `call_tool`) e `ToolExecutor` |
| `registry.rs` | `ProviderRegistry` (registro, `replace`/`unregister`, provider ativo, `PROVIDER_SWITCHED`) |
| `manager.rs` | `SessionManager` (sessões, turnos, transcript, uso, cancelar, encerrar/retomar, subagentes; provider resolvido pelo registro a cada turno; contexto do projeto no primeiro turno e `annotate`) |
| `project_context.rs` | trait `ContextSource`, `ContextOptions`, `ContextRequest`, `AttachedContext` (Fase 7: quem monta o contexto é o `orchestrator-engine`) |
| `log.rs` | transcript numerado com fusão de trechos e limite; restauração a partir do banco |
| `store.rs` | trait `SessionStore` e `PersistedSession` (Fase 6: sessões e transcripts entre execuções; o app usa o banco local) e `MemorySessionStore` para testes |
| `echo.rs` | provider de desenvolvimento sem IA |

| Adapter | Fase |
| ------- | ---- |
| [`api/`](./api/README.md) — conexões de API cadastradas pelo usuário: OpenAI e compatíveis, Anthropic, Gemini e qualquer API por perfil genérico ([ADR-0010](../../docs/adr/0010-providers-por-api-com-cadastro-livre.md)) | 4 |

Regras:

- Nenhuma chamada específica de fornecedor fora do adapter correspondente.
- Providers nunca executam operações de sistema: pedem ferramentas com
  `TurnContext::call_tool` e o Orchestrator executa via Tool Runtime.
- Uma IA nova entra como conexão cadastrada (sem código) ou como protocolo
  novo em `api/`, sem alterar o núcleo.

Testes: `cargo test -p orchestrator-providers` (inclui o contrato de ponta a
ponta com o Tool Runtime real em `tests/sessions.rs`).

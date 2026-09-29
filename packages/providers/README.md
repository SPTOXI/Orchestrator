# packages/providers

**AI Provider Layer** — crate `orchestrator-providers` (Fase 3,
[ADR-0009](../../docs/adr/0009-camada-de-providers-e-sessoes.md)). Referência:
[`docs/providers.md`](../../docs/providers.md).

```text
AIProvider: start · resume · execute · stream · cancel · spawnAgent · inspect · capabilities
```

| Módulo | Conteúdo |
| ------ | -------- |
| `provider.rs` | trait `AIProvider`, `ProviderDescriptor`, `ProviderCapabilities`, `ProviderStatus`, `NativeSession`, `SessionSpec` |
| `context.rs` | `TurnContext` (saída, uso, cancelamento, `call_tool`) e `ToolExecutor` |
| `registry.rs` | `ProviderRegistry` (registro, provider ativo, `PROVIDER_SWITCHED`) |
| `manager.rs` | `SessionManager` (sessões, turnos, transcript, uso, cancelar, encerrar/retomar, subagentes) |
| `log.rs` | transcript numerado com fusão de trechos e limite |
| `echo.rs` | provider de desenvolvimento sem IA |

| Adapter | Fase |
| ------- | ---- |
| `openai/` — OpenAI / Codex | 4 |
| `claude/` — Claude Code | 5 |

Regras:

- Nenhuma chamada específica de fornecedor fora do adapter correspondente.
- Providers nunca executam operações de sistema: pedem ferramentas com
  `TurnContext::call_tool` e o Orchestrator executa via Tool Runtime.
- Novos providers (Gemini, modelos locais) entram como novos crates sem
  alterar o núcleo.

Testes: `cargo test -p orchestrator-providers` (inclui o contrato de ponta a
ponta com o Tool Runtime real em `tests/sessions.rs`).

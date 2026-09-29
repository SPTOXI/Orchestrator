# packages/providers/api

**Conexões de API** — crate `orchestrator-provider-api` (Fase 4,
[ADR-0010](../../../docs/adr/0010-providers-por-api-com-cadastro-livre.md)).
Referência: [`docs/api-connections.md`](../../../docs/api-connections.md).

Cada conexão cadastrada pelo usuário vira um `ApiProvider` no
`ProviderRegistry`. Os tipos são `openai` (e compatíveis), `anthropic`,
`gemini` e `generic`, este último para qualquer API HTTP/JSON descrita por
perfil.

| Módulo | Conteúdo |
| ------ | -------- |
| `config.rs` | `Connection`, `ModelEntry`, credencial, modo de ferramentas, perfil genérico, validação |
| `manager.rs` | `ConnectionManager`: `connections.json`, cofre, registro, `CONNECTION_*`, teste e descoberta |
| `provider.rs` | `ApiProvider` (`AIProvider`): loop modelo → ferramentas → modelo, custo, teste de conexão |
| `protocol.rs` | trait `Protocol` (requisição, decodificador, descoberta de modelos) |
| `openai.rs`, `anthropic.rs`, `gemini.rs`, `generic.rs` | um módulo por protocolo |
| `tools.rs` | nomes de ferramentas, protocolo por prompt (`<tool_call>`), `MarkupFilter`, schema do Gemini |
| `conversation.rs` | histórico enviado à API (append-only, partes nativas preservadas) |
| `http.rs` | cliente `reqwest`, leitura de SSE/NDJSON, erros HTTP |
| `jsonpath.rs` | caminhos com pontos e modelo de corpo do perfil genérico |
| `secrets.rs` | trait `SecretStore` (o app usa o cofre do SO) e cofre em memória para testes |
| `presets.rs` | pontos de partida da tela "Adicionar API" |

Regras:

- A chave nunca é gravada em arquivo, evento ou log, e nunca volta para a
  UI.
- O modelo só **pede** ferramentas; quem executa é o Orchestrator
  (`ctx.call_tool`).

Testes: `cargo test -p orchestrator-provider-api`. Os testes de integração
(`tests/api.rs`) usam um servidor HTTP falso local que imita cada protocolo.

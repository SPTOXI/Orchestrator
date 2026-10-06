# Conexões de API (Fase 4)

Referência de `packages/providers/api` (crate `orchestrator-provider-api`).
Decisão registrada em
[ADR-0010](./adr/0010-providers-por-api-com-cadastro-livre.md).

O usuário cadastra **quantas APIs quiser**. Cada conexão salva e ativa vira
um provider no `ProviderRegistry` ([providers.md](./providers.md)), com o
**id da conexão** como `ProviderId`.

```text
AI PROVIDERS ─▶ ConnectionManager ──▶ connections.json (sem segredos)
 (UI)             │      │
                  │      └──▶ SecretStore: cofre do SO (a chave nunca sai dele)
                  ▼
            ProviderRegistry ◀── ApiProvider (1 por conexão ativa)
                  ▲                    │  HTTP (reqwest, rustls)
SessionManager ───┘                    ▼
  turno: modelo → tool_call → Tool Runtime → resultado → modelo … → resposta
```

O modelo **nunca** executa nada: ele pede uma ferramenta, e o Orchestrator a
executa com `ctx.call_tool`, que grava `TOOL_CALLED` com a origem
`agent { sessionId, provider }`.

## Tipos e modelos prontos

| Tipo (`kind`) | Protocolo | Autenticação |
| ------------- | --------- | ------------ |
| `openai` | Chat Completions (`POST {base}/chat/completions`, SSE). Serve para qualquer API compatível | `Authorization: Bearer <chave>` |
| `anthropic` | Messages API (`POST {base}/messages`, SSE) | `x-api-key`, `anthropic-version: 2023-06-01` |
| `gemini` | `POST {base}/models/{modelo}:streamGenerateContent?alt=sse` | `x-goog-api-key` |
| `generic` | **Qualquer API HTTP/JSON**, descrita por um perfil (abaixo) | bearer, header, parâmetro na URL ou nenhuma |

Pontos de partida ("Adicionar API"), todos editáveis:

| Preset | Tipo | URL base |
| ------ | ---- | -------- |
| OpenAI | `openai` | `https://api.openai.com/v1` |
| Anthropic (Claude) | `anthropic` | `https://api.anthropic.com/v1` (modelos atuais já listados, com preços de referência) |
| Google Gemini | `gemini` | `https://generativelanguage.googleapis.com/v1beta` |
| OpenRouter | `openai` | `https://openrouter.ai/api/v1` (a descoberta traz preços) |
| Compatível com OpenAI | `openai` | a preencher: DeepSeek, Groq, Mistral, xAI, Together, vLLM, LM Studio… |
| Ollama local (compatível) | `openai` | `http://localhost:11434/v1`, sem chave |
| Ollama (API nativa) | `generic` | `http://localhost:11434`: exemplo de perfil (`/api/chat` em NDJSON) |
| Qualquer API | `generic` | perfil em branco |

Pode haver várias conexões do mesmo tipo (ex.: duas contas OpenAI, três
servidores locais).

## A conexão

```json
{
  "id": "openai-pessoal",
  "name": "OpenAI pessoal",
  "kind": "openai",
  "baseUrl": "https://api.openai.com/v1",
  "credential": { "source": "vault", "envVar": null },
  "headers": { "OpenAI-Organization": "org-…" },
  "extraBody": { "temperature": 0.2 },
  "models": [
    {
      "id": "gpt-…", "name": null, "contextWindow": 400000, "maxOutputTokens": null,
      "supportsTools": true, "supportsVision": null,
      "inputPrice": 1.25, "outputPrice": 10, "cachedInputPrice": 0.125, "tags": ["código"],
      "extraBody": null, "enabled": true
    }
  ],
  "defaultModel": "gpt-…",
  "toolMode": null,
  "maxToolRounds": 50,
  "maxOutputTokens": null,
  "options": { "streamUsage": null, "eagerToolStreaming": null, "refusalFallback": null,
               "promptCache": null, "cacheTtl": null },
  "generic": null,
  "enabled": true,
  "notes": null
}
```

| Campo | Regra |
| ----- | ----- |
| `id` | 1–48 caracteres: letras minúsculas, dígitos e `-`. Vira o id do provider; renomear = outro provider |
| `baseUrl` | `http://` ou `https://` |
| `headers` | headers extras; um header do usuário **substitui** o do protocolo com o mesmo nome |
| `extraBody` / `models[].extraBody` | objeto JSON mesclado no corpo de cada requisição (o do modelo por último) |
| `models` | lista editável; só os `enabled` aparecem na sessão. Sem modelo ativo a conexão não abre sessões |
| `defaultModel` | modelo das sessões novas (padrão: o primeiro ativo) |
| `toolMode` | `native`, `prompt` ou `none`; `null` = `native`, ou `prompt` no tipo `generic` |
| `maxToolRounds` | rodadas modelo → ferramentas por turno (padrão 50) |
| `maxOutputTokens` | limite de saída; na Anthropic o padrão é 64000 com streaming e 16000 sem |
| `enabled` | desativada = fica salva, mas sai do registro |
| `models[].cachedInputPrice` | US$ por milhão de tokens lidos do cache (Fase 11); sem ele, a leitura custa a entrada cheia (Anthropic: 10% da entrada) |
| `options.promptCache` | cache de prompt (`null` = ligado): marcadores `cache_control` na Anthropic, `prompt_cache_key` na OpenAI oficial |
| `options.cacheTtl` | `5m` (padrão) ou `1h`, validade do cache na Anthropic |

## Credenciais

| `credential.source` | De onde vem a chave |
| ------------------- | ------------------- |
| `vault` | **Cofre do sistema** (crate `keyring`): Gerenciador de Credenciais do Windows, Keychain do macOS, Secret Service no Linux (GNOME Keyring, KWallet). Serviço `dev.orchestrator.desktop`, conta `connection:<id>` |
| `env` | variável de ambiente indicada em `envVar` (lida a cada uso) |
| `none` | sem chave (servidor local) |

- A chave digitada vai direto para o cofre ao salvar. Ela **nunca** é
  gravada em `connections.json`, em eventos do histórico ou em logs, e
  **nunca** volta para a UI: a UI só recebe
  `KeyStatus { source, present, detail }`.
- Mensagens de erro de HTTP não incluem a URL (uma chave na query não vaza).
- Renomear o id move a chave no cofre; remover a conexão apaga a chave.
- O cofre pode estar indisponível no Linux sem Secret Service. Nesse caso a
  UI mostra o erro e sugere a variável de ambiente.
- Uma configuração pode ser testada com uma chave digitada antes de salvar.
  Essa chave é usada só naquela chamada.

## Modelos

"Buscar modelos" consulta a API e **mescla** o resultado na lista:

- o que o usuário editou prevalece;
- metadados ausentes (nome, contexto, preços) são preenchidos;
- modelos novos entram desativados, exceto na primeira busca com até 8
  modelos.

| Tipo | Descoberta |
| ---- | ---------- |
| `openai` | `GET {base}/models`. No OpenRouter, também preços e contexto |
| `anthropic` | `GET {base}/models` (paginado), completado com a tabela de referência (contexto e preços) |
| `gemini` | `GET {base}/models` (paginado por `pageToken`), só modelos com `generateContent` |
| `generic` | `generic.modelsPath` + `modelsListPath` + `modelIdField`, se configurados |

Cada modelo guarda contexto, limite de saída, suporte a ferramentas e visão,
preços e **etiquetas livres** ("código", "barato", "raciocínio"…). O
roteador e o Conselho usam esses dados para escolher o modelo de cada tarefa
([router.md](./router.md)): quanto mais completos, melhor a escolha.

## Ferramentas

O catálogo do Tool Runtime (`ToolRuntime::definitions()`) vai para o modelo
com o JSON Schema dos argumentos de cada ferramenta
([tool-runtime.md](./tool-runtime.md#schemas-dos-argumentos)).

| `toolMode` | Como o modelo pede | Para |
| ---------- | ------------------ | ---- |
| `native` | chamada de funções do protocolo (`tools`/`tool_calls`, `tool_use`, `functionCall`) | OpenAI e compatíveis, Anthropic, Gemini |
| `prompt` | as ferramentas são descritas no prompt de sistema e o modelo escreve `<tool_call>{"tool": "filesystem.read", "args": {"path": "README.md"}}</tool_call>`. Os resultados voltam como `<tool_result tool="…" ok="true">…</tool_result>` | **qualquer** modelo de texto; padrão do `generic` |
| `none` | não pede; só conversa | — |

- Nomes com ponto viram `grupo__acao` nos protocolos que não aceitam ponto
  (`filesystem.read` ↔ `filesystem__read`).
- **O que o modelo pode fazer:** com ferramentas, as instruções de sistema
  ganham um parágrafo montado a partir dos nomes delas (arquivos, comandos,
  git, GitHub com a conta conectada, memória, subagentes). Ele diz ao
  modelo para usar as ferramentas em vez de responder "não tenho acesso",
  que não há navegador nem login em sites, e que, faltando um acesso, deve
  dizer qual e como o usuário o habilita (no GitHub: painel GIT →
  "Conectar ao GitHub"). O texto só depende das ferramentas, então não
  muda dentro da sessão (o cache de prompt continua valendo).
- Os blocos `<tool_call>` não aparecem no texto mostrado ao usuário.
- **Loop do turno:** modelo → ferramentas pedidas (em ordem, pelo
  Orchestrator) → resultados → modelo… até uma resposta sem pedidos.
- **Contexto do projeto** (Fase 7): no primeiro turno, o texto do Context
  Builder é acrescentado às instruções de sistema da conversa e fica nelas
  nas requisições seguintes (e na persistência da sessão). O catálogo
  inclui as ferramentas de memória (`memory.*`, `decision.*`); ver
  [context.md](./context.md).
  - Ao atingir `maxToolRounds`, o turno termina com um aviso no
    transcript. O limite existe contra laços infinitos e gasto de tokens.
    Não é uma restrição de operações: nenhuma ferramenta é bloqueada.
- Argumentos inválidos ou truncados **não** são executados: o modelo
  recebe `INVALID_ARGS: …` como resultado e pode corrigir.
- Um resultado maior que 40.000 caracteres é cortado com uma nota pedindo
  menos (ex.: `maxBytes`).
- Na Anthropic, os argumentos das ferramentas chegam em streaming
  (`eager_input_streaming`, opção `eagerToolStreaming`).

## Detalhes por protocolo

A conversa enviada à API é **só acrescentada** (append-only): o conteúdo de
cada resposta do assistente é guardado como veio e reenviado sem mudança.

- **OpenAI e compatíveis**
  - streaming com `stream_options.include_usage` (opção `streamUsage`;
    desligue para servidores que recusam o campo);
  - `max_completion_tokens` quando há limite;
  - deltas `reasoning_content`/`reasoning` viram raciocínio no transcript.
- **Anthropic**
  - blocos `thinking` (com assinatura) e `tool_use` voltam intactos;
  - `stop_reason: "refusal"` é tratado antes de usar a resposta: o turno
    falha com a categoria da recusa e nenhuma ferramenta é executada;
  - na API oficial, os modelos que aceitam recebem `fallbacks: "default"`
    (beta `server-side-fallback-2026-07-01`): se o modelo recusar, outro
    continua a resposta e o transcript registra o aviso. Opção
    `refusalFallback` desliga. Por proxy (outra URL base) o campo não é
    enviado.
- **Gemini**
  - `functionCall`/`functionResponse` com `id`;
  - partes com `thoughtSignature` voltam intactas;
  - os schemas das ferramentas são convertidos para o subconjunto OpenAPI
    que a API aceita;
  - `finishReason` de segurança vira recusa e `MAX_TOKENS` vira aviso;
  - 400 com `API_KEY_INVALID` é tratado como erro de autenticação.

## Perfil genérico

Descreve uma API sem código. Caminhos usam pontos e índices
(`choices.0.message.content`).

| Campo | Significado |
| ----- | ----------- |
| `path` | acrescentado à URL base; pode conter `{{model}}` |
| `auth` | `{"type": "bearer"}`, `{"type": "header", "name": "X-Key", "prefix": ""}`, `{"type": "query", "param": "key"}` ou `{"type": "none"}` |
| `messageFormat` | `chat` (lista `[{role, content}]`, com nomes de papel em `roles`) ou `prompt` (um texto só com a conversa) |
| `body` | modelo do corpo. Uma string que seja exatamente `{{messages}}`, `{{stream}}` ou `{{maxTokens}}` vira esse valor JSON; `{{model}}`, `{{system}}` e `{{prompt}}` são substituídos dentro de qualquer string |
| `stream` | `none` (uma resposta JSON), `sse` ou `ndjson` |
| `textPath` | texto da resposta (ou de cada pedaço do stream) — obrigatório |
| `donePath` / `doneMarker` | fim do stream: campo `true` (NDJSON) ou valor de `data` (SSE, ex.: `[DONE]`) |
| `inputTokensPath` / `outputTokensPath` | uso de tokens |
| `errorPath` | mensagem de erro no corpo |
| `modelsPath` / `modelsListPath` / `modelIdField` | descoberta de modelos (opcional) |

Exemplo (preset "Ollama, API nativa"):

```json
{
  "path": "/api/chat",
  "auth": { "type": "none" },
  "messageFormat": "chat",
  "body": { "model": "{{model}}", "messages": "{{messages}}", "stream": "{{stream}}" },
  "stream": "ndjson",
  "textPath": "message.content",
  "donePath": "done",
  "inputTokensPath": "prompt_eval_count",
  "outputTokensPath": "eval_count",
  "errorPath": "error",
  "modelsPath": "/api/tags",
  "modelsListPath": "models",
  "modelIdField": "name"
}
```

O tipo `generic` usa `toolMode` `prompt` ou `none`: a chamada de funções
nativa é específica de cada protocolo.

## Uso e custo

- O uso de tokens vem da resposta de cada API e é somado por turno e por
  sessão.
- **Custo** = tokens de entrada × preço de entrada + tokens de saída × preço
  de saída, com preços em US$ por milhão de tokens informados no modelo —
  e, desde a Fase 11, com o cache: tokens lidos do cache a
  `cachedInputPrice` e gravações (Anthropic) a 1,25× ou 2× a entrada
  ([tokens.md](./tokens.md#custo-real)). As APIs não informam preço; sem
  preço, o custo não aparece.
- A tabela de referência da Anthropic (setembro de 2026) vem preenchida no
  preset:

  | Modelo | Contexto | US$ entrada / saída / cache por 1M |
  | ------ | -------- | ---------------------------------- |
  | `claude-opus-5-5` | 1M | 4 / 20 / 0,20 |
  | `claude-sonnet-5-5` | 1M | 2 / 10 / 0,20 |
  | `claude-haiku-4-5` | 200K | 1 / 5 / 0,10 |
  | `claude-fable-5-1` | 1M | 10 / 50 / 0,25 |

  "Buscar modelos" completa o preço do cache dos modelos que ainda não o
  têm.

## Testar conexão

Funciona com a configuração do formulário, salva ou não. Faz duas chamadas
curtas:

1. pede a resposta "OK";
2. se o modo de ferramentas não for `none`, oferece a ferramenta
   `orchestrator.ping` e pede que o modelo a chame. A ferramenta **nunca é
   executada**: o teste só verifica se o pedido veio bem formado.

O relatório (`TestReport`) traz:

- `ok`, `model`, `servedModel`, `latencyMs`, `reply` e `usage`;
- `tools`: `passed`, `noCall`, `failed` ou `notTested`;
- `toolsDetail` e `error`.

## Erros

| HTTP | Tipo | Mensagem |
| ---- | ---- | -------- |
| 401, 403 (e 400 `API_KEY_INVALID` do Gemini) | `UNAVAILABLE` | `authentication rejected — check the API key (HTTP …): <mensagem da API>` |
| 402 | `FAILED` | `insufficient credit or spending limit reached` |
| 404 | `INVALID_REQUEST` | `not found — check the base URL and the model` |
| 400, 409, 413, 422 | `INVALID_REQUEST` | `request rejected` |
| 429 | `FAILED` | `rate limit or quota exceeded` |
| 5xx | `FAILED` | `server error` |
| sem conexão | `UNAVAILABLE` | `cannot reach <host>` |

Desde a Fase 11, 408, 429, 500, 502, 503, 504, 529 e falhas de conexão
são repetidos até duas vezes antes de virar erro, com a espera do
`retry-after` (até 60 s) ou 2 s e 4 s; a sessão mostra cada tentativa
([tokens.md](./tokens.md#retentativas)). Um 402 que diz quantos tokens o
crédito paga (OpenRouter) é repetido na hora pedindo uma resposta menor.

Um stream parado por 300 s falha o turno. A conexão TCP tem 20 s para abrir.
O cancelamento interrompe a requisição em andamento.

### Servidor que não começa a responder e conexão reserva

Servidores sobrecarregados (o da DeepSeek nos horários de pico, por
exemplo) aceitam o pedido, mandam só comentários `: keep-alive` e, depois
de 15 minutos, respondem "unable to start processing your request within
the 900-second timeout limit". Por isso:

- **`firstResponseSecs`** (padrão 120; 0 desliga): um pedido com streaming
  que não recebe nenhum dado nesse tempo — cabeçalhos ou o primeiro evento;
  `keep-alive` não conta — é abandonado, sem repetir na mesma conexão. Sem
  streaming não há limite (a resposta inteira vem de uma vez).
- **`fallback: {connection, model?}`**: quando o pedido falha por servidor
  sobrecarregado ou fora do ar — o limite acima, 408, 429, 5xx, 529,
  conexão recusada, stream parado, "overloaded" ou "unable to start
  processing", depois das retentativas normais —, o mesmo pedido vai para a
  outra conexão: com `model`, o mesmo id de modelo se ela o tiver, ou o
  padrão dela. Vale para turnos de sessões e agentes, Conselho e
  compactação; "Testar conexão" nunca usa a reserva. A sessão avisa
  ("o servidor de DeepSeek está sobrecarregado … Continuando com a conexão
  reserva …"), o custo usa os preços da reserva e o que é próprio de um
  protocolo (`native`) não vai para outro. Só um pedido que ainda não
  mostrou nada muda de conexão; com ferramentas nativas, só para uma
  reserva com ferramentas nativas. A reserva não tem reserva.
- Sem reserva, o erro diz em português que o servidor está sobrecarregado e
  o que fazer; na sessão, o último turno que falhou tem **Tentar de novo**.

Na tela: seção "Quando o servidor não responder" da conexão.

## Sessões e edição de conexões

A sessão guarda o id do provider e usa a instância registrada **no início
de cada turno** (também ao retomar e ao criar subagente):

- **editar** a conexão (chave, URL, headers, modelos) vale para as sessões
  abertas a partir do próximo turno. A conversa continua: as instâncias de
  uma mesma conexão compartilham o histórico enviado à API;
- **desativar, renomear ou remover**: o próximo turno falha com
  `provider … is no longer registered` e nada é enviado. A sessão continua
  listada, com aviso na UI. Se a conexão for reativada, a sessão volta a
  funcionar com a conversa.

A conversa também vai para o banco local ao fim de cada turno
(`AIProvider::snapshot`, Fase 6), com as partes nativas, como as
assinaturas de raciocínio. Depois de reiniciar o app, "Retomar" continua a
mesma conversa: a próxima requisição leva todas as mensagens anteriores.
A chave da API nunca vai para o banco.

## Persistência e histórico

- `<app-data>/connections.json` (`{ "version": 1, "connections": [...] }`).
  É gravado de forma atômica (arquivo temporário + rename) e não contém
  segredos. Uma entrada inválida é ignorada com aviso; ela nunca impede o
  app de abrir.
- Eventos (`AuditEvent`):

  | Evento | `data` |
  | ------ | ------ |
  | `CONNECTION_SAVED` | `id`, `name`, `kind`, `baseUrl`, `credential` (a origem, nunca a chave), `models`, `enabled`, `created`, `renamedFrom` |
  | `CONNECTION_REMOVED` | `id`, `name`, `kind`, `vaultError` |

  Salvar ou remover também pode gerar `PROVIDER_SWITCHED` (`reason:
  "removed"`) quando o provider ativo sai do registro.

## IPC

`connections_list`, `connection_save`, `connection_delete`,
`connection_test` e `connection_models`: ver [ipc.md](./ipc.md).

## Testes

- `packages/providers/api/tests/api.rs` usa um servidor HTTP falso local
  que imita cada protocolo. Os testes cobrem:
  - streaming e ferramentas executadas pelo Tool Runtime real;
  - blocos de raciocínio da Anthropic e assinaturas do Gemini;
  - recusa, protocolo por prompt, erros HTTP e ausência de rede;
  - credenciais ausentes, descoberta de modelos e persistência sem chaves;
  - teste de conexão, limite de rodadas e argumentos inválidos;
  - cancelamento no meio do stream;
  - edição e remoção com sessão aberta.
- Testes unitários nos módulos: decodificação de cada protocolo, fallback
  da Anthropic, perfil genérico, caminhos, `MarkupFilter` e erros.

# Fase 11 — Tokens, cache, compactação de contexto e escalonamento de agentes

## STATUS

🚧 **Em andamento.** Decisão em
[ADR-0018](../adr/0018-tokens-cache-compactacao-e-escalonamento.md).

## Andamento

A fase é feita em passos, cada um commitado e enviado ao terminar, para
poder ser retomada de onde parou.

1. ✅ ADR-0018 e este arquivo.
2. ✅ Custo real e cache de prompt: `TokenUsage` (gravação no cache,
   economia), preço do cache por modelo, marcadores `cache_control`
   (Anthropic), `prompt_cache_key` (OpenAI), retentativas; testes.
3. ⏳ Compactação de contexto: adapter das APIs, `SessionManager`,
   `CONTEXT_COMPACTED`, `context.json`; testes.
4. ⏳ Escalonamento de agentes: ordem da fila, limite por provider, tetos
   de custo, orçamento diário, `maxSubagents`; testes.
5. ⏳ Supervisão de processos: Job Object (Windows) e processos órfãos
   (Linux/macOS); testes.
6. ⏳ Desktop e UI: sessão, conexão, contexto, agentes, aba "Tokens e
   custo".
7. ⏳ Validação no app real, documentação e publicação.

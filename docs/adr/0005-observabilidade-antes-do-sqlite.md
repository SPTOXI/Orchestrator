# ADR-0005 — Observabilidade antes do SQLite (JSONL + eventos)

- **Estado:** Aceita
- **Fase:** 1

## Contexto

O documento mestre exige que logs, histórico, eventos de auditoria, saída de
terminal e tool calls sejam registrados sempre — inclusive em Acesso
Irrestrito. O SQLite só chega na Fase 6, mas o Tool Runtime já executa
operações reais na Fase 1.

## Decisão

1. `packages/core` define `AuditEvent` e o trait `EventSink`. O runtime emite
   eventos sem saber quem os consome.
2. Na Fase 1 o sink do desktop:
   - publica cada evento na UI (`runtime://audit`, painel HISTORY);
   - mantém os 1000 eventos mais recentes em memória (`history_recent`);
   - grava cada evento como uma linha JSON em `<app-data>/audit.jsonl`
     (append + flush por evento).
3. Todo `ToolCall` gera `TOOL_CALLED` com argumentos, resultado (ok/erro) e
   duração. Strings longas nos argumentos (ex.: conteúdo de `filesystem.write`)
   são resumidas (`…(+N bytes)`) para o log não duplicar arquivos.
4. Eventos de domínio adicionais: `FILE_CHANGED` (write/move/delete) e
   `COMMAND_EXECUTED` (`shell.execute`, `process.start`).
5. Dois tipos de evento além da lista da seção 22, necessários para
   diagnóstico e continuidade: `PROCESS_EXITED` (processo gerenciado terminou,
   com exit code) e `TERMINAL_EXITED` (o shell de um terminal terminou).
6. Na Fase 6, um sink SQLite substitui o JSONL (tabela `audit_events`); o
   formato do evento não muda.

## Consequências

- Nenhuma operação do runtime deixa de ser registrada.
- O arquivo JSONL cresce sem rotação na Fase 1 (limitação conhecida, resolvida
  com o SQLite).

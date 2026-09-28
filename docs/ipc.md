# Camada IPC (React ↔ Tauri ↔ Rust)

```text
React (apps/desktop/src)
   │  invoke(...)                       ▲ listen("runtime://stream" | "runtime://audit")
   ▼                                    │
Tauri (apps/desktop/src-tauri) ── ponte, sem lógica de domínio
   │  ToolRuntime::invoke(ToolCall)     ▲ EventSink (DesktopSink)
   ▼                                    │
Rust Runtime (packages/runtime) ──▶ Orchestrator Core (packages/core)
```

Decisão registrada em [ADR-0003](./adr/0003-gateway-ipc-unico.md).

## Comandos

| Comando | Argumentos | Retorno | Auditado |
| ------- | ---------- | ------- | -------- |
| `runtime_invoke` | `tool: string`, `args?: object` | `ToolResult` | sim (`TOOL_CALLED` + eventos de domínio) |
| `runtime_tools` | — | `ToolSpec[]` | não (somente leitura do catálogo) |
| `terminal_input` | `id: string`, `data: string` | `void` | não por tecla (ADR-0003) |
| `terminal_resize` | `id: string`, `cols: number`, `rows: number` | `void` | não |
| `history_recent` | `limit?: number` (padrão 200) | `AuditEvent[]` (mais antigo primeiro) | não |
| `app_info` | — | `{ version, os, arch, baseDir, dataDir, auditLog, defaultShell }` | não |

`runtime_invoke` sempre resolve com um `ToolResult` (falhas vêm em
`ok: false` + `error`); a UI sempre chama com `origin = user`. O cliente
tipado está em `apps/desktop/src/lib/runtime.ts`.

## Eventos

| Evento | Payload | Uso |
| ------ | ------- | --- |
| `runtime://stream` | `StreamEvent` | saída de terminal/processo ao vivo, término |
| `runtime://audit` | `AuditEvent` | painel HISTORY, atualização de listas e do explorer |

`StreamEvent`:

```ts
| { type: "terminalOutput"; terminalId; offset; data }
| { type: "terminalExited"; terminalId; exitCode }
| { type: "processOutput"; processId; stream: "stdout" | "stderr"; offset; data }
| { type: "processExited"; processId; exitCode; stopped }
```

`offset` é a posição (bytes UTF-8) de `data` no fluxo completo — a mesma
coordenada de `since`/`next` em `terminal.read`/`process.read`.

## Sincronizando buffer e eventos sem perder saída

Ao abrir a visualização de um terminal/processo que já está rodando
(`apps/desktop/src/lib/outputSync.ts`):

1. inscrever-se em `runtime://stream` e **enfileirar** os eventos;
2. aguardar o listener Tauri estar registrado;
3. ler o buffer (`terminal.read`/`process.read`) → `data`, `next`;
4. escrever `data`; descartar da fila os eventos com `offset < next`;
5. a partir daí, escrever cada evento com `offset >= next` e avançar.

## Persistência

`DesktopSink` grava cada `AuditEvent` como uma linha JSON em
`<app-data>/audit.jsonl` (Linux: `~/.local/share/dev.orchestrator.desktop/`,
Windows: `%APPDATA%\dev.orchestrator.desktop\`, macOS:
`~/Library/Application Support/dev.orchestrator.desktop/`) e mantém os 1000
mais recentes em memória. Substituído por SQLite na Fase 6
([ADR-0005](./adr/0005-observabilidade-antes-do-sqlite.md)).

## Segurança da webview

- Capability `default`: apenas `core:default` (eventos, janela) e os comandos
  acima. Nenhum plugin com acesso ao SO é exposto à webview: todo acesso passa
  pelo runtime.
- CSP: `default-src 'self' ipc: http://ipc.localhost`; `style-src 'self'
  'unsafe-inline'` com `dangerousDisableAssetCspModification: ["style-src"]`,
  necessário porque o xterm.js injeta elementos `<style>` (com nonce, o
  navegador ignoraria `'unsafe-inline'`).

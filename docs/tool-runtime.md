# Tool Runtime — referência

O Tool Runtime (`packages/runtime`, crate `orchestrator-runtime`) é o único
componente que executa operações no sistema operacional. Toda chamada entra por
`ToolRuntime::invoke(ToolCall) -> ToolResult` — pela UI hoje (via
`runtime_invoke`) e pelos agentes de IA a partir da Fase 3.

## Contrato

```jsonc
// ToolCall
{ "id": "01J…", "tool": "filesystem.read", "args": { "path": "src/main.rs" },
  "origin": { "type": "user" } }          // | { "type": "agent", "agentId": "…" } | { "type": "system" }

// ToolResult
{ "callId": "01J…", "tool": "filesystem.read", "ok": true,
  "output": { … },                         // null quando ok = false
  "error": null,                           // { "kind": "NOT_FOUND", "message": "…" }
  "startedAt": "…", "finishedAt": "…", "durationMs": 3 }
```

- JSON em `camelCase`. Campos desconhecidos em `args` são rejeitados
  (`INVALID_ARGS`) para que erros de digitação de uma IA não passem em silêncio.
- `args: null` equivale a `{}`.
- Caminhos relativos são resolvidos a partir do diretório base do runtime
  (home do usuário na Fase 1; raiz do projeto a partir da Fase 2). `~` e `~/…`
  são expandidos.
- Erros (`error.kind`): `UNKNOWN_TOOL`, `INVALID_ARGS`, `NOT_FOUND`,
  `ALREADY_EXISTS`, `PERMISSION_DENIED` (negado pelo SO), `IO`, `SPAWN`,
  `NOT_RUNNING`, `INTERNAL`.
- Não existe lista de comandos proibidos nem confirmação oculta no runtime. O
  gate de autonomia (Fase 9) ficará explicitamente na frente de `invoke`.

## Eventos

Toda chamada gera `TOOL_CALLED` (`data`: `tool`, `args` resumidos, `ok`,
`error`, `durationMs`), com `callId` para correlação. Eventos adicionais:

| Evento | Quando | `data` |
| ------ | ------ | ------ |
| `FILE_CHANGED` | `filesystem.write/move/delete` com sucesso | `change` (`created`/`modified`/`moved`/`deleted`), `path` ou `from`/`to` |
| `COMMAND_EXECUTED` | `shell.execute` terminou; `process.start` iniciou | `command`, `shell`, `cwd`, `exitCode`, `timedOut`, `durationMs`, `stdoutTail`, `stderrTail` (últimos 4 KiB) / `processId`, `pid`, `background` |
| `PROCESS_EXITED` | processo gerenciado terminou | `processId`, `command`, `exitCode`, `stopped` |
| `TERMINAL_EXITED` | shell de um terminal terminou | `terminalId`, `shell`, `exitCode`, `closed` |

Strings com mais de 512 bytes em `args` são resumidas no evento
(`"…(+N bytes)"`). Eventos de streaming (`StreamEvent`) não são duráveis: veja
[`ipc.md`](./ipc.md).

---

## filesystem

### `filesystem.list`

| Arg | Tipo | |
| --- | ---- | - |
| `path` | string | diretório |

Saída: `{ path, entries: [{ name, path, kind: "file"|"directory"|"other", isSymlink, size, modifiedAt }] }`
— diretórios primeiro, depois arquivos, ordem alfabética sem distinção de caixa.

### `filesystem.read`

| Arg | Tipo | Padrão | |
| --- | ---- | ------ | - |
| `path` | string | | arquivo |
| `encoding` | `"utf8"` \| `"base64"` | `utf8` | `utf8` cai para `base64` se o conteúdo for binário |
| `maxBytes` | number | arquivo inteiro | lê só o início |

Saída: `{ path, content, encoding, size, truncated }`. Um caractere UTF-8
cortado pelo limite é descartado, em vez de tratar o arquivo como binário.

### `filesystem.write`

| Arg | Tipo | Padrão |
| --- | ---- | ------ |
| `path` | string | |
| `content` | string | |
| `encoding` | `"utf8"` \| `"base64"` | `utf8` |
| `createDirs` | boolean | `true` |
| `append` | boolean | `false` |

Saída: `{ path, bytesWritten, created }`.

### `filesystem.move`

| Arg | Tipo | Padrão | |
| --- | ---- | ------ | - |
| `from`, `to` | string | | |
| `overwrite` | boolean | `false` | sem ele, destino existente → `ALREADY_EXISTS` |

Cria os diretórios do destino. Entre dispositivos diferentes, copia e remove.
Saída: `{ from, to, replaced }`.

### `filesystem.delete`

| Arg | Tipo | Padrão | |
| --- | ---- | ------ | - |
| `path` | string | | |
| `recursive` | boolean | `false` | necessário para diretório não vazio |

Saída: `{ path, kind }`. Links simbólicos são removidos sem seguir o alvo.

---

## shell

### `shell.list`

Saída: `{ default, shells: [{ id, kind, name, path }] }`.

| SO | Shells detectados (`id`) | Padrão |
| -- | ------------------------ | ------ |
| Windows | `pwsh`, `powershell`, `cmd`, `wsl`, `git-bash` | `pwsh` → `powershell` → `cmd` |
| Linux/macOS | `bash`, `zsh`, `fish`, `sh`, `pwsh` | `$SHELL` → `bash` → `zsh` → `sh` |

O argumento `shell` das outras ferramentas aceita um `id`, um `kind`
(`bash`, `powerShell`, …) ou o caminho de um executável de shell.

### `shell.execute`

| Arg | Tipo | Padrão | |
| --- | ---- | ------ | - |
| `command` | string | | |
| `cwd` | string | diretório base | |
| `shell` | string | shell padrão | |
| `env` | objeto | `{}` | variáveis adicionais |
| `timeoutMs` | number | sem timeout | ao estourar, a árvore do processo é encerrada |
| `stdin` | string | stdin vazio | |
| `maxOutputBytes` | number | 4 MiB | por stream |

Saída: `{ command, shell, cwd, exitCode, stdout, stderr, stdoutTruncated, stderrTruncated, timedOut, durationMs }`.
Exit code ≠ 0 **não** é erro da ferramenta. `exitCode` é `null` em timeout ou
término por sinal.

Como o comando é executado:

| Shell | Invocação |
| ----- | --------- |
| bash, zsh, fish | `<shell> -l -c "<cmd>"` (login: carrega o PATH do perfil) |
| sh | `sh -c "<cmd>"` |
| pwsh, powershell | `-NoLogo -NoProfile -NonInteractive -Command "<UTF-8 setup>; <cmd>"` |
| cmd | `cmd.exe /D /S /C "<cmd>"` (argumento bruto) |
| wsl | `wsl.exe -e bash -lc "<cmd>"` |

No Windows os processos são criados com `CREATE_NO_WINDOW` (sem janelas de
console piscando).

---

## terminal

Terminais reais (PTY): ConPTY no Windows, pty Unix no Linux/macOS. A saída é
mantida em buffer circular (1 MiB por terminal) endereçado por offset em bytes.

### `terminal.create`

| Arg | Tipo | Padrão |
| --- | ---- | ------ |
| `shell` | string | shell padrão |
| `cwd` | string | diretório base |
| `cols`, `rows` | number | 120 × 30 |
| `env` | objeto | `{}` |

Saída (`TerminalInfo`): `{ id, shell, cwd, pid, alive, exitCode, cols, rows, createdAt }`.
No Unix, `TERM=xterm-256color` e `COLORTERM=truecolor`.

### `terminal.write`

`{ id, data }` → `{ id, bytesWritten }`. `data` é enviado cru; Enter é `\r`
(ex.: `"npm test\r"`). Terminal encerrado → `NOT_RUNNING`.

### `terminal.read`

`{ id, since?, maxBytes? }` → `{ id, alive, exitCode, data, from, next, truncated, hasMore }`.
Passe `next` como `since` na leitura seguinte para receber só o que é novo.
`truncated: true` indica que parte da saída anterior a `from` já saiu do buffer.

### `terminal.close`

`{ id }` → `TerminalInfo`. Mata o shell e fecha o PTY (os jobs em primeiro
plano recebem SIGHUP no Unix).

### `terminal.list`

`{}` → `TerminalInfo[]` (inclui terminais cujo shell terminou mas que ainda não
foram fechados).

---

## process

Processos de longa duração (servidores de desenvolvimento, watchers). Cada
processo roda em um grupo próprio (Unix) para que a árvore inteira possa ser
encerrada. Saída combinada (stdout + stderr) em buffer de 1 MiB.

### `process.start`

| Arg | Tipo | Padrão |
| --- | ---- | ------ |
| `command` | string | |
| `cwd` | string | diretório base |
| `shell` | string | shell padrão |
| `env` | objeto | `{}` |
| `name` | string | o próprio comando |

Saída (`ProcessInfo`): `{ id, name, command, cwd, shell, pid, status, exitCode, startedAt, endedAt }`,
`status` ∈ `running` | `exited` | `stopped`.

### `process.stop`

`{ id, force? = false }` → `ProcessInfo`. Unix: SIGTERM no grupo, SIGKILL após
5 s (ou imediatamente com `force`). Windows: `taskkill /PID <pid> /T /F`.
Parar um processo já terminado é idempotente.

### `process.list`

`{}` → `ProcessInfo[]` em ordem de início.

### `process.read`

`{ id, since?, maxBytes? }` → `{ id, status, exitCode, data, from, next, truncated, hasMore }`
(mesma semântica de `terminal.read`).

---

## Encerramento do app

Ao fechar o Orchestrator, `ToolRuntime::shutdown` fecha todos os terminais e
mata todos os processos gerenciados. No Linux/macOS, SIGTERM, SIGINT e SIGHUP
(logout, `kill`, Ctrl+C no `tauri dev`) são convertidos em saída normal do app,
com a mesma limpeza.

**Limitação conhecida:** se o app for morto abruptamente (SIGKILL,
`taskkill /F`, crash), processos iniciados por `process.start` podem
sobreviver (terminais morrem junto, pois o PTY é fechado pelo SO). A solução
robusta (Job Objects no Windows, supervisão de processos no Unix) está
prevista junto dos controles globais (Fase 9).

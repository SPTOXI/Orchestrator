# Tool Runtime — referência

O Tool Runtime (`packages/runtime`, crate `orchestrator-runtime`) é o único
componente que executa operações no sistema operacional. Toda chamada entra por
`ToolRuntime::invoke(ToolCall) -> ToolResult` — pela UI hoje (via
`runtime_invoke`) e pelas sessões de provider de IA (Fase 3, via
`TurnContext::call_tool`; ver [`providers.md`](./providers.md)).

## Contrato

```jsonc
// ToolCall
{ "id": "01J…", "tool": "filesystem.read", "args": { "path": "src/main.rs" },
  "origin": { "type": "user" } }
  // | { "type": "agent", "agentId": "…", "sessionId": "…", "provider": "echo" }  (sessão de IA)
  // | { "type": "system" }

// ToolResult
{ "callId": "01J…", "tool": "filesystem.read", "ok": true,
  "output": { … },                         // null quando ok = false
  "error": null,                           // { "kind": "NOT_FOUND", "message": "…" }
  "startedAt": "…", "finishedAt": "…", "durationMs": 3 }
```

- JSON em `camelCase`. Campos desconhecidos em `args` são rejeitados
  (`INVALID_ARGS`) para que erros de digitação de uma IA não passem em silêncio.
- `args: null` equivale a `{}`.
- Caminhos relativos são resolvidos a partir do diretório base do runtime:
  a raiz do projeto aberto (`project.open`) ou, sem projeto, a pasta do
  usuário. `~` e `~/…` são expandidos.
- Erros (`error.kind`): `UNKNOWN_TOOL`, `INVALID_ARGS`, `NOT_FOUND`,
  `ALREADY_EXISTS`, `PERMISSION_DENIED` (negado pelo SO), `IO`, `SPAWN`,
  `NOT_RUNNING`, `COMMAND_FAILED` (um comando externo, ex. `git`, falhou; a
  mensagem traz a saída dele), `CANCELLED` (pedido por um turno de IA já
  cancelado; nada foi executado, ADR-0009), `INTERNAL`.
- Cada ferramenta é uma **consulta** (`readOnly: true`, não altera estado) ou
  uma **ação**. `runtime_tools` devolve a marca e o `TOOL_CALLED` a registra.
- Não existe lista de comandos proibidos nem confirmação oculta no runtime. O
  gate de autonomia (Fase 9) ficará explicitamente na frente de `invoke`.

## Eventos

Toda chamada gera `TOOL_CALLED` (`data`: `tool`, `readOnly`, `args`
resumidos, `ok`, `error`, `durationMs`), com `callId` para correlação.
Eventos adicionais:

| Evento | Quando | `data` |
| ------ | ------ | ------ |
| `FILE_CHANGED` | `filesystem.write/move/delete` com sucesso | `change` (`created`/`modified`/`moved`/`deleted`), `path` ou `from`/`to` |
| `COMMAND_EXECUTED` | `shell.execute`/`package.*` terminou; `process.start` (ou `package.run` em segundo plano) iniciou | `command`, `shell`, `cwd`, `exitCode`, `timedOut`, `durationMs`, `stdoutTail`, `stderrTail` (últimos 4 KiB) / `processId`, `pid`, `background` |
| `PROCESS_EXITED` | processo gerenciado terminou | `processId`, `command`, `exitCode`, `stopped` |
| `TERMINAL_EXITED` | shell de um terminal terminou | `terminalId`, `shell`, `exitCode`, `closed` |
| `PROJECT_OPENED` | `project.open` | `name`, `path`, `gitRoot`, `branch`, `languages`, `frameworks` |
| `GIT_COMMIT` | `git.commit` | `repo`, `hash`, `branch`, `subject` |
| `GIT_PUSH` | `git.push` | `repo`, `branch`, `upstream`, `forced` |

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
No Unix, `TERM=xterm-256color` e `COLORTERM=truecolor`. No Windows, o ConPTY
abre a sessão pedindo a posição do cursor (`ESC[6n`) e bloqueia até receber
resposta: o runtime responde esse handshake e o remove da saída, para que um
terminal operado só por `terminal.write`/`terminal.read` (sem UI) funcione.

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

## project (Fase 2)

### `project.discover`

| Arg | Tipo | Padrão | |
| --- | ---- | ------ | - |
| `roots` | string[] | pasta do usuário + `C:\Projetos`, `D:\dev`, … existentes (Windows) | raízes da busca |
| `maxDepth` | number | 4 (máx. 12) | profundidade a partir de cada raiz |
| `maxDirs` | number | 20000 | limite de pastas visitadas |

Saída: `{ roots, projects: [{ name, path, markers, isGitRepo }], scannedDirs, truncated }`.
Marcadores: `.git`, `package.json`, `deno.json`, `pyproject.toml`,
`requirements.txt`, `setup.py`, `Pipfile`, `Cargo.toml`, `go.mod`, `pom.xml`,
`build.gradle(.kts)`, `composer.json`, `Gemfile`, `*.sln`/`*.csproj`,
`Dockerfile`, compose. Não entra em pastas ocultas, `node_modules`, `target`,
`dist`, `build`, `vendor`, `venv`, `AppData`, `Library`… nem dentro de um
projeto já encontrado; não segue links simbólicos.

### `project.profile`

`{ path? }` (padrão: projeto aberto) → `ProjectProfile`:

```jsonc
{
  "name": "meu-saas", "path": "/…/meu-saas",
  "git": { "root", "branch", "head", "upstream", "ahead", "behind", "remotes": [{ "name", "url" }],
           "staged", "modified", "deleted", "untracked", "conflicted", "clean" },   // null sem Git
  "languages": ["TypeScript"], "frameworks": ["Next.js", "React"],
  "packageManagers": ["pnpm"],                       // o primeiro é o usado por package.*
  "runtimes": [{ "name": "Node.js", "version": ">=20" }],
  "docker": { "dockerfiles": ["Dockerfile"], "composeFiles": ["docker-compose.yml"], "images": ["postgres:16"] },
  "databases": ["PostgreSQL", "Redis"], "tools": ["Prisma", "Vitest", "Docker"],
  "importantFiles": ["README.md", ".env", "prisma/schema.prisma"],   // .env: só o nome
  "scripts": { "dev": "next dev" }, "monorepo": false,
  "markers": ["package.json", "pnpm-lock.yaml", "dependency: next", "prisma/schema.prisma: provider postgresql"],
  "detectedAt": "…"
}
```

Detecção (heurística, por evidência): Node/Deno/Bun (lockfiles,
`packageManager`, dependências, `engines.node`, `.nvmrc`), Python
(`pyproject.toml`, `requirements*.txt`, `Pipfile`, `poetry.lock`, `uv.lock`,
`manage.py`, `.python-version`, `requires-python`), Rust (`Cargo.toml`,
`rust-toolchain.toml`), Go (`go.mod`), Java/Kotlin (Maven/Gradle), C# (.NET),
PHP (Composer), Ruby (Bundler); bancos via Prisma, imagens do compose e
dependências.

### `project.open`

`{ path }` → `ProjectProfile`. A pasta vira o diretório base do runtime;
emite `PROJECT_OPENED`. Pasta inexistente → `NOT_FOUND` (o projeto anterior
continua aberto).

---

## git (Fase 2)

Executa o `git` do sistema ([ADR-0007](./adr/0007-git-via-cli-do-sistema.md)).
Todas aceitam `path?` (pasta dentro do repositório; padrão: projeto aberto).
Fora de um repositório → `NOT_FOUND`; `git` ausente → `SPAWN`; o Git recusou →
`COMMAND_FAILED` com stderr/stdout.

| Ferramenta | Tipo | Argumentos | Saída |
| ---------- | ---- | ---------- | ----- |
| `git.status` | consulta | — | `{ root, branch, head, detached, upstream, ahead, behind, files: [{ path, originalPath, staged, unstaged, conflicted }], clean, remotes }` (`staged`/`unstaged` ∈ `modified`, `added`, `deleted`, `renamed`, `copied`, `typeChanged`, `untracked`, `conflicted`) |
| `git.diff` | consulta | `staged?`, `target?` (revisão), `files?`, `contextLines?`, `maxBytes?` (1 MiB) | `{ patch, files: [{ path, originalPath, additions, deletions, binary }], truncated }` |
| `git.log` | consulta | `limit?` (30, máx. 1000), `ref?`, `file?` | `[{ hash, shortHash, parents, author, email, date, subject }]` (repositório sem commits → `[]`) |
| `git.branch` | ação | `create?`, `startPoint?`, `delete?`, `force?` | branches locais e remotas `[{ name, remote, current, upstream, commit, date, subject }]` |
| `git.checkout` | ação | `target`, `create?`, `startPoint?` | `{ output, status }` |
| `git.add` | ação | `files?` ou `all?` | status |
| `git.commit` | ação | `message`, `all?`, `amend?` | `{ hash, shortHash, branch, subject, output }` + `GIT_COMMIT` |
| `git.pull` | ação | `remote?`, `branch?`, `mode?` (`merge`/`rebase`/`ffOnly`; padrão: config do usuário), `timeoutMs?` | `{ output, status }` |
| `git.push` | ação | `remote?`, `branch?`, `setUpstream?`, `force?` (`--force-with-lease`), `timeoutMs?` | `{ output, status }` + `GIT_PUSH` |
| `git.stash` | ação | `action` (`push`/`pop`/`apply`/`drop`/`list`), `message?`, `includeUntracked?`, `index?` | `{ output, stashes }` |
| `git.reset` | ação | `mode?` (`soft`/`mixed`/`hard`; padrão `mixed`), `target?`, `files?` (só `mixed`: tira do stage) | `{ output, status }` |

Nomes de ref que começam com `-` são rejeitados (`INVALID_ARGS`) para não
virarem opções do Git. `branch` sem `remote` em pull/push → `INVALID_ARGS`.

---

## package (Fase 2)

Usam o gerenciador detectado no perfil (o primeiro de `packageManagers`) ou o
informado em `manager`. Rodam pelo shell padrão (mesmo caminho de
`shell.execute`); **cada argumento é citado para o shell** (POSIX, PowerShell
ou CMD), então nomes de pacote e argumentos nunca são interpretados por ele.

### `package.install`

`{ path?, packages?, dev?, manager?, timeoutMs? }` → `{ manager, command, result }`.

| Gerenciador | Sem pacotes | Com pacotes (`dev`) |
| ----------- | ----------- | ------------------- |
| npm | `npm install` | `npm install [--save-dev] …` |
| pnpm | `pnpm install` | `pnpm add [-D] …` |
| yarn / bun | `yarn install` / `bun install` | `yarn add [--dev] …` / `bun add [--dev] …` |
| poetry | `poetry install` | `poetry add [--group dev] …` |
| uv | `uv sync` | `uv add [--dev] …` |
| pipenv | `pipenv install [--dev]` | `pipenv install [--dev] …` |
| pip | `python -m pip install -r requirements.txt` (ou `.`) | `python -m pip install …` |
| cargo | `cargo fetch` | `cargo add [--dev] …` |
| go | `go mod download` | `go get …` |

### `package.run`

`{ script, args?, path?, manager?, background?, timeoutMs? }` →
`{ manager, command, result }` ou, com `background: true`,
`{ manager, command, process }` (processo gerenciado, como `process.start`).
npm/pnpm/yarn/bun: `<pm> run <script>` (npm recebe `--` antes dos argumentos);
poetry/uv/pipenv: `<pm> run <script>`; cargo/go: `<pm> <script>` (ex.: `test`).

---

## runtime (Fase 2)

Consultas executadas no shell padrão (timeout de 15 s cada):

| Ferramenta | Saída |
| ---------- | ----- |
| `runtime.node` | `{ available, version, managers: { npm, pnpm, yarn, bun } }` |
| `runtime.python` | `{ available, version, command, pip }` (tenta `python3`/`python`; no Windows `python`/`py`/`python3`) |
| `runtime.docker` | `{ available, version, daemonRunning, serverVersion, compose }` |

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

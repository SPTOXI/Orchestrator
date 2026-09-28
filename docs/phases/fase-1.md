# Fase 1 — Desktop shell e Tool Runtime

## STATUS

✅ **Concluída.** O app desktop (Tauri 2 + React 19 + TypeScript) abre, conecta
ao runtime Rust e opera filesystem, shell, terminal real (PTY) e processos,
com auditoria de toda chamada.

![Orchestrator — Fase 1](../assets/fase-1-desktop.png)

## Arquivos criados

**Rust — `packages/core` (`orchestrator-core`)**
- `src/lib.rs`, `src/ids.rs` (IDs UUID v7), `src/tool.rs` (`ToolCall`, `ToolResult`,
  `ToolError`, `CallOrigin`, `ToolSpec`), `src/event.rs` (`AuditEvent`, `EventKind`,
  `StreamEvent`, `EventSink`, `MemorySink`, `NullSink`)

**Rust — `packages/runtime` (`orchestrator-runtime`)**
- `src/lib.rs` (dispatcher `ToolRuntime::invoke`, auditoria), `src/catalog.rs`
- `src/filesystem.rs`, `src/shell.rs`, `src/terminal.rs`, `src/process.rs`
- `src/output.rs` (buffer com offsets, decodificador UTF-8 incremental)
- `src/platform.rs` (paths, grupos de processo, encerramento de árvore)
- `tests/runtime.rs` (testes ponta a ponta via `invoke`)

**Rust — `apps/desktop/src-tauri` (`orchestrator-desktop`)**
- `src/main.rs`, `src/lib.rs` (setup, `DesktopSink`, shutdown), `src/commands.rs`,
  `src/audit_log.rs` (JSONL + memória)
- `tauri.conf.json`, `capabilities/default.json`, `build.rs`, `icons/*`

**TypeScript — `apps/desktop`**
- `src/lib/`: `types.ts`, `runtime.ts` (cliente IPC tipado), `events.ts`,
  `outputSync.ts`, `format.ts`, `useRuntimeSessions.ts` + testes `*.test.ts`
- `src/components/`: `ContextBar`, `StatusBar`, `Explorer`, `FileEditor`,
  `TerminalPanel`, `XTermView`, `ProcessesPanel`, `CommandPanel`,
  `HistoryPanel`, `SessionsPanel`, `PhasePlaceholder`, `icons`
- `src/App.tsx`, `src/main.tsx`, `src/styles.css`, `index.html`,
  `vite.config.ts`, `tsconfig.json`, `package.json`, `public/favicon.svg`

**Documentação e CI**
- `docs/tool-runtime.md`, `docs/ipc.md`, `docs/phases/*`
- ADRs [0003](../adr/0003-gateway-ipc-unico.md), [0004](../adr/0004-operacoes-auxiliares-do-tool-runtime.md),
  [0005](../adr/0005-observabilidade-antes-do-sqlite.md), [0006](../adr/0006-ci-multiplataforma.md)
- `.github/workflows/ci.yml`

## Arquivos modificados

`README.md`, `ARCHITECTURE.md`, `Cargo.toml`, `package.json` (estado da fase,
membros do workspace, scripts).

## Dependências instaladas

| Onde | Dependências |
| ---- | ------------ |
| Rust (runtime) | `tokio`, `portable-pty` 0.9, `serde`/`serde_json`, `chrono`, `uuid` (v7), `parking_lot`, `which`, `base64`, `libc` (Unix); dev: `tempfile` |
| Rust (desktop) | `tauri` 2.12, `tauri-build` 2.7, `tokio` (sinais, Unix) |
| npm | `@tauri-apps/api` 2.12, `@tauri-apps/cli` 2.12, `react`/`react-dom` 19.3, `@xterm/xterm` 6, `@xterm/addon-fit` 0.11, `vite` 8, `@vitejs/plugin-react` 6, `typescript` 7, `vitest` 5 |
| Sistema (Linux) | `libwebkit2gtk-4.1-dev`, `libxdo-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev` (pré-requisitos do Tauri) |

## Comandos executados

```bash
pnpm install
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check -p orchestrator-runtime --all-targets --target x86_64-pc-windows-gnu
cargo check -p orchestrator-runtime --all-targets --target x86_64-apple-darwin
cargo clippy --workspace --all-targets --target x86_64-pc-windows-gnu -- -D warnings   # inclui o crate Tauri
pnpm check        # typecheck + fmt + clippy
pnpm test         # cargo test --workspace + vitest
pnpm --filter @orchestrator/desktop build
pnpm tauri icon   # ícones a partir de icons/icon.svg
pnpm tauri dev    # app real sob Xvfb, exercitado com xdotool
pnpm tauri build --no-bundle
```

## Testes executados

| Suíte | Qtde | Cobre |
| ----- | ---- | ----- |
| `orchestrator-core` (unit) | 9 | serialização de contratos, IDs, sinks |
| `orchestrator-runtime` (unit) | 31 | buffers/offsets, UTF-8 incremental, paths, filesystem, detecção de shells, resumo de auditoria |
| `orchestrator-runtime` (integração) | 15 | catálogo ↔ dispatcher, auditoria, filesystem, `shell.execute` (exit code, stderr, env, stdin, timeout, truncamento), ciclo de vida de processos, **kill da árvore de processos**, **PTY real** (comando, resize, exit code, fechamento), shutdown |
| `orchestrator-desktop` (unit) | 3 | log JSONL, janela de eventos recentes, falha de abertura do log |
| Frontend (vitest) | 15 | sincronização buffer × eventos, fan-out de eventos, formatação, paths (Windows/Unix), fins de linha CRLF/LF |
| Manual (app real, Xvfb) | — | `tauri dev` e binário de release (CSP ativa): terminal bash interativo com cores e resize; múltiplos terminais; troca de workspace; abrir/editar/salvar/criar/excluir arquivos; processo com stderr e exit code; processo longo parado (árvore encerrada, verificado com `ps`); `shell.execute`; HISTORY; `audit.jsonl`; SIGTERM no app encerra os processos gerenciados |

## Resultado dos testes

- Rust: **58/58** aprovados (integração repetida 6× sem falhas).
- Frontend: **15/15** aprovados; `tsc` sem erros; build de produção OK.
- `cargo clippy -D warnings` e `cargo fmt --check`: limpos (Linux e alvo Windows,
  incluindo o crate Tauri).
- Windows e macOS: código compila (checagem cruzada); os testes nesses SOs
  rodam no CI (ADR-0006).

## Problemas encontrados

| Problema | Resolução |
| -------- | --------- |
| Estrutura do documento sem lugar para o Tool Runtime | ADR-0002 (`packages/runtime`) |
| Digitação humana no terminal geraria um `TOOL_CALLED` por tecla | Canal de streaming `terminal_input`/`terminal_resize` (ADR-0003); `terminal.write` continua auditado para agentes |
| Saída perdida/duplicada ao abrir um terminal já em execução | Offsets absolutos + `OutputSync` (fila até o snapshot, descarte por offset) |
| Chunks de PTY/pipe cortando caracteres UTF-8 | Decodificador incremental |
| `process.stop` deixava netos vivos (`sleep &`) | Grupo de processos próprio + `kill(-pgid)`; `taskkill /T /F` no Windows; teste dedicado |
| No Windows (CI) o terminal só emitia `ESC[6n`: o ConPTY espera a resposta ao pedido de posição do cursor e um terminal operado sem UI (agente) travava | O runtime responde ao handshake inicial e o remove da saída |
| ConPTY não entrega EOF ao leitor quando o shell sai | O waiter libera o pseudo console ao detectar a saída |
| Nova aba de terminal não ficava ativa (corrida com a lista compartilhada) | Seleção pendente até a lista atualizar |
| Fechar terminal aparecia como "exit code 1" no histórico | Estado `closed` → evento "terminal … closed" |
| Com SIGTERM (logout, `kill`) o app morria sem `RunEvent::Exit` e deixava processos órfãos (verificado) | Handler de SIGTERM/SIGINT/SIGHUP → `AppHandle::exit`; verificado com o binário de release |
| `<textarea>` converte CRLF em LF: arquivos Windows abririam "não salvos" e seriam gravados em LF | Edição em LF e restauração do fim de linha original ao salvar (testado) |
| `<StrictMode>` dispararia efeitos duas vezes criando terminais/processos reais | StrictMode não usado (comentado em `main.tsx`) |
| xterm.js injeta `<style>` e a CSP com nonce bloquearia | `dangerousDisableAssetCspModification: ["style-src"]` (documentado em `docs/ipc.md`) |

Limitações conhecidas (registradas): processos de `process.start` podem
sobreviver se o app for morto abruptamente (SIGKILL, `taskkill /F`, crash); `audit.jsonl` sem rotação até o
SQLite (Fase 6); processos encerrados permanecem em `process.list` durante a
sessão.

## Próxima fase

**Fase 2 — Project Discovery, Project Profile e Git**: abrir pasta
(diálogo nativo), localizar repositórios, detectar `.git`, gerenciadores de
pacote, Python, Docker, Next.js, Prisma; montar o `PROJECT PROFILE`; Git local
(`status`, `diff`, `log`, `branch`, `checkout`, `add`, `commit`, `pull`,
`push`, `stash`, `reset`) em `packages/git`, exposto como `git.*` no Tool
Runtime e no painel GIT; o diretório base do runtime passa a ser a raiz do
projeto.

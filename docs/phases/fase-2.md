# Fase 2 — Project Discovery, Project Profile e Git

## STATUS

✅ **Concluída.** O Orchestrator abre e descobre projetos, monta o PROJECT
PROFILE com evidências e opera o Git local do usuário. Tudo exposto como
ferramentas auditadas do Tool Runtime (prontas para os agentes da Fase 3+) e
na UI (painéis PROJECT e GIT, perfil, descoberta e diff).

![Git e diff](../assets/fase-2-git.png)

![PROJECT PROFILE](../assets/fase-2-perfil.png)

## Decisões registradas antes do código

- [ADR-0007](../adr/0007-git-via-cli-do-sistema.md) — Git via `git` do sistema
  (credenciais, hooks e configuração do usuário), formatos para máquina, sem
  prompts interativos, erro `COMMAND_FAILED`.
- [ADR-0008](../adr/0008-projeto-deteccao-e-diretorio-base.md) — tipos do
  projeto em `core`, detecção no `runtime`, ferramentas `project.*`, projeto
  aberto = diretório base do runtime, `package.*`/`runtime.*` pelo perfil,
  marca `readOnly` no catálogo, `.env` nunca lido.

## Arquivos criados

**Rust — `packages/git` (`orchestrator-git`, novo crate)**
- `src/lib.rs` (executor do `git`, operações, timeout), `src/parse.rs`
  (porcelain v2, log, for-each-ref, numstat, stash, remotes), `src/types.rs`
- `tests/git.rs` (repositórios reais; push/pull contra remoto bare local)
- `Cargo.toml`, `README.md` (substitui o README de planejamento)

**Rust — `packages/core`**
- `src/project.rs` (`ProjectProfile`, `ProjectCandidate`, `GitSummary`,
  `DockerInfo`, `RuntimeRequirement`)

**Rust — `packages/runtime`**
- `src/project.rs` (descoberta e perfil), `src/git_tools.rs` (ferramentas
  `git.*`), `src/package.rs` (`package.*`, `runtime.*`, citação por shell)
- `tests/phase2.rs`

**TypeScript — `apps/desktop/src`**
- `components/`: `ProjectPanel`, `ProfileView`, `DiscoveryView`, `GitPanel`,
  `DiffView`, `ErrorBoundary`
- `lib/`: `useGitStatus.ts`, `diff.ts` (+ teste), `recent.ts` (+ teste)

**Documentação**
- ADRs 0007 e 0008, `docs/phases/fase-2.md`, `docs/assets/fase-2-*.png`

## Arquivos modificados

- `packages/core/src/tool.rs` — `ToolErrorKind::CommandFailed`, `ToolSpec.read_only`
- `packages/runtime/src/lib.rs` — diretório base mutável (projeto aberto),
  despacho das novas ferramentas, `readOnly` no `TOOL_CALLED`
- `packages/runtime/src/catalog.rs` — 35 ferramentas, cada uma consulta ou ação
- `apps/desktop/src-tauri` — plugin de diálogo e comando `pick_folder`
- `apps/desktop/src` — `App.tsx` (projeto, abas genéricas, status Git),
  `Explorer`, `ContextBar` (branch real), `StatusBar`, `HistoryPanel` (filtro por
  `readOnly`), `useRuntimeSessions`, `main.tsx`, tipos, cliente IPC, estilos
- `ARCHITECTURE.md`, `README.md`, `docs/tool-runtime.md`, `docs/ipc.md`,
  ADR-0006, READMEs dos pacotes, `.github/workflows/ci.yml`, `Cargo.toml`

## Dependências instaladas

| Onde | Dependência |
| ---- | ----------- |
| Rust (desktop) | `tauri-plugin-dialog` 2.8 (seletor nativo de pasta, usado só pelo Rust) |
| Rust (git, runtime) | nenhuma nova externa (`which`, `serde` já no workspace) |
| Sistema | `git` (já era pré-requisito) |

## Comandos executados

```bash
cargo test -p orchestrator-git
cargo test -p orchestrator-runtime --test phase2
pnpm check        # tsc + cargo fmt --check + cargo clippy -D warnings
pnpm test         # cargo test --workspace + vitest
cargo clippy --workspace --all-targets --target x86_64-pc-windows-gnu -- -D warnings
cargo check -p orchestrator-git -p orchestrator-runtime --all-targets --target x86_64-apple-darwin
pnpm tauri dev    # app real sob Xvfb com projetos de demonstração
```

## Testes executados

| Suíte | Qtde | Cobre |
| ----- | ---- | ----- |
| `orchestrator-git` (unit) | 9 | parsers de status/log/branches/numstat/stash/remotes, refs com `-`, mensagens de erro |
| `orchestrator-git` (integração) | 10 | status de todos os tipos de mudança, add/commit/log/diff, repositório sem commits, branch/checkout/delete, stash, reset soft/hard, **push e pull com remoto local**, fora de repositório |
| `orchestrator-runtime` (unit, novos) | 12 | perfil Next.js+Prisma+Docker (sem vazar `.env`), Django, Poetry+Rust workspace, pasta vazia, descoberta (ignora `node_modules`/ocultas/profundas, limite), comandos de `package.*`, **citação de argumentos** (POSIX, PowerShell, CMD) |
| `orchestrator-runtime` (integração Fase 2) | 8 | `project.open` muda o diretório base + `PROJECT_OPENED`, pasta inexistente, `project.discover`, fluxo Git completo via ferramentas com `GIT_COMMIT`/`GIT_PUSH` e marca `readOnly`, erros tipados, `package.run` real (npm), `runtime.*` |
| Frontend (vitest, novos) | 5 | parser de diff unificado, projetos recentes |
| Manual (app real) | — | seletor nativo de pasta (abrir/cancelar); abrir por caminho; perfil completo; executar script `dev` em segundo plano e parar; editar/salvar → GIT mostra `M`; diff colorido; stage, commit (Ctrl+Enter), push; criar branch e push com upstream; atualização do GIT após comando no terminal; descoberta e troca de projeto; runtimes instalados; HISTORY com `PROJECT_OPENED`/`GIT_COMMIT`/`GIT_PUSH`; reload da UI preservando terminais |

## Resultado dos testes

- Rust: **96/96** (core 9, desktop 3, git 19, runtime 65); integração repetida
  5× sem falhas.
- Frontend: **20/20**; `tsc` limpo.
- `cargo clippy -D warnings` limpo no Linux e no alvo Windows (workspace
  inteiro); `cargo fmt --check` limpo; checagem cruzada para macOS OK.
- Windows e macOS: testes rodam no CI.

## Problemas encontrados

| Problema | Resolução |
| -------- | --------- |
| A aba Processos não mostrava processos iniciados por `package.run` em segundo plano | A lista também atualiza em `COMMAND_EXECUTED` com `background: true` |
| Aviso de script do projeto anterior continuava visível após trocar de projeto | Mensagens e checagem de runtimes são zeradas ao trocar de projeto |
| Em pasta sem Git, cada atualização automática gerava um `git.status` com falha no histórico | Enquanto a pasta não é repositório, só reconsulta ao abrir projeto, focar a janela ou pelo botão |
| Saída contínua de terminal dispararia `git.status` a cada 1,5 s | Debounce com espera máxima: no máximo uma consulta a cada 15 s durante saída contínua |
| Erro de renderização deixava a janela em branco (visto com HMR no modo dev) | `ErrorBoundary` no topo: mostra o erro e recarrega só a UI (terminais e processos continuam no runtime) |
| `@` inicia *splatting* no PowerShell e `%`/`"` são perigosos no CMD | Citação por shell nos comandos de `package.*`; CMD rejeita argumentos inseguros |
| Plurais ("1 alterações") | Corrigidos |

Limitações conhecidas: o diálogo nativo de pasta não pôde ser automatizado
até o fim sob Xvfb sem gerenciador de janelas (abrir e cancelar foram
verificados; a abertura foi validada pelo campo de caminho, que usa a mesma
ferramenta `project.open`). Projetos recentes ficam no armazenamento local da
UI até o SQLite (Fase 6). `PROJECT_CREATED` fica para a Fase 6.

## Próxima fase

**Fase 3 — AIProvider, Provider Registry e Provider Sessions**: a interface
comum (`start`, `resume`, `execute`, `stream`, `cancel`, `spawnAgent`,
`inspect`, `capabilities`) em `packages/providers`, o registro de providers e
as sessões — sem ainda conectar OpenAI/Codex (Fase 4) e Claude Code (Fase 5).

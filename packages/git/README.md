# packages/git — `orchestrator-git`

**Git local** (Fase 2) e a integração GitHub (Fase 10,
[ADR-0017](../../docs/adr/0017-github-e-operacoes-remotas.md); referência em
[`docs/github.md`](../../docs/github.md)).

Executa o `git` instalado no sistema (ADR-0007) para respeitar credenciais,
hooks e configuração do usuário, e interpreta formatos estáveis para
máquinas (`status --porcelain=v2 -z`, `log`/`for-each-ref` com separadores de
controle, `diff --numstat -z`).

| Operação | Comando |
| -------- | ------- |
| `status` | `git status --porcelain=v2 --branch -z --untracked-files=all` |
| `diff` | `git diff [--cached] [<ref>] -- <paths>` + `--numstat -z` |
| `log` | `git log -n <N> --format=<campos separados por 0x1f>` |
| `branches` / `create_branch` / `delete_branch` | `git for-each-ref`, `git branch` |
| `checkout` | `git checkout [-b] <ref>` |
| `add` | `git add -A` / `git add -- <paths>` |
| `commit` | `git commit -m <msg> [--all] [--amend]` |
| `pull` / `push` | `git pull` / `git push` (com `timeout` opcional) |
| `fetch` | `git fetch [--prune] [<remote>]` (Fase 10) |
| `stash` | `git stash push/pop/apply/drop/list` |
| `reset` | `git reset --soft/--mixed/--hard [<ref>]`, `git reset -- <paths>` |
| `remotes` | `git remote -v` |

Todas as invocações usam `GIT_TERMINAL_PROMPT=0`, `core.quotepath=false` e,
no Windows, `CREATE_NO_WINDOW`; consultas usam `GIT_OPTIONAL_LOCKS=0`.

Fluxo normal: `AI → workspace local → Git → GitHub`. O GitHub nunca substitui
o workspace local.

## Módulo `github` (Fase 10)

Cliente assíncrono da API REST v3 (`reqwest`), separado do Git local:

| Arquivo | Conteúdo |
| ------- | -------- |
| `github/remote.rs` | URL de remoto → `RepoRef` (https, ssh, scp, git://); qual remoto representa o projeto |
| `github/auth.rs` | `Secret` (nunca aparece em `Debug`) e a ordem do token: cofre → `GH_TOKEN`/`GITHUB_TOKEN` → `gh auth token` |
| `github/client.rs` | `GitHubClient`: conta, repositório, PRs (com CI, revisões e comentários recentes), checks, criar PR, comentar, merge (e apagar a branch), issues; erros com o motivo do GitHub |
| `github/types.rs` | o que o cliente devolve, montado da resposta com leitura tolerante; CI resumida em `success`/`failure`/`pending`/`none` |
| `github/mod.rs` | `GitHubSettings` (`host`, `apiUrl` para Enterprise) e `GitHubError` |

O token só é usado pelo cliente para o cabeçalho `Authorization`; nenhum
valor devolvido o contém.

```bash
cargo test -p orchestrator-git   # repositórios temporários reais e uma API do GitHub falsa
```

# packages/git — `orchestrator-git`

**Git local** (Fase 2) e, na Fase 10, a integração GitHub.

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
| `stash` | `git stash push/pop/apply/drop/list` |
| `reset` | `git reset --soft/--mixed/--hard [<ref>]`, `git reset -- <paths>` |
| `remotes` | `git remote -v` |

Todas as invocações usam `GIT_TERMINAL_PROMPT=0`, `core.quotepath=false` e,
no Windows, `CREATE_NO_WINDOW`; consultas usam `GIT_OPTIONAL_LOCKS=0`.

Fluxo normal: `AI → workspace local → Git → GitHub`. O GitHub nunca substitui
o workspace local.

```bash
cargo test -p orchestrator-git   # usa repositórios temporários reais
```

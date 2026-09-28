# packages/git

**Git local e integração GitHub.**

| Responsabilidade | Fase |
| ---------------- | ---- |
| status, diff, log, branch, checkout, add, commit, pull, push, stash, reset | 2 |
| Dados para a UI: branch atual, modificados, novos, removidos, diff, últimos commits, remote | 2 |
| Operações registradas no Tool Runtime como `git.*` | 2 |
| GitHub: repositórios, branches, commits, criar branch, abrir/consultar PRs (`github.*`) | 10 |

Fluxo normal: `AI → workspace local → Git → GitHub`. O GitHub nunca substitui
o workspace local.

Ainda não implementado — vira crate Rust na Fase 2 (ver ADR-0001).

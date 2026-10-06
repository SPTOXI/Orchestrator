# GitHub, pull requests e operações remotas

Referência da Fase 10. Decisão em
[ADR-0017](./adr/0017-github-e-operacoes-remotas.md); cliente no módulo
`github` do [`packages/git`](../packages/git/README.md) e ferramentas no
Tool Runtime (`github_tools.rs`).

> O workspace local é a fonte primária; o GitHub é remoto.

```text
task → agente → commit → git.push → github.pr.create → CI (github.checks)
                                               ↓
                    github.pr.merge ← revisões e comentários (github.pr.get)
```

## Conexão

O token é procurado nesta ordem; o primeiro que existir vale:

| Ordem | Fonte | Como |
| ----- | ----- | ---- |
| 1 | cofre do sistema | aba GitHub → "Salvar no cofre" (Windows Credential Manager, Keychain, Secret Service) |
| 2 | ambiente | `GH_TOKEN`, depois `GITHUB_TOKEN` |
| 3 | GitHub CLI | `gh auth token --hostname <host>` (depois de `gh auth login`) |

- O token nunca aparece em argumentos, eventos, saídas, logs ou na UI: o
  que aparece é a fonte (`vault`, `env:GH_TOKEN`, `gh`) e o login.
- O token resolvido fica em memória por um minuto; um 401 o descarta.
- Permissões: token *fine-grained* com o repositório e **Contents**,
  **Pull requests**, **Issues** (leitura e escrita), **Checks** e **Commit
  statuses** (leitura); ou clássico com `repo`.

### Servidor

`<app-data>/github.json`:

```json
{ "host": "github.com", "apiUrl": null }
```

- `host`: host dos remotos que são GitHub.
- `apiUrl`: vazio = `https://api.github.com` (github.com) ou
  `https://<host>/api/v3` (Enterprise).
- Arquivo inválido: github.com, com aviso na aba GitHub.

## Qual repositório

Toda ferramenta `github.*` aceita `repo: "dono/nome"` e `path` (pasta do
projeto). Sem `repo`, vale o remoto do projeto:

1. o remoto do *upstream* da branch atual (`origin/feat` → `origin`);
2. senão `origin`;
3. senão o primeiro remoto no host configurado.

URLs aceitas: `https://host/dono/nome(.git)`, `git@host:dono/nome(.git)`,
`ssh://git@host(:porta)/dono/nome(.git)` e `git://host/dono/nome`. Um
remoto de outro host (GitLab, pasta local) não é GitHub.

## Ferramentas

| Ferramenta | Tipo | Faz |
| ---------- | ---- | --- |
| `git.remotes` | consulta | remotos, com o repositório do GitHub que cada um é |
| `git.fetch` | ação | `git fetch [--prune] [remoto]`: referências remotas, sem tocar nos arquivos |
| `github.status` | consulta | conta, fonte do token, repositório, branch (↑↓), o PR mais recente da branch (aberto, integrado ou fechado) e a CI dele; problemas vão em `accountError`/`repoError` |
| `github.pr.list` | consulta | PRs (`state`, `head`, `base`, `limit`) |
| `github.pr.get` | consulta | um PR: descrição, `mergeable`/`mergeableState`, CI do último commit, a última revisão decisiva de cada revisor, os 20 comentários mais recentes |
| `github.checks` | consulta | CI de um commit/branch/tag (padrão: a branch atual como está no GitHub) |
| `github.issue.list` / `github.issue.get` | consulta | issues (sem os PRs que a API mistura) / uma issue com comentários |
| `github.pr.create` | ação | abre um PR da branch atual (ou `head`) para a branch padrão (ou `base`); `draft` |
| `github.pr.comment` / `github.issue.comment` | ação | comenta |
| `github.pr.merge` | ação | `merge`, `squash` ou `rebase`; `deleteBranch` apaga a branch no GitHub (só do mesmo repositório) |
| `github.issue.create` | ação | abre uma issue (`labels` opcionais) |

### Abrir um PR a partir da branch local

Sem `head` explícito, o `github.pr.create` confere a branch:

- HEAD destacado → recusa;
- sem *upstream* → "a branch X ainda não está no GitHub: envie com
  `git.push { setUpstream: true }`";
- com commits não enviados → "tem N commit(s) ainda não enviados".

Ele não faz o push: enviar é outra operação remota, com a sua própria
decisão de autonomia. Se o *upstream* é um fork (outro dono), o `head` vira
`dono-do-fork:branch`.

### CI

*Check runs* (GitHub Actions e apps) e *commit statuses* viram um estado:

| Estado | Quando |
| ------ | ------ |
| `failure` | algum falhou (`failure`, `timed_out`, `cancelled`, `action_required`, `error`) |
| `pending` | nenhum falhou e algum ainda roda |
| `success` | todos passaram (`neutral` e `skipped` contam como passar) |
| `none` | nada rodou |

### Erros

| GitHub | `ToolError` |
| ------ | ----------- |
| sem token | `PERMISSION_DENIED` com as três formas de conectar |
| 401 | `PERMISSION_DENIED` (token inválido ou expirado) |
| 403 | `PERMISSION_DENIED` (sem permissão) ou `COMMAND_FAILED` (limite de requisições, com a hora em que volta) |
| 404 | `NOT_FOUND` (inclusive "o token não tem acesso") |
| 405/409/422 | `INVALID_ARGS` com a mensagem e os detalhes do GitHub |
| rede, 5xx | `COMMAND_FAILED` |

O Orchestrator não bloqueia merge com CI vermelha nem sem revisão: quem
decide é o repositório (proteção de branch); a resposta do GitHub volta
como está.

## Autonomia

As ferramentas `github.*` passam pelo gate (ADR-0016) como qualquer outra:

| Modo | Consultas | Ações |
| ---- | --------- | ----- |
| Assistido | rodam | pedem autorização |
| Autônomo | rodam (regra 3) | regra 11 das padrão: **perguntar** — "publica no GitHub em nome da sua conta" |
| Acesso Irrestrito | rodam | rodam |

O pedido diz o que vai acontecer: "Abrir o pull request "Retentativas"
(→ main) como rascunho", "Fazer merge (squash) do PR #12 em time/app e
apagar a branch", "Comentar na issue #3: …".

## Eventos

| Evento | Dados |
| ------ | ----- |
| `GITHUB_PR_CREATED` | `repo`, `number`, `title`, `url`, `head`, `base`, `draft` |
| `GITHUB_PR_MERGED` | `repo`, `number`, `title`, `url`, `method`, `sha`, `branchDeleted` |
| `GITHUB_ISSUE_CREATED` | `repo`, `number`, `title`, `url` |

Os três entram na busca do projeto (L3). Comentários ficam só no
`TOOL_CALLED`.

## UI

- **Painel GIT:** botão **Fetch**; seção **GitHub** com o repositório (abre
  no navegador), a conta, o PR da branch atual com a CI (✓ ✗ ●) — ou o
  último, marcado "integrado"/"fechado" —, "Criar pull request" (com o
  aviso quando a branch não está no GitHub), os PRs e as issues abertos.
  Sem token: "Conectar ao GitHub".
- **Aba do PR:** título, estado, `head → base`, commits e arquivos,
  descrição, CI com "detalhes", revisões, comentários e caixa para
  comentar, merge (método, apagar a branch, confirmação e o motivo quando
  o GitHub não deixa), "Abrir no GitHub".
- **Novo PR:** título (da branch, ou de uma task), descrição (da task:
  descrição, resultado e arquivos), base, rascunho, "Enviar (push)" quando
  falta.
- **Aba GitHub:** conta e fonte do token, salvar/remover o token no cofre,
  o que há no ambiente e se o `gh` existe, servidor (Enterprise).
- **HISTORY:** filtros dos três eventos.

## IPC

| Comando | Faz |
| ------- | --- |
| `runtime_invoke("github.*" / "git.fetch" / "git.remotes", args)` | as ferramentas, como qualquer outra |
| `github_settings_get()` | `GitHubSetup`: host, API, se há token no cofre, variável de ambiente, `gh`, aviso |
| `github_settings_save(settings)` | host e API; recusa valores inválidos |
| `github_token_save(token)` / `github_token_clear()` | o token no cofre (nunca volta para a UI) |
| `open_url(url)` | abre uma página `http(s)` no navegador do sistema |

## Limitações

- Sem OAuth pelo navegador nem GitHub App: token pessoal ou `gh`.
- Sem webhooks: a UI consulta ao abrir, quando a branch muda, depois de
  cada ação e em "Atualizar".
- Sem revisão de código pelo Orchestrator (aprovar, pedir mudanças), sem
  editar PR (título, reviewers, labels) e sem GitLab/Bitbucket.
- Comentários: os 20 mais recentes; revisões: a última decisiva de cada
  revisor.
- O contexto das IAs não consulta o GitHub (montar o contexto continua sem
  rede); a IA pede `github.status` quando precisa.

# ADR-0017 — GitHub, pull requests e operações remotas

- **Estado:** Aceita
- **Fase:** 10

## Contexto

A Fase 2 (ADR-0007) deu ao Orchestrator o Git local pelo `git` do sistema,
com `git.pull` e `git.push` como as únicas operações que falam com um
remoto. O resto do trabalho que acontece "do outro lado" — abrir um pull
request, ver se a CI passou, ler a revisão, comentar, fazer o merge, abrir
uma issue — ainda obriga o usuário (e as IAs) a sair do Orchestrator.

Os princípios que valem aqui:

- **o workspace local é a fonte primária; o GitHub é remoto**
  (`ARCHITECTURE.md`, princípio 5): nada do GitHub substitui o que está no
  disco, e nenhuma operação local passa a depender da rede;
- **toda execução passa pelo Tool Runtime e é auditada** (ADR-0001/0003);
- **as IAs nunca tocam no sistema diretamente** (ADR-0009): elas pedem
  ferramentas, e o gate de autonomia (ADR-0016) decide;
- **segredos nunca vão para arquivo, histórico, log ou UI** (ADR-0010).

## Decisão

### 1. Onde

Um módulo `github` no crate `orchestrator-git` (`packages/git`), como a
tabela de módulos do `ARCHITECTURE.md` previa desde a Fase 0. O Git local
continua síncrono e pelo executável `git`; o módulo `github` é um cliente
HTTP assíncrono da API REST v3 (`reqwest`, já usado pelas conexões de API).
O `orchestrator-runtime` expõe o módulo como ferramentas `github.*`, no
mesmo catálogo e pelo mesmo `invoke` auditado das outras.

Não usamos o GitHub CLI (`gh`) para as operações: ele nem sempre está
instalado, e a API dá respostas estruturadas. O `gh` é aceito apenas como
**fonte do token** (abaixo).

### 2. Credencial

O token é procurado nesta ordem, e o primeiro que existir vale:

1. **cofre do sistema** (o mesmo `keyring` das chaves de API), salvo pelo
   usuário na aba GitHub;
2. variáveis de ambiente `GH_TOKEN` ou `GITHUB_TOKEN`;
3. `gh auth token`, se o GitHub CLI estiver instalado e autenticado.

O token nunca aparece em argumentos, eventos, saídas de ferramenta, logs
ou na UI; o que aparece é **de onde** ele veio (`vault`, `env:GH_TOKEN`,
`gh`) e **de quem** ele é (o login que a API devolve). Sem token, as
ferramentas falham com `PERMISSION_DENIED` e a mensagem diz as três formas
de conectar. Não há OAuth pelo navegador nesta fase: um token pessoal
(clássico com `repo`, ou *fine-grained* com Contents, Pull requests,
Issues, Checks e Commit statuses) cobre tudo, e o cofre já existe.

### 3. Qual repositório

As ferramentas aceitam `repo: "dono/nome"`. Sem ele, o repositório vem do
projeto aberto (ou de `path`): o remoto da branch atual, se ela tem um
*upstream*; senão `origin`; senão o primeiro remoto que aponte para o
GitHub. URLs aceitas: `https://host/dono/nome(.git)`,
`git@host:dono/nome(.git)` e `ssh://git@host/dono/nome(.git)`.

O host padrão é `github.com` (API em `https://api.github.com`).
**GitHub Enterprise**: `github.json` guarda `host` e `apiUrl`
(`https://host/api/v3`), editáveis na aba GitHub. Um remoto de outro host
não é tratado como GitHub.

### 4. Ferramentas

Consultas (`readOnly`):

| Ferramenta | Faz |
| ---------- | --- |
| `github.status` | conta (login e de onde vem o token), repositório do projeto (dono, nome, URL, branch padrão, privado) e o PR aberto da branch atual; responde mesmo sem token, dizendo o que falta |
| `github.pr.list` | PRs do repositório (`state`, `head`, `base`, `limit`) |
| `github.pr.get` | um PR: estado, rascunho, `head → base`, autor, se dá para fazer merge, CI do último commit, revisões (a última de cada revisor) e os comentários recentes |
| `github.checks` | a CI de um commit ou branch (padrão: a branch atual): *check runs* e *statuses*, com o resumo `success`, `failure`, `pending` ou `none` |
| `github.issue.list` / `github.issue.get` | issues (sem os PRs, que a API mistura) e uma issue com comentários |

Ações:

| Ferramenta | Faz |
| ---------- | --- |
| `github.pr.create` | abre um PR da branch atual (ou `head`) para a branch padrão (ou `base`); `draft` opcional |
| `github.pr.comment` | comenta num PR |
| `github.pr.merge` | faz o merge (`merge`, `squash` ou `rebase`) e, se pedido, apaga a branch remota |
| `github.issue.create` / `github.issue.comment` | abre uma issue / comenta numa issue |

**O PR sai do que está no GitHub, não do disco.** Antes de abrir, o
`github.pr.create` confere a branch local: sem *upstream*, ou com commits
ainda não enviados, ele recusa dizendo quantos e sugerindo `git.push`. Ele
não faz o push sozinho: enviar é outra operação remota, com a sua própria
decisão de autonomia. Com `head` explícito (inclusive `dono:branch` de um
fork), a conferência não se aplica.

**Nada é bloqueado por nós.** O Orchestrator não impede merge com CI
vermelha nem sem revisão: quem decide é o repositório (proteção de
branch), e a resposta do GitHub volta como está. O que o Orchestrator faz
é mostrar a CI e as revisões antes do botão.

Erros da API viram `ToolError` com a mensagem do GitHub: 401 →
`PERMISSION_DENIED` (token inválido ou expirado), 403 → `PERMISSION_DENIED`
(sem permissão) ou `COMMAND_FAILED` (limite de requisições, com a hora em
que ele volta), 404 → `NOT_FOUND` (inclusive "o token não tem acesso"),
422 → `INVALID_ARGS` com os detalhes, rede e 5xx → `COMMAND_FAILED`.

### 5. Autonomia

As ferramentas `github.*` passam pelo gate como qualquer outra (ADR-0016).

- **Assistido:** consultas rodam; as ações pedem autorização (regra fixa).
- **Autônomo:** as regras padrão ganham, antes da regra geral, **"`github.*`
  · ações → perguntar — publica no GitHub em nome da sua conta"**, ao lado
  de `git.push`. Quem já salvou as próprias regras continua com elas.
- **Acesso Irrestrito:** nada é avaliado.

O resumo do pedido diz o que vai acontecer: "Abrir o PR «título»
(feature → main) em dono/nome", "Fazer merge (squash) do PR #12 em
dono/nome", "Comentar no PR #12".

### 6. Eventos

Três eventos novos, além do `TOOL_CALLED` de toda chamada:

| Evento | Quando | Dados |
| ------ | ------ | ----- |
| `GITHUB_PR_CREATED` | um PR foi aberto | `repo`, `number`, `title`, `url`, `head`, `base`, `draft` |
| `GITHUB_PR_MERGED` | um PR foi integrado | `repo`, `number`, `title`, `url`, `method`, `sha`, `branchDeleted` |
| `GITHUB_ISSUE_CREATED` | uma issue foi aberta | `repo`, `number`, `title`, `url` |

Comentários ficam só no `TOOL_CALLED` (o texto é resumido como qualquer
argumento).

### 7. Operações remotas do Git

Duas ferramentas no grupo `git`, que faltavam para trabalhar com remotos:

- `git.fetch` (ação): `{ path?, remote?, prune? }`, atualiza as referências
  remotas sem tocar na árvore de trabalho;
- `git.remotes` (consulta): os remotos, com URL e, quando for o caso, o
  repositório do GitHub que ele representa.

### 8. Interface

- **Painel GIT**, seção **GitHub**: conta e repositório, o PR da branch
  atual (número, estado, CI), "Criar pull request", a lista de PRs abertos
  e de issues abertas. Sem token, um botão "Conectar ao GitHub".
- **Aba do PR**: detalhes, CI com links, revisões, comentários (e caixa
  para comentar), merge com o método, "Abrir no GitHub". A mesma aba, sem
  número, é o formulário de novo PR: título e descrição (preenchidos pelo
  último commit, ou por uma task escolhida), base, rascunho; se a branch
  não está no GitHub ou tem commits não enviados, ela avisa e oferece
  "Enviar (push)".
- **Aba GitHub**: salvar/remover o token (campo de senha; vai para o
  cofre), de onde vem o token em uso, o login, e `host`/`apiUrl` para
  Enterprise.
- **HISTORY**: filtros dos três eventos.

### 9. Persistência

`<app-data>/github.json` (`host`, `apiUrl`), no mesmo padrão dos outros
arquivos de configuração. Token só no cofre. **Nenhuma migração**: o
banco continua no esquema 4.

## Consequências

- O Orchestrator passa a cobrir o ciclo "task → agente → commit → push →
  PR → CI → revisão → merge" sem sair dele, e as IAs podem ajudar em cada
  etapa, sob o modo de autonomia.
- As operações do GitHub precisam de rede e de um token; sem eles, tudo o
  que é local continua igual.
- Sem OAuth pelo navegador nem GitHub App; sem webhooks: a UI consulta
  quando abre e quando o usuário pede ("Atualizar"), e depois de cada ação.
- Sem revisão de código pelo Orchestrator (aprovar ou pedir mudanças), sem
  edição de PR (título, descrição, reviewers, labels) e sem GitLab ou
  Bitbucket nesta fase.
- O contexto das IAs (ADR-0013) não consulta o GitHub: montar o contexto
  continua sem rede; a IA pode pedir `github.status` quando precisar.

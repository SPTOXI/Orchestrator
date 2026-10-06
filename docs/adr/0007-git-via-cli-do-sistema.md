# ADR-0007 — Git via CLI do sistema

- **Estado:** Aceita
- **Fase:** 2

## Contexto

A Fase 2 implementa o Git local (`status`, `diff`, `log`, `branch`,
`checkout`, `add`, `commit`, `pull`, `push`, `stash`, `reset`) em
`packages/git`. Há duas famílias de implementação:

- bibliotecas embutidas (`libgit2`/`git2`, `gitoxide`/`gix`);
- o executável `git` instalado no sistema.

O princípio “o Git pertence ao usuário” pesa: `pull`/`push` precisam usar as
credenciais, *credential helpers*, chaves SSH, hooks e a configuração que o
usuário já tem. As bibliotecas embutidas não suportam isso por completo
(credential helpers, hooks, `includeIf`, LFS).

## Decisão

1. `packages/git` (crate `orchestrator-git`) executa o **`git` do sistema** e
   interpreta formatos estáveis para máquinas: `status --porcelain=v2 -z`,
   `log` com separadores de controle, `for-each-ref --format`,
   `diff --numstat -z`.
2. Toda invocação usa `-c core.quotepath=false`, `GIT_TERMINAL_PROMPT=0` (uma
   credencial ausente falha em vez de travar esperando um terminal) e, no
   Windows, `CREATE_NO_WINDOW`. Consultas usam `GIT_OPTIONAL_LOCKS=0` para
   não disputar o `index.lock` com o usuário.
3. `pull`/`push` aceitam `timeoutMs` opcional; sem ele não há timeout.
4. Falha de um comando Git é um erro da ferramenta com o novo tipo
   `COMMAND_FAILED`, cuja mensagem traz stderr/stdout do Git.
5. Git ausente → erro `SPAWN` com mensagem explícita; a detecção de projeto
   continua funcionando sem Git (sem a seção Git no perfil).

## Consequências

- Comportamento idêntico ao Git que o usuário usa no terminal.
- Dependência do `git` instalado (já é pré-requisito do projeto).
- Parsers pequenos e testados com repositórios reais criados nos testes.

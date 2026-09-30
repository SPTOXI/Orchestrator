# Fase 10 — GitHub, pull requests e operações remotas

## STATUS

⏳ **Em andamento.** Decisão registrada na
[ADR-0017](../adr/0017-github-e-operacoes-remotas.md).

## Andamento

Cada passo é commitado e enviado ao terminar, para que a fase possa ser
retomada de onde parou (por exemplo, depois de um limite de uso). O
próximo passo é o primeiro sem ✅.

1. ✅ ADR-0017 e este arquivo.
2. ✅ Core: eventos `GITHUB_PR_CREATED`, `GITHUB_PR_MERGED`,
   `GITHUB_ISSUE_CREATED` (e indexados na busca do projeto).
3. ✅ `orchestrator-git`, módulo `github`: remotos → repositório, token
   (cofre/ambiente/`gh`), cliente REST, tipos, erros; testes com servidor
   HTTP falso.
4. ✅ Runtime: `github.*` e `git.fetch`/`git.remotes` no catálogo, schemas,
   eventos; testes.
5. ✅ Motor: regra padrão do Autônomo para `github.*` e resumo dos pedidos;
   testes.
6. ✅ Desktop: `github.json`, token no cofre, comandos, ligação com o runtime.
7. ✅ UI: seção GitHub no painel GIT, aba do PR (detalhes e novo PR), aba
   GitHub (conexão), filtros do HISTORY; testes do frontend.
8. Validação no app real com uma API do GitHub simulada; relatório final
   neste arquivo; README, ARCHITECTURE e docs; PR #1 e CI.

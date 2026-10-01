# Fase 12 — Instaladores, release e atualização automática

## STATUS

🚧 **Em andamento.** Escopo escolhido pelo usuário depois das Fases 0–11
do documento mestre. Decisão em
[ADR-0019](../adr/0019-instaladores-release-e-atualizacao.md).

## Andamento

A fase é feita em passos, cada um commitado e enviado ao terminar, para
poder ser retomada de onde parou.

1. ✅ ADR-0019 e este arquivo.
2. ✅ Empacotamento: metadados e pacotes por sistema no `tauri.conf.json`,
   `scripts/version.mjs` (gravar e conferir a versão) e o `--check` na CI.
3. ✅ Workflow `release.yml`: pacotes nos três sistemas, release em
   rascunho por tag, artefatos quando um push mexe no empacotamento.
4. ✅ Updater no desktop: plugin, `updates.json`, comandos, progresso,
   `APP_UPDATED`; testes.
5. ✅ UI: aba "Sobre e atualizações" e chip na barra de status; testes.
6. ⏳ Validação: pacotes Linux gerados e instalados no container, uma
   atualização assinada de ponta a ponta, documentação e publicação.

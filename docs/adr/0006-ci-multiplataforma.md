# ADR-0006 — CI multiplataforma

- **Estado:** Aceita
- **Fase:** 1

## Contexto

O runtime tem caminhos específicos por SO (ConPTY vs pty, PowerShell/CMD/WSL
vs bash, `taskkill` vs grupos de processo). O desenvolvimento acontece em
Linux, mas o Orchestrator precisa funcionar em Windows e macOS. A regra
“implementar, compilar, executar, testar” não é verificável para esses SOs
sem uma máquina de cada.

## Decisão

Adicionar `.github/workflows/ci.yml` (ferramenta de desenvolvimento, não
funcionalidade do produto), executado em push de qualquer branch, em pull
requests e manualmente:

- **rust** (Linux, Windows, macOS): `cargo fmt --check`, `cargo clippy` e
  `cargo test` de `orchestrator-core`, `orchestrator-git` (Fase 2),
  `orchestrator-runtime`, `orchestrator-providers` (Fase 3),
  `orchestrator-provider-api` (Fase 4, com servidor HTTP falso local),
  `orchestrator-router` (Fase 5) e `orchestrator-memory` (Fase 6, SQLite
  embutido compilado em cada SO).
- **desktop** (Linux): dependências de sistema do Tauri, `pnpm install`,
  typecheck, testes do frontend, build do frontend e `cargo clippy`/`cargo
  test` do crate Tauri.

## Consequências

- Mudanças no runtime são validadas nos três SOs a cada push/PR.

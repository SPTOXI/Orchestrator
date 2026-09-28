# Fase 0 — README, ARCHITECTURE e estrutura do monorepo

## STATUS

✅ **Concluída.**

## Ambiente verificado

| Item | Valor |
| ---- | ----- |
| Sistema operacional | Linux 6.18 x86_64 (Ubuntu 24.04, container de desenvolvimento) |
| Node.js | 22.22.2 |
| npm / pnpm / yarn | 10.9.7 / 10.33.0 / 1.22.22 |
| Rust / Cargo | 1.94.1 / 1.94.1 |
| Git | 2.43.0 |
| Tauri CLI | não instalado globalmente → adicionado como dependência do projeto (`@tauri-apps/cli` 2.12.0) |
| Repositório | `SPTOXI/Orchestrator` existia **vazio** (sem commits); nada a preservar |

## Arquivos criados

- `README.md`, `ARCHITECTURE.md`, `.gitignore`
- `package.json`, `pnpm-workspace.yaml` (workspace pnpm), `Cargo.toml` (workspace Cargo)
- `docs/adr/README.md` e ADRs
  [0001](../adr/0001-monorepo-hibrido-nucleo-em-rust.md) (monorepo híbrido, núcleo em Rust),
  [0002](../adr/0002-pacote-runtime-para-o-tool-runtime.md) (`packages/runtime`)
- Estrutura `apps/desktop/`, `packages/{core, runtime, orchestrator, agents, memory, git, providers/openai, providers/claude}`
- `README.md` de cada pacote de fase futura descrevendo responsabilidade e fase

## Arquivos modificados

Nenhum (repositório vazio).

## Dependências instaladas

Nenhuma nesta fase.

## Comandos executados

`git status`, `git log`, verificação de versões (`node`, `npm`, `pnpm`, `yarn`,
`rustc`, `cargo`, `git`, `cargo tauri`), `pkg-config` para as bibliotecas do
Tauri.

## Testes executados / resultado

Fase apenas documental e estrutural; validada na Fase 1 (o workspace compila
e os testes rodam).

## Problemas encontrados

- A estrutura da seção 5 não tem pacote para o Tool Runtime → registrada a
  decisão ADR-0002 antes de criar `packages/runtime`.
- O documento não define a linguagem dos `packages/*` → ADR-0001: todos são
  crates Rust; o React é só apresentação.

## Próxima fase

Fase 1 — Tauri + React + TypeScript + Rust; filesystem, shell, terminal,
process manager.

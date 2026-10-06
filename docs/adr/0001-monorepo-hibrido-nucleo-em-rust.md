# ADR-0001 — Monorepo híbrido (Cargo + pnpm) com núcleo em Rust

- **Estado:** Aceita
- **Fase:** 0

## Contexto

O documento mestre define a stack (Tauri, React, TypeScript, Rust, SQLite) e o
fluxo `React → Tauri → Rust Runtime → Orchestrator Core`, além da estrutura
`apps/desktop` + `packages/{core, orchestrator, agents, memory, git, providers/*}`.
Não especifica em que linguagem cada pacote é escrito.

Requisitos que pesam na decisão:

- Providers de IA nunca podem acessar o SO ignorando o runtime.
- O núcleo precisa sobreviver a recarregamentos da UI e controlar processos,
  terminais e arquivos.
- Agentes em paralelo, locks de arquivo e SQLite exigem um processo único e
  confiável como dono do estado.

## Decisão

1. O repositório é um **monorepo híbrido**:
   - **workspace Cargo** na raiz (`Cargo.toml`) com os crates Rust;
   - **workspace pnpm** na raiz (`package.json`, `pnpm-workspace.yaml`) com os
     apps TypeScript.
2. Todos os `packages/*` são **crates Rust**. Todo o núcleo (engine, agentes,
   memória, providers, Tool Runtime) roda no processo Rust do Tauri.
3. `apps/desktop` contém a UI React/TypeScript (`src/`) e o crate Tauri
   (`src-tauri/`), que é **apenas ponte IPC**: converte comandos em chamadas ao
   runtime e eventos do runtime em eventos Tauri. Não contém lógica de domínio.
4. Pacotes de fases futuras existem desde já como diretórios com `README.md`
   descrevendo sua responsabilidade. Viram crates (e entram no workspace Cargo)
   na fase em que forem implementados — não há crates vazios compilando código
   inexistente.

## Consequências

- A UI é substituível (outra UI ou um CLI podem usar os mesmos crates).
- Providers, sendo crates Rust, só conseguem agir através do `ToolRuntime`.
- Tipos compartilhados com a UI são espelhados manualmente em
  `apps/desktop/src/lib/types.ts`; se o espelhamento crescer, avaliar geração
  automática (ex.: `ts-rs`) com novo ADR.

# ADR-0002 — Pacote `packages/runtime` para o Tool Runtime

- **Estado:** Aceita
- **Fase:** 1

## Contexto

A seção 3 do documento mestre define o **Tool Runtime** (filesystem, shell,
terminal, process manager, git, github, package managers, runtime) como módulo
próprio da arquitetura, mas a estrutura de repositório da seção 5 não tem um
pacote para ele. A Fase 1 exige implementar filesystem, shell, terminal e
process manager.

Alternativas consideradas:

- Colocar o runtime em `packages/core`: mistura contratos de domínio (tipos
  puros, sem I/O) com execução de sistema (PTY, processos). Ruim para testes
  e para os providers, que devem depender dos contratos e não da execução.
- Colocar o runtime em `apps/desktop/src-tauri`: acopla o runtime à UI,
  contrariando “React → Tauri → Rust Runtime → Orchestrator Core”.

## Decisão

Adicionar `packages/runtime` (crate `orchestrator-runtime`) contendo o Tool
Runtime: dispatcher (`ToolRuntime::invoke`), catálogo e os módulos
`filesystem`, `shell`, `terminal`, `process`.

- `packages/core` (crate `orchestrator-core`) contém apenas contratos:
  `ToolCall`, `ToolResult`, `AuditEvent`, `StreamEvent`, `EventSink`, IDs.
- `packages/git` continuará dono da lógica Git (Fase 2); o runtime apenas
  registra as operações `git.*` e delega a ele.

## Consequências

- Dependências: `desktop → runtime → core`; futuramente
  `orchestrator/agents/providers → core` e `orchestrator → runtime`.
- Providers dependem de `core` (para emitir `ToolCall`), nunca de `runtime`.

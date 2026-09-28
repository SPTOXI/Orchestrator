# Architecture Decision Records

Toda decisão que altera a estrutura definida no documento mestre, ou que
adiciona algo fora dela, é registrada aqui **antes** de ser implementada.

| ADR | Título | Estado |
| --- | ------ | ------ |
| [0001](./0001-monorepo-hibrido-nucleo-em-rust.md) | Monorepo híbrido (Cargo + pnpm) com núcleo em Rust | Aceita |
| [0002](./0002-pacote-runtime-para-o-tool-runtime.md) | Pacote `packages/runtime` para o Tool Runtime | Aceita |
| [0003](./0003-gateway-ipc-unico.md) | Gateway IPC único (`runtime_invoke`) e canal de streaming do terminal | Aceita |
| [0004](./0004-operacoes-auxiliares-do-tool-runtime.md) | Operações auxiliares do Tool Runtime | Aceita |
| [0005](./0005-observabilidade-antes-do-sqlite.md) | Observabilidade antes do SQLite (JSONL + eventos) | Aceita |
| [0006](./0006-ci-multiplataforma.md) | CI multiplataforma | Aceita |

Formato: Contexto → Decisão → Consequências.

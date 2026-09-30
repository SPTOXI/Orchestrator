# Architecture Decision Records

Toda decisão que altera a estrutura definida no documento mestre, ou que
adiciona algo fora dela, é registrada aqui **antes** de ser implementada.

| ADR | Título | Estado |
| --- | ------ | ------ |
| [0001](./0001-monorepo-hibrido-nucleo-em-rust.md) | Monorepo híbrido (Cargo + pnpm) com núcleo em Rust | Aceita |
| [0002](./0002-pacote-runtime-para-o-tool-runtime.md) | Pacote `packages/runtime` para o Tool Runtime | Aceita |
| [0003](./0003-gateway-ipc-unico.md) | Gateway IPC único (`runtime_invoke`) e canal de streaming do terminal | Aceita |
| [0004](./0004-operacoes-auxiliares-do-tool-runtime.md) | Operações auxiliares do Tool Runtime | Aceita |
| [0005](./0005-observabilidade-antes-do-sqlite.md) | Observabilidade antes do SQLite (JSONL + eventos) | Aceita (JSONL: ver 0012) |
| [0006](./0006-ci-multiplataforma.md) | CI multiplataforma | Aceita |
| [0007](./0007-git-via-cli-do-sistema.md) | Git via CLI do sistema | Aceita |
| [0008](./0008-projeto-deteccao-e-diretorio-base.md) | Projeto: detecção no runtime, diretório base e ferramentas auxiliares | Aceita |
| [0009](./0009-camada-de-providers-e-sessoes.md) | Camada de providers: `AIProvider`, registro e sessões | Aceita (adapters: ver 0010) |
| [0010](./0010-providers-por-api-com-cadastro-livre.md) | Providers por API com cadastro livre e nova ordem das Fases 4–5 | Aceita |
| [0011](./0011-roteador-de-modelos-e-conselho.md) | Roteador de modelos e Conselho de IAs | Aceita |
| [0012](./0012-sqlite-memoria-e-historico.md) | SQLite, memória do projeto e histórico | Aceita |
| [0013](./0013-context-builder-e-handoff.md) | Context Builder e Handoff entre IAs | Aceita |
| [0014](./0014-task-manager.md) | Task Manager | Aceita |
| [0015](./0015-agentes-subagentes-e-file-locks.md) | Agentes, subagentes e File Locks | Aceita |
| [0016](./0016-autonomia-e-pause.md) | Autonomia: Assistido, Autônomo, Acesso Irrestrito e Pause | Aceita |
| [0017](./0017-github-e-operacoes-remotas.md) | GitHub, pull requests e operações remotas | Aceita |
| [0018](./0018-tokens-cache-compactacao-e-escalonamento.md) | Tokens, cache, compactação de contexto e escalonamento de agentes | Aceita |

Formato: Contexto → Decisão → Consequências.

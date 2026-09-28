# packages/runtime — `orchestrator-runtime`

**Tool Runtime** do Orchestrator (ADR-0002): o único componente que executa
operações no sistema operacional. UI e agentes chamam
`ToolRuntime::invoke(ToolCall)`; toda chamada gera eventos de auditoria.

| Módulo | Ferramentas |
| ------ | ----------- |
| `filesystem` | `filesystem.list`, `.read`, `.write`, `.move`, `.delete` |
| `shell` | `shell.execute`, `shell.list` (detecção de PowerShell, CMD, WSL, Git Bash, bash, zsh, fish, sh) |
| `terminal` | `terminal.create`, `.write`, `.read`, `.close`, `.list` (PTY real via `portable-pty`) |
| `process` | `process.start`, `.stop`, `.list`, `.read` (encerra a árvore de processos) |
| `output` | buffers com offset e decodificador UTF-8 incremental |

Referência dos argumentos e saídas: [`docs/tool-runtime.md`](../../docs/tool-runtime.md).

```bash
cargo test -p orchestrator-runtime
```

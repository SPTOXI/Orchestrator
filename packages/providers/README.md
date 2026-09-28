# packages/providers

**AI Provider Layer** — abstração `AIProvider` e adapters por fornecedor.

```text
AIProvider: start · resume · execute · stream · cancel · spawnAgent · inspect · capabilities
```

| Responsabilidade | Fase |
| ---------------- | ---- |
| Trait `AIProvider`, Provider Registry, Provider Sessions | 3 |
| `openai/` — OpenAI / Codex | 4 |
| `claude/` — Claude Code | 5 |

Regras:

- Nenhuma chamada específica de fornecedor fora do adapter correspondente.
- Providers nunca executam operações de sistema: produzem `ToolCall`
  (`orchestrator-core`) e o Orchestrator executa via Tool Runtime.
- Novos providers (Gemini, modelos locais) entram como novos crates sem
  alterar o núcleo.

Ainda não implementado — vira crate Rust na Fase 3 (ver ADR-0001).

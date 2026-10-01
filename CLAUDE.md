# Orchestrator — instruções para agentes

Regras específicas deste repositório. Regras gerais de estilo por linguagem
(Rust, TypeScript, React) e de segurança ficam na configuração global de cada
pessoa — por exemplo, as do [ECC](https://github.com/affaan-m/ECC) em
`~/.claude/rules/`. Quando uma regra global conflitar com este documento,
vale este documento.

## Regra de ouro

Os princípios da seção 1 do [`ARCHITECTURE.md`](./ARCHITECTURE.md) valem
acima de qualquer outra regra. Os que mais pesam no dia a dia:

- Nenhum módulo do núcleo depende de um provider específico (OpenAI,
  Anthropic…); todos passam pela interface `AIProvider`.
- Providers pedem `tool_call`; quem executa é o Tool Runtime.
- Logs, histórico e auditoria são sempre registrados, inclusive em Acesso
  Irrestrito.

## Decisões e fases

- Toda mudança estrutural, ou que adicione algo fora do `ARCHITECTURE.md`,
  ganha um ADR em [`docs/adr/`](./docs/adr) **antes** de ser implementada
  (Contexto → Decisão → Consequências) e entra na tabela do README dos ADRs.
- O trabalho é organizado em fases; cada fase termina com um relatório em
  [`docs/phases/`](./docs/phases).
- Commits seguem o padrão do histórico, em português:
  `Fase N: <resumo>` para trabalho de fase, `<Área>: <resumo>` para o resto
  (ex.: `Runtime: …`, `CI: …`). Não use `feat:`/`fix:`.

## Antes de commitar

Rode o que o CI cobra:

```bash
pnpm check      # typecheck + cargo fmt --check + clippy -D warnings
pnpm test       # cargo test --workspace + testes da interface
```

## Testes

- Funcionalidade nova ou bug corrigido vem com teste. Não é exigido escrever
  o teste antes (TDD) nem uma porcentagem de cobertura.
- Testes Rust ficam em `packages/*/tests/` ou em módulos `#[cfg(test)]`;
  testes da interface ficam ao lado do módulo testado (`*.test.ts`).

## Segurança

O Orchestrator executa comandos, mexe no sistema de arquivos e guarda chaves
de API. Por isso:

- Segredos só passam pelo cofre (`vault.rs`, `secrets.rs`); nunca em código,
  logs, histórico ou mensagens de erro.
- Toda operação destrutiva ou remota passa pelo gate de autonomia (ADR-0016).
- Valide caminhos e argumentos recebidos de IAs antes de executar.

O app é desktop (Tauri) e não expõe servidor HTTP: regras de CSRF ou rate
limiting por endpoint não se aplicam.

## Tamanho de arquivo

Arquivos acima de ~800 linhas são um alerta, não um bloqueio. Não divida
arquivos existentes no meio de outra tarefa; proponha a divisão à parte.

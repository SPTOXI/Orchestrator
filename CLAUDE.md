# Orchestrator — instruções para agentes

## Regra geral

Todo agente que trabalha neste repositório segue as regras do
[ECC](https://github.com/affaan-m/ECC), copiadas para
[`.claude/rules/ecc/`](./.claude/rules/ecc):

- [`common/`](./.claude/rules/ecc/common): estilo, testes, segurança,
  revisão de código, fluxo de desenvolvimento e de git, desempenho e padrões;
  valem para qualquer arquivo.
- [`rust/`](./.claude/rules/ecc/rust): valem para `**/*.rs` (núcleo em
  `packages/*` e `apps/desktop/src-tauri`).
- [`typescript/`](./.claude/rules/ecc/typescript) e
  [`react/`](./.claude/rules/ecc/react): valem para a interface em
  `apps/desktop/src`.

O Claude Code carrega `.claude/rules/` sozinho; outros agentes leem os mesmos
arquivos a partir deste documento ou do [`AGENTS.md`](./AGENTS.md).

### Precedência

Quando uma regra do ECC conflitar com este repositório, vale o repositório:

1. Os princípios de [`ARCHITECTURE.md`](./ARCHITECTURE.md) (regra de ouro).
2. As decisões registradas em [`docs/adr/`](./docs/adr) — toda mudança
   estrutural ganha um ADR antes.
3. As convenções já presentes no código ao redor.
4. As regras do ECC.

Referências do ECC a agentes, comandos e hooks do plugin `ecc@ecc` só se
aplicam quando o plugin estiver instalado; sem ele, siga o princípio descrito.

### Atualizar as regras

As regras vêm do commit `c70874f` do ECC (licença MIT, em
[`.claude/rules/ecc/LICENSE`](./.claude/rules/ecc/LICENSE)). Para atualizar,
copie de novo os diretórios inteiros — sem achatar, porque os arquivos de
linguagem apontam para `../common/`:

```bash
git clone --depth 1 https://github.com/affaan-m/ECC.git /tmp/ecc
for d in common rust typescript react; do
  rm -rf .claude/rules/ecc/$d && cp -r /tmp/ecc/rules/$d .claude/rules/ecc/
done
cp /tmp/ecc/LICENSE .claude/rules/ecc/LICENSE
```

e troque o commit citado acima.

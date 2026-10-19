# ADR-0026 — Economia de tokens nas ferramentas e nos pedidos

- **Estado:** Aceita
- **Fase:** depois da 12 (pedido do usuário)
- **Complementa:** a [ADR-0018](./0018-tokens-cache-compactacao-e-escalonamento.md)

## Contexto

O usuário pediu uma revisão do código para gastar menos tokens nas
conexões de API e nas CLIs. A ADR-0018 já cobria o cache de prompt, a
compactação e os tetos de custo. A revisão achou o que ainda ia à IA sem
necessidade:

- os resultados de ferramentas iam como JSON com escapes (`\n`, `\"`) e,
  pelo MCP, indentados e sem limite de tamanho;
- toda sessão recebia as definições de todas as ferramentas (só as do
  runtime somavam ~31 mil caracteres), com schemas cheios de `null`,
  `"default": null` e formatos de inteiro;
- resultados grandes antigos ficavam na conversa até a compactação;
- pedidos auxiliares (resumo, Conselho, teste) pediam a saída máxima do
  modelo;
- o Claude Code pela assinatura carregava os skills do usuário no prompt;
- não havia como ver do que a entrada de cada pedido era feita.

## Decisão

1. **Resultados em menos tokens** (`render_for_model`, `truncate_for_model`
   em `orchestrator-core`): texto cru; campos longos com várias linhas como
   blocos crus; listas de 3+ objetos simples como tabela; o resto em JSON
   compacto; corte no começo e no fim. O mesmo para API e MCP.
2. **Limite por conexão** (`maxToolResultChars`): padrão de ~10% da janela
   de contexto do modelo, entre 4.000 e 40.000 caracteres.
3. **Ferramentas por sessão** (`ContextOptions.toolGroups`): só os grupos
   escolhidos vão à IA, sempre na mesma ordem; chamadas a outros grupos são
   recusadas sem executar; subagentes herdam os grupos do pai; a escolha
   só muda antes do primeiro turno.
4. **Schemas enxutos** (`slim_schema`): opcionais sem a alternativa `null`,
   sem `"default": null`, sem formatos de inteiro e sem `title`/`$schema`;
   também nas ferramentas dos servidores MCP conectados.
5. **Resultados antigos saem da conversa** (`clearToolResults`, ligado por
   padrão): na metade do limite da compactação, resultados grandes de
   rodadas anteriores às 3 últimas viram uma nota de uma linha, só quando
   isso tira ao menos um quarto do limite.
6. **Saída máxima dos pedidos auxiliares:** resumo 16.000, Conselho 8.000,
   teste de conexão 1.024 tokens; um limite configurado menor vale.
7. **Claude Code sem skills** (`--disable-slash-commands`) quando só as
   ferramentas do Orchestrator estão ligadas e a versão tem a opção.
8. **Composição do prompt** (`TokenUsage.prompt`, `SpendReport.prompt`):
   estimativa de instruções, ferramentas, conversa e resultados por pedido,
   na aba "Tokens e custo".
9. **Regras do projeto sem repetição:** o `CLAUDE.md` não vai ao Claude
   Code nem o `AGENTS.md` ao Codex (as CLIs já os leem da pasta do
   projeto), e um arquivo de regras repetido inteiro em outro vai uma vez.

Detalhes em [`docs/tokens.md`](../tokens.md).

## Consequências

- Menos tokens em todo pedido, sem mudar o que a IA pode fazer.
- Conjuntos diferentes de grupos de ferramentas gravam caches diferentes
  na Anthropic; por isso a escolha é por sessão e fica fixa.
- Limpar resultados antigos recomeça o cache dali em diante; por isso
  acontece em blocos.
- A IA pode precisar chamar de novo uma ferramenta cujo resultado saiu.
- A composição é uma estimativa (4 caracteres por token) e só existe nas
  conexões de API: as CLIs não mostram o prompt que montam.
- `TokenUsage` cresceu 16 bytes (`PromptBreakdown` em `u32`) para não
  passar do limite do `clippy::result_large_err`.

**Fica para depois**

- Limite de resultado configurável para as CLIs.
- Medir o ganho real por fornecedor.

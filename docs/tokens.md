# Tokens, custo, cache, compactação e escalonamento

Referência da Fase 11. A decisão está na
[ADR-0018](./adr/0018-tokens-cache-compactacao-e-escalonamento.md).

O princípio é a seção 23 do documento mestre: nada vai para a IA só porque
existe. O contexto do projeto vai uma vez, com orçamento e só com caminhos
de arquivos (Fase 7). A partir desta fase, o que se repete em cada
requisição vem do cache do fornecedor, conversas longas viram um resumo,
os agentes respeitam limites de custo e tudo o que foi gasto fica visível.

Tudo vale para os quatro protocolos (OpenAI e compatíveis, Anthropic,
Gemini e o perfil genérico). O que é próprio de um fornecedor é otimização
a mais, nunca o único caminho.

## Custo real

Cada turno grava em `TokenUsage`:

| Campo | O que é |
| ----- | ------- |
| `inputTokens` | o prompt inteiro |
| `cachedInputTokens` | a parte do prompt lida do cache |
| `cacheWriteTokens` | a parte gravada no cache (Anthropic) |
| `outputTokens` | a saída |
| `costUsd` | o que o fornecedor cobra, com o cache; `null` sem preço |
| `cacheSavedUsd` | quanto o cache economizou (negativo: gravou e não leu de volta) |

```text
custo    = (entrada − lida − gravada) × preço de entrada
         + lida × preço do cache + gravada × preço de gravação
         + saída × preço de saída
economia = lida × (entrada − cache) − gravada × (gravação − entrada)
```

- **Preço do cache** é o campo `cachedInputPrice` do modelo (US$ por milhão
  de tokens lidos do cache), na coluna "$ cache" da conexão. Sem ele, a
  leitura custa o preço cheio da entrada, que é a conta conservadora. Na
  Anthropic, sem o campo, a leitura custa 10% da entrada (regra do
  fornecedor).
- **Gravação** (Anthropic) custa 1,25× a entrada com validade de 5 minutos e
  2× com 1 hora — regra do fornecedor, sem campo próprio.
- "Buscar modelos" preenche o preço do cache dos modelos Claude pela tabela
  de referência ([api-connections.md](./api-connections.md#uso-e-custo)).
  Nos outros fornecedores, o usuário informa o preço.

A sessão mostra, por turno e no total: "53.120 tokens (53.000 in / 120 out ·
30.000 do cache) · US$ 0.0798 · cache economizou US$ 0.0810".

## Cache de prompt

O cache dos fornecedores compara o começo da requisição byte a byte; só
funciona com **prefixo estável**. O Orchestrator garante isto (e um teste
confere que duas requisições seguidas repetem o prefixo):

- as instruções de sistema ficam congeladas depois do primeiro turno (o
  contexto do projeto entra uma vez);
- a lista de ferramentas é fixa, na mesma ordem, com serialização
  determinística;
- a conversa só cresce, até ser compactada.

| Protocolo | O que o Orchestrator faz |
| --------- | ------------------------ |
| Anthropic | Três marcadores `cache_control`: na última ferramenta (as ferramentas, iguais em todas as sessões do mesmo modelo — compartilhadas **entre agentes**), no fim das instruções de sistema e no último bloco da última mensagem. Só em sessões; chamadas avulsas (teste de conexão, Conselho) não marcam. |
| OpenAI | O cache é automático. Na API oficial (`api.openai.com`), vai `prompt_cache_key` com a sessão, para as requisições da mesma conversa caírem no mesmo cache. APIs compatíveis podem recusar o campo; ele não vai para elas. |
| Gemini | Cache implícito do fornecedor; nada a enviar. |
| Genérica | Nada. |

Opções da conexão (`options`), no editor da conexão:

| Opção | Valores | Padrão |
| ----- | ------- | ------ |
| `promptCache` | `true`/`false` ("Cache de prompt") | ligado |
| `cacheTtl` | `5m` ou `1h` ("Validade", Anthropic) | `5m` |

Não existe cache de respostas de sessão: a mesma pergunta num momento
diferente do projeto merece outra resposta. O cache de deliberações do
Conselho (Fase 5) continua.

## Compactação de contexto

Conversas longas são **resumidas pela própria IA**, igual nos quatro
protocolos. O provider `echo` não compacta.

**Quando:**

- **Automática**, entre turnos ou entre rodadas de ferramentas — **nunca**
  com uma chamada de ferramenta esperando resultado. Dispara quando o
  prompt da última requisição (o que o fornecedor informou, ou uma
  estimativa) passa do limite: o menor entre `thresholdTokens` e
  `thresholdPercent` da janela de contexto do modelo. Não dispara quando
  as instruções e as ferramentas sozinhas já passam do limite (compactar
  não resolveria).
- **Manual**, no botão "Compactar" da sessão parada.

**Como:** a requisição do resumo é a própria conversa (mesmo modelo, mesmas
instruções, mesmas ferramentas) com o pedido de resumo no fim — por isso
ela lê o cache em vez de pagar o prompt de novo. O pedido diz o que
preservar (problemas e soluções, opções descartadas, o que o usuário pediu
e decidiu com as palavras dele, onde as coisas estão, o que falta) e para
não chamar ferramentas.

**Depois:** as instruções de sistema ficam; as mensagens anteriores saem; o
resumo abre a próxima mensagem do usuário (ou uma mensagem "continue"
entre rodadas). Nenhuma parte nativa antiga é reenviada — a forma que os
modelos com raciocínio preservado aceitam.

**Se falhar** (erro, texto vazio, só chamadas de ferramenta), a conversa
fica como estava, a sessão recebe um aviso e o turno segue.

**Registro:** o transcript mostra "Conversa compactada automaticamente · 6
mensagens · ~30.060 → ~9.350 tokens" com o resumo (expansível); o
histórico ganha `CONTEXT_COMPACTED`, só com os números. A tela da sessão
continua mostrando a conversa inteira: só o que vai para a IA é compactado.

**Configuração** em `<app-data>/context.json`, na aba Contexto:

```json
{ "compaction": { "auto": true, "thresholdTokens": 150000, "thresholdPercent": 80 } }
```

| Ajuste | Padrão | Faixa |
| ------ | ------ | ----- |
| `auto` | `true` | — |
| `thresholdTokens` | 150.000 | 8.000–2.000.000 |
| `thresholdPercent` | 80 | 10–95 |

## Retentativas

A requisição ao modelo é repetida até **duas vezes** quando recebe 408,
429, 500, 502, 503, 504 ou 529, ou quando a conexão falha antes da
resposta. A espera é a que o fornecedor pede (`retry-after` ou
`retry-after-ms`, até 60 s) ou 2 s e 4 s. A sessão mostra "rate limit or
quota exceeded (HTTP 429): … — tentando de novo em 1 s (1 de 2)", e
cancelar interrompe a espera.

**Crédito que não paga a resposta (402).** Sem limite de saída na conexão,
o OpenRouter supõe a resposta mais longa que o modelo permite e recusa se o
crédito (ou o limite da chave) não a paga: "You requested up to 131072
tokens, but can only afford 42012". O Orchestrator repete o pedido na hora,
uma vez, pedindo no máximo **90% do que o crédito paga** (37.810 no
exemplo), e avisa na sessão. O limite aprendido vale para os pedidos
seguintes do mesmo modelo por 30 minutos (o crédito muda) e nunca sobe um
limite já configurado. Se o crédito não paga nem 1.024 tokens, o erro diz o
que fazer: um modelo gratuito (`:free`, no OpenRouter) ou mais crédito.

## Escalonamento de agentes

A fila do Agent Manager ([agents.md](./agents.md)):

- **Ordem:** a prioridade da task (Urgente, Alta, Normal, Baixa), depois a
  ordem de chegada. Um agente que não pode começar (arquivo travado, limite
  do provider) **não segura os de trás**.
- **Posição e motivo:** o painel mostra "1º na fila · a conexão nuvem-cache
  já tem 1 agente (limite)".
- **Limite por provider** (`providerLimits`): quantos agentes de uma
  conexão rodam ao mesmo tempo, para respeitar os limites de taxa da conta.
- **Afinidade:** subagentes seguem o provider e o modelo do pai — mesmos
  preços e, na Anthropic, o mesmo cache de ferramentas.

### Tetos de custo

- **Por agente** (`maxCostUsd`): o padrão das configurações, ou outro ao
  executar ("teto US$" na aba da task). É conferido antes de cada turno; o
  agente que chegou ao teto para com handoff, e o motivo fica no agente e
  no `AGENT_FINISHED` (`reason: "costCeiling"`): "o agente chegou ao teto
  de custo de US$ 0,100 (gastou US$ 0,104)".
- **Orçamento diário do projeto** (`dailyBudgetUsd`): o gasto do projeto
  desde a meia-noite (hora local), somando **todas** as IAs — sessões,
  agentes e Conselho. Quando acaba:
  - nenhum agente começa ("o orçamento diário do projeto acabou (US$ 0,315
    de US$ 0,300)");
  - os que estão rodando param no fim do turno, com handoff
    (`reason: "dailyBudget"`);
  - **as sessões do usuário continuam** — o usuário nunca é bloqueado.
  Subir o orçamento libera a fila na hora.
- **Sem preço**, o custo é desconhecido: os tetos não se aplicam a essas
  chamadas, e a UI diz quantas chamadas ficaram sem preço ("+?").

Os tetos são conferidos entre turnos: um turno pode passar do teto. O teto
de turnos e o de rodadas de ferramentas limitam quanto.

### Subagentes

`maxSubagents` (padrão 5, até 10; 0 desliga a delegação). A descrição de
`agent.delegate` diz à IA que só vale delegar uma subtask independente e
grande.

### Configuração

`<app-data>/agents.json`, editável no painel AGENTS (seção LIMITES):

```json
{
  "version": 1,
  "agents": {
    "maxParallel": 2,
    "maxTurns": 12,
    "maxSubagents": 5,
    "providerLimits": { "openai-pessoal": 1 },
    "maxCostUsd": 0.5,
    "dailyBudgetUsd": 5.0
  }
}
```

| Ajuste | Padrão | Faixa |
| ------ | ------ | ----- |
| `maxSubagents` | 5 | 0–10 |
| `providerLimits` | `{}` (sem limite além do `maxParallel`) | 1–8 por conexão |
| `maxCostUsd` | `null` (sem teto) | > 0 |
| `dailyBudgetUsd` | `null` (sem orçamento) | > 0 |

## Onde ver o gasto

- **Barra de status:** "hoje: US$ 0,21" (ou "hoje: US$ 0,585 de US$ 0,58",
  em vermelho quando o orçamento acabou; "+?" quando há chamadas sem preço).
  O clique abre a aba "Tokens e custo".
- **Painel AGENTS:** o gasto de hoje contra o orçamento, o custo de cada
  agente e o aviso quando o orçamento acabou.
- **Aba "Tokens e custo":** hoje, 7 ou 30 dias, do projeto ou de todos os
  projetos — gasto, quanto da entrada veio do cache e quanto economizou,
  tokens, chamadas, compactações e a tabela por provider e modelo.

O gasto sai do histórico (`TURN_COMPLETED`, que passa a levar o `model`, e
`COUNCIL_DELIBERATED`, sem as respostas que vieram do cache do Conselho):
nada é guardado duas vezes e **não há migração** (o banco continua no
esquema 4).

## IPC

| Comando | Argumentos | Retorno |
| ------- | ---------- | ------- |
| `spend_report` | `days`, `projectId?`, `allProjects?` | `SpendReport` (totais, compactações e `rows` por provider e modelo, o mais caro primeiro) |
| `agents_budget` | `projectId?` | `BudgetView` (`spentTodayUsd`, `budgetUsd`, `unpriced`, `exhausted`) |
| `session_compact` | `id` | `TurnResult`; recusa sessão ocupada, fechada ou sem conversa |

Ver também [ipc.md](./ipc.md#tokens-e-custo-fase-11-adr-0018).

## Limitações

- A compactação perde detalhes: o resumo é tudo o que a IA tem do que veio
  antes. O usuário continua vendo a conversa inteira, o histórico guarda os
  fatos e a IA pode consultar a memória do projeto.
- Compactar recomeça o cache das mensagens; por isso o limite padrão é alto.
- Sem compactação ou edição de contexto do lado do servidor, sem cache
  explícito do Gemini.
- Orçamento só por dia e por projeto (não por mês, provider ou task), e
  conferido entre turnos.
- Uma retentativa depois de um 5xx pode cobrar duas vezes uma requisição
  que o fornecedor chegou a processar (raro; os SDKs oficiais fazem o
  mesmo).
- Sem escalonamento por previsão de custo ou duração.

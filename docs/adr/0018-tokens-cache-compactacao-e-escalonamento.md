# ADR-0018 — Tokens, cache, compactação de contexto e escalonamento de agentes

- **Estado:** Aceita
- **Fase:** 11

## Contexto

A seção 23 do documento mestre ("Token e custo") pede um sistema projetado
para reduzir consumo desnecessário. Ele não deve mandar automaticamente
todo o histórico, todo o repositório, todos os arquivos nem todas as
mensagens. Deve preferir contexto relevante, resumos, busca, cache,
compactação e contexto por task. Tarefas simples usam um agente, e
subagentes só entram quando se justificam. A Fase 11 é
"Token optimization, Caching, Context compaction, Agent scheduling".

O que já existe:

- o Context Builder (ADR-0013) manda um contexto com orçamento, no
  primeiro turno, com caminhos de arquivos e nunca o conteúdo;
- o resultado de uma ferramenta é cortado em 40.000 caracteres antes de ir
  para a IA;
- o Conselho guarda deliberações em cache (ADR-0011);
- o uso de cada turno é somado. Os tokens lidos do cache do fornecedor já
  são contados, mas **cobrados a preço cheio** no custo mostrado;
- a conversa de uma sessão de API **só cresce**: cada requisição reenvia
  tudo, e nenhuma requisição marca o que o fornecedor pode guardar em
  cache;
- a fila de agentes é por ordem de chegada, com teto de agentes em
  paralelo e de turnos por agente. Um 429 do fornecedor derruba o turno;
- ADRs anteriores deixaram para esta fase o cache e a compactação
  (ADR-0013), o escalonamento por custo/afinidade e o cache entre agentes
  (ADR-0015), o orçamento de custo e a supervisão de processos contra
  queda do app (ADR-0016).

Os princípios que valem aqui:

- **nenhuma solução funciona só com uma IA** (seção 28): todo mecanismo
  vale para os quatro protocolos. O que é próprio de um fornecedor é
  otimização a mais, nunca o único caminho;
- **o usuário nunca é bloqueado no próprio projeto** (ADR-0015);
- **tudo que gasta é visível e auditado.**

## Decisão

### 1. Custo real

O custo mostrado passa a ser o que o fornecedor cobra, com o cache.

- `TokenUsage`:
  - `inputTokens` continua sendo o prompt inteiro;
  - `cachedInputTokens` é o que foi lido do cache;
  - **novo** `cacheWriteTokens`: o que foi gravado no cache (Anthropic);
  - **novo** `cacheSavedUsd`: quanto o cache economizou (ou custou)
    naquela requisição.
- Modelo da conexão: **novo** `cachedInputPrice` (US$ por milhão de tokens
  lidos do cache). Sem ele, a leitura custa o preço cheio da entrada, que
  é a conta conservadora. Na Anthropic, a leitura custa 10% da entrada,
  que é a regra do fornecedor.
- Gravação no cache (Anthropic) custa 1,25× a entrada com validade de
  5 minutos e 2× com 1 hora. É a regra do fornecedor, sem campo próprio.

```text
custo    = (entrada − lida − gravada) × preço de entrada
         + lida × preço do cache + gravada × preço de gravação
         + saída × preço de saída
economia = lida × (entrada − cache) − gravada × (gravação − entrada)
```

"Buscar modelos" preenche o preço do cache dos modelos Claude pela tabela
de referência. Nos outros fornecedores, o usuário informa o preço.

### 2. Cache de prompt

O cache dos fornecedores só funciona com **prefixo estável**, porque é uma
comparação byte a byte do começo da requisição. O Orchestrator garante
isto, e um teste confere que duas requisições seguidas repetem o prefixo:

- as instruções de sistema ficam congeladas depois do primeiro turno (o
  contexto do projeto entra uma vez);
- a lista de ferramentas é fixa, na mesma ordem, com serialização
  determinística;
- a conversa só cresce, até ser compactada (item 3).

Por fornecedor:

| Protocolo | O que o Orchestrator faz |
| --------- | ------------------------ |
| Anthropic | Três marcadores `cache_control`: na última ferramenta (as ferramentas, iguais em todas as sessões do mesmo modelo, logo compartilhadas **entre agentes**), no fim das instruções de sistema e no último bloco da última mensagem (a conversa até ali). Opções da conexão: `promptCache` (padrão ligado) e `cacheTtl` (`5m`, o padrão, ou `1h`). |
| OpenAI | O cache é automático no fornecedor. Na API oficial (`api.openai.com`), o Orchestrator manda `prompt_cache_key` com a sessão, para as requisições da mesma conversa caírem no mesmo cache. APIs compatíveis podem recusar o campo, então ele não vai para elas. `promptCache: false` desliga. |
| Gemini | O cache implícito é do fornecedor; nada a enviar. O cache explícito (`cachedContents`) fica de fora. |
| Genérica | Nada. |

Não existe cache de respostas de sessão: a mesma pergunta num momento
diferente do projeto merece outra resposta. O cache do Conselho continua.

### 3. Compactação de contexto

**Onde:** no adapter das conexões de API, que é o dono da conversa, igual
para os quatro protocolos. O provider `echo` não compacta.

**Quando:**

- **Automático.** A compactação acontece entre turnos ou entre rodadas de
  ferramentas, **nunca** com uma chamada de ferramenta esperando
  resultado. Dispara quando o prompt da última requisição passa do
  limite. O tamanho do prompt é o que o fornecedor informou (ou uma
  estimativa, sem essa informação). O limite é o menor entre
  `thresholdTokens` (padrão 150.000) e `thresholdPercent` (padrão 80%) da
  janela de contexto do modelo.
- **Manual.** "Compactar" na sessão parada.

**Como (compactação simples):** a mesma IA resume a conversa.

- A requisição do resumo é a própria conversa, com o mesmo modelo, as
  mesmas instruções e as mesmas ferramentas, mais o pedido de resumo no
  fim. Por isso ela lê o cache em vez de pagar o prompt de novo.
- O pedido diz o que preservar:
  - problemas e como foram resolvidos;
  - opções tentadas ou descartadas;
  - o que o usuário pediu, decidiu ou restringiu, com as palavras dele;
  - onde as coisas estão e o que falta;
  - detalhes difíceis de reconstruir.
- O pedido também diz para não chamar ferramentas.

**Resultado:** a conversa vira só o resumo.

- As instruções de sistema, com o contexto do projeto, ficam.
- As mensagens anteriores saem. O resumo entra no começo da próxima
  mensagem do usuário, ou de uma mensagem "continue" entre rodadas.
- Nenhuma parte nativa antiga é reenviada, como os blocos de raciocínio
  assinados. É a forma que os modelos que conferem o histórico
  ("preserved thinking") aceitam, e não depende de fornecedor.

**Falha:** se o resumo falhar (erro, texto vazio, só chamadas de
ferramenta), a conversa fica como estava, a sessão recebe um aviso e o
turno segue.

**Custo:** a saída do resumo mais a leitura do prompt, que vem do cache.
Depois, o cache das mensagens recomeça. O das ferramentas e instruções
continua (Anthropic).

**Registro:**

- a transcrição da sessão guarda "Conversa compactada", com o resumo e os
  tokens antes e depois;
- o histórico ganha **`CONTEXT_COMPACTED`**, só com os números;
- a tela da sessão continua mostrando tudo. Só o que vai para a IA é
  compactado.

**Configuração:** `context.json` ganha
`compaction { auto, thresholdTokens, thresholdPercent }`, na aba Contexto.

**Por que não:**

- **Compactação ou edição de contexto do lado do servidor** (Anthropic):
  seria de um fornecedor só.
- **"Manter o fim da conversa" ou apagar resultados antigos:** editar o
  meio do histórico invalida o cache de tudo o que vem depois. Nos
  modelos Claude recentes, também invalida as assinaturas de raciocínio
  (erro 400 em contas novas). A compactação simples não tem esses
  problemas.

### 4. Escalonamento de agentes

**Ordem da fila:** a prioridade da task (Urgente, Alta, Normal, Baixa),
depois a ordem de chegada. Um agente que não pode começar não segura os de
trás. O painel mostra a posição na fila e o motivo da espera.

**Limite por provider:** `providerLimits` em `agents.json`, por exemplo
`{"openai-pessoal": 1}`. Serve para respeitar limites de taxa da conta.

**Tetos de custo:**

- `maxCostUsd` por agente: vem das configurações, e o usuário pode dar
  outro ao executar. É conferido antes de cada turno. O agente que passa
  do teto para, com handoff.
- `dailyBudgetUsd` por projeto: é o gasto do projeto no dia, somando
  todas as IAs (sessões, agentes e Conselho).
  - Ao chegar ao orçamento, nenhum agente começa, e os que rodam param no
    fim do turno, com handoff.
  - **As sessões do usuário não são bloqueadas.**
- Sem preço configurado, o custo é desconhecido: os tetos não se aplicam,
  e a UI diz isso.

**Subagentes:** `maxSubagents` (padrão 5; 0 desliga a delegação) no lugar
da constante. A descrição de `agent.delegate` diz que só vale delegar uma
subtask independente e grande (seção 23).

**Retentativas:**

- A requisição ao modelo é repetida até duas vezes quando:
  - recebe 408, 429, 500, 502, 503, 504 ou 529 (sobrecarga);
  - ou a conexão falha antes da resposta.
- A espera é o que o fornecedor pede (`retry-after`, até 60 s) ou 2 s e
  4 s.
- A sessão mostra "tentando de novo em N s", e cancelar interrompe a
  espera.

**Afinidade:** subagentes seguem o provider e o modelo do pai. Mesmo
modelo significa mesmos preços e, na Anthropic, o cache das ferramentas
compartilhado.

### 5. Supervisão de processos

- **Windows:** cada processo iniciado por `process.start` e `shell.exec`
  entra num Job Object do runtime, com "encerrar ao fechar". Se o app
  morrer, o Windows encerra o processo e os filhos dele.
- **Linux e macOS:** não há equivalente portátil.
  - O runtime anota os grupos de processos que inicia em
    `<app-data>/processes.json`: pid, horário de início e comando.
  - Ao abrir, ele encerra os que sobraram de uma execução que caiu.
    Antes, confere o horário de início, para não matar um processo que
    reaproveitou o pid.
  - O histórico registra `PROCESS_EXITED` com `reason: "orphan"`.
- Terminais continuam como estão: o PTY fecha junto com o app.

### 6. Interface

- **Sessão:** tokens, quanto veio do cache, custo e economia, o botão
  "Compactar" e o resumo na transcrição.
- **Conexão:** preço do cache por modelo e as opções de cache.
- **Aba Contexto:** compactação automática e os limites.
- **Painel AGENTS:** posição na fila e motivo da espera, custo de cada
  agente, e as configurações de agentes (limites por provider,
  subagentes, teto por agente e orçamento diário).
- **Aba "Tokens e custo":** gasto do projeto hoje e nos últimos 7 e 30
  dias, por provider e modelo, com tokens lidos do cache, economia e
  compactações.

### 7. Eventos e persistência

- Novo evento: `CONTEXT_COMPACTED`.
- `TURN_COMPLETED` passa a levar o `model`.
- `AGENT_FINISHED` leva o `reason` quando o agente parou por teto de custo
  ou orçamento.
- **Nenhuma migração (o banco continua no esquema 4):**
  - o gasto sai do histórico (`TURN_COMPLETED` e `COUNCIL_DELIBERATED`);
  - o teto do agente vai no JSON do agente;
  - as configurações ficam em `agents.json` e `context.json`;
  - a conversa compactada vai no snapshot da sessão.

## Consequências

- Sessões longas passam a custar a leitura do cache em vez do prompt
  inteiro a cada requisição. O custo mostrado bate com o do fornecedor
  quando os preços estão preenchidos.
- **A compactação perde detalhes:** o resumo é tudo o que a IA tem do que
  veio antes. O usuário continua vendo a conversa inteira, o histórico
  guarda os fatos e a IA pode consultar a memória do projeto.
- Compactar recomeça o cache das mensagens. Por isso o limite padrão é
  alto e a compactação é rara.
- Os tetos de custo são conferidos entre turnos: um turno pode passar do
  teto. O teto de turnos e o de rodadas de ferramentas limitam quanto.
- Uma retentativa depois de um 5xx pode cobrar duas vezes uma requisição
  que o fornecedor chegou a processar. É raro, e os SDKs oficiais fazem o
  mesmo.
- No Linux e no macOS, os processos que sobram de uma queda vivem até o
  app abrir de novo.

**Fica para depois**

- Compactação e edição de contexto do lado do servidor, como otimização
  da conexão Anthropic.
- Cache explícito do Gemini.
- Orçamento por mês, por provider ou por task; teto de custo dentro do
  turno.
- Escalonamento por previsão de custo ou duração.
- Busca semântica no contexto.

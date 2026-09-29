# Roteador de modelos e Conselho de IAs (Fase 5)

Referência de `packages/router` (crate `orchestrator-router`). Decisão
registrada em [ADR-0011](./adr/0011-roteador-de-modelos-e-conselho.md).

Com várias APIs cadastradas ([api-connections.md](./api-connections.md)),
escolher o modelo de cada tarefa vira trabalho manual. A Fase 5 automatiza a
escolha em dois níveis:

- **Roteador:** ordena todos os modelos por regras, sem gastar tokens, e
  explica cada nota.
- **Conselho:** de 1 a 5 modelos (provider + modelo) deliberam sobre os
  melhores candidatos do roteador e votam. Com um membro, ele é o
  "gerenciador".

```text
tarefa ──▶ atividade (detectada ou escolhida)
            │
            ▼
      ROTEADOR (regras, 0 tokens)
      filtros → notas 0–100 → ranking + excluídos
            │  K melhores (lista curta)
            ▼
      CONSELHO (modos Sugerir / Full)        cache (mesma pergunta = 0 tokens)
      membros em paralelo: AIProvider::complete (sem sessão, sem ferramentas)
      → votos JSON → Borda ponderada pela confiança → decisão + concordância
            │
            ▼
   Sugerir: o usuário aprova ou escolhe outro   Full: o Orchestrator abre a
   → route_start_session (ROUTE_DECIDED)        sessão e envia a tarefa sozinho
```

## Na interface

- **AI PROVIDERS → Conselho → Configurar:** modo, membros, candidatos para
  o Conselho, cache, prazo por membro, preferência padrão, enviar a tarefa
  como 1ª mensagem e deliberações recentes.
- **AI PROVIDERS → Nova sessão com o Conselho:** descreva a tarefa e,
  opcionalmente, escolha atividade, preferência, se as ferramentas são
  obrigatórias e o contexto mínimo (`128k`, `1M`…). Ctrl+Enter decide.
  O resultado mostra:
  - a **decisão**, com quem decidiu (Roteador, Conselho ou Conselho
    (cache)), o motivo, a concordância, e o uso e custo do Conselho;
  - os **votos** de cada membro: escolha, confiança, motivo, uso, tempo e
    abstenções com o erro;
  - o **ranking do roteador**, com a nota, os motivos e "Usar este" para
    iniciar com outro modelo;
  - os **excluídos**, cada um com o motivo.
- **HISTORY:** filtros `COUNCIL_CONFIGURED`, `COUNCIL_DELIBERATED` e
  `ROUTE_DECIDED`. A origem "Conselho (Full)" marca o que o Conselho
  aplicou sozinho.

## Modos

| Modo | Tokens | Quem decide | O que acontece |
| ---- | ------ | ----------- | -------------- |
| **Desligado** (`off`) | nenhum | o usuário | o roteador ordena e recomenda; o usuário inicia com o recomendado ou outro |
| **Sugerir** (`suggest`) | os do Conselho | o usuário aprova | o Conselho delibera e recomenda; "Aprovar e iniciar sessão" ou "Usar este" em outro modelo |
| **Full** (`full`) | os do Conselho | o Conselho | a sessão abre sozinha com o modelo decidido e recebe a tarefa como 1ª mensagem (origem `council`) |

Regras do Full:

- Só aplica decisões **do Conselho**. Se nenhum membro responder de forma
  válida, vale a recomendação do roteador e o usuário decide.
- Com um único candidato elegível, o Conselho não é consultado e a decisão é
  aplicada (não há escolha a fazer).
- Escolher o modelo **não** dá permissão para operações. O gate de
  autonomia (Assistido / Autônomo / Acesso Irrestrito) chega na Fase 9, e
  nas Fases 8–9 o mesmo serviço escolhe o modelo de tasks e agentes.
- A sessão aberta pelo Full recebe o contexto do projeto no primeiro turno,
  como qualquer sessão (Fase 7, [context.md](./context.md)). Os membros do
  Conselho continuam sem contexto, arquivos e ferramentas.
- O handoff (Fase 7) usa o roteador para sugerir quem assume, pelo objetivo
  e o que falta, sem gastar tokens.

Sugerir e Full exigem pelo menos um membro.

## Atividades

A atividade é detectada por palavras-chave (português e inglês) na
descrição. A primeira palavra, normalmente o verbo, conta em dobro:
"**Resuma** o README" é resumo, não documentação. O usuário pode escolher a
atividade.

| Atividade | Rótulo | Etiquetas afins | Ferramentas | Preferência |
| --------- | ------ | --------------- | ----------- | ----------- |
| `code` | Implementar código | código, code, coding, programação, dev | obrigatórias | equilíbrio |
| `debug` | Depurar um erro | debug, depuração, código, raciocínio | obrigatórias | qualidade |
| `review` | Revisar código | revisão, review, código, raciocínio | obrigatórias | qualidade |
| `tests` | Escrever testes | testes, tests, código | obrigatórias | equilíbrio |
| `planning` | Planejar / arquitetura | planejamento, arquitetura, raciocínio | opcionais | qualidade |
| `docs` | Documentação / escrita | escrita, docs, documentação, texto | obrigatórias | custo |
| `summary` | Resumo / pergunta rápida | resumo, rápido, barato, chat | opcionais | velocidade |
| `general` | Geral | geral, chat | opcionais | equilíbrio |

Etiquetas são comparadas sem acento e sem diferenciar maiúsculas
("Raciocínio" = "raciocinio").

## Roteador: pontuação

Entrada: todos os modelos **ativos** de todos os providers registrados, com
a disponibilidade do último `inspect`. Providers sem resultado recente são
inspecionados em paralelo antes de ordenar; o resultado vale 5 minutos, e
salvar ou remover uma conexão o descarta.

**Filtros**, com o motivo mostrado em "excluídos":

- provider indisponível (com o detalhe do `inspect`, ex.: chave inválida);
- atividade com ferramentas obrigatórias e conexão com ferramentas
  desligadas;
- atividade com ferramentas obrigatórias e modelo marcado como "não chama
  ferramentas";
- contexto conhecido menor que o mínimo pedido.

**Critérios**, cada um de 0 a 1:

| Critério | Como |
| -------- | ---- |
| etiquetas | 1 etiqueta da atividade = 0,7; 2 ou mais = 1; cada etiqueta citada na tarefa (ex.: "python") soma 0,5. Sem etiquetas = 0,25; etiquetas sem relação = 0,1 |
| custo | preço combinado (3 entrada : 1 saída) em escala log entre os candidatos: grátis = 1, o mais barato pago = 0,9, o mais caro = 0. Preço desconhecido = 0,5 |
| qualidade | etiquetas como `raciocínio`, `premium`, `avançado`; dicas no id (`opus`, `pro`, `large`, `ultra`, `max`, `thinking`, ≥ 65b); um pouco pelo preço |
| velocidade | etiquetas como `rápido`, `leve`; dicas no id (`mini`, `nano`, `flash`, `haiku`, `lite`, `small`, `turbo`, ≤ 14b) |
| contexto | 4k = 0 … 128k ou mais = 1; desconhecido = 0,5 |
| ferramentas | declara suporte = 1; não informado = 0,6 (só pesa quando são obrigatórias) |

**Pesos por preferência** (etiquetas, qualidade, custo, velocidade,
contexto, ferramentas):

| Preferência | Pesos |
| ----------- | ----- |
| qualidade | 0,30 · 0,35 · 0,05 · 0 · 0,15 · 0,15 |
| equilíbrio | 0,35 · 0,15 · 0,20 · 0,10 · 0,10 · 0,10 |
| custo | 0,20 · 0,05 · 0,60 · 0,05 · 0,05 · 0,05 |
| velocidade | 0,25 · 0,05 · 0,15 · 0,40 · 0,05 · 0,10 |

Quando as ferramentas são opcionais, o peso delas passa para as etiquetas.
A nota final é `100 × Σ peso × critério`. Há dois desempates: +1 para o
modelo padrão da conexão e +0,5 para o provider ativo.

É uma **heurística**, e a UI diz isso. Para melhorar a escolha, preencha
etiquetas, preços, contexto e suporte a ferramentas na conexão. O
julgamento fino fica com o Conselho.

## Conselho: deliberação

1. O roteador ordena e a **lista curta** recebe os K melhores (padrão 6, de
   2 a 10).
2. Cada membro recebe, via `AIProvider::complete`:
   - instruções fixas (`council::SYSTEM`);
   - a tarefa (até 4.000 caracteres), a atividade e os requisitos;
   - os candidatos com ids curtos: `c1 · Nuvem A / gpt-medio · US$ 2.5
     entrada e 10 saída por M tokens · contexto 128k · ferramentas: sim ·
     etiquetas: código · nota do roteador 69.9`.

   Não recebe arquivos, chaves nem ferramentas.
3. Resposta esperada, só JSON:
   `{"choice": "c2", "ranking": ["c2", "c1"], "confidence": 0.8, "reason": "…"}`.
   A leitura tolera cercas de código e texto em volta, `"C2"`, `"2"` e `2`,
   e confiança em porcentagem (80 → 0,8). Id fora da lista, JSON inválido,
   erro da API ou prazo estourado (padrão 60 s) contam como **abstenção**,
   com o motivo.
4. Os membros respondem **em paralelo**; um membro lento não segura os
   outros.
5. **Agregação:**
   - contagem de Borda: a posição `p` do ranking de um membro vale `K − p`
     pontos, multiplicados pela confiança (de 0,2 a 1; sem confiança,
     0,7);
   - empate: vence a nota do roteador;
   - concordância: fração dos votos válidos que escolheram o vencedor.
6. **Sem votos válidos:** vale a primeira do roteador, com aviso, e o Full
   não aplica.

Uso e custo de cada membro saem dos preços do modelo do membro. O total
aparece na decisão e no histórico.

## Cache

Uma pergunta igual não gasta tokens de novo.

- **Chave:** as palavras da tarefa (sem diferenciar maiúsculas, espaços ou
  acentos), a atividade, a preferência, os requisitos, a lista curta (ids,
  preços, contexto, ferramentas e etiquetas) e os membros. Mudar o preço ou
  as etiquetas de um candidato, ou os membros, gera outra chave.
- **Validade:** padrão de 60 minutos, 0 desliga. Guarda até 100 entradas
  em memória. Desde a Fase 6, cada deliberação que pode ser reaproveitada
  também vai para o banco (tabela `deliberations`), com a chave e a
  validade, e o cache vale entre execuções do app (ADR-0012).
- **Acerto:** vira uma deliberação nova (`cached: true`, `cachedFrom`, uso
  zero e `savedUsage` com o que foi economizado) e também é registrado.
- **Limpeza:** "Deliberar de novo" ignora o cache, e salvar a configuração o
  limpa.

## Configuração (`<app-data>/council.json`)

```json
{
  "version": 1,
  "council": {
    "mode": "suggest",
    "members": [
      { "provider": "anthropic", "model": "claude-opus-4-7" },
      { "provider": "openai", "model": null }
    ],
    "shortlist": 6,
    "cacheMinutes": 60,
    "timeoutSecs": 60,
    "preference": null,
    "sendTask": true
  }
}
```

- **Limites:** até 5 membros, sem repetição; lista curta de 2 a 10; cache
  de 0 a 1.440 minutos; prazo de 5 a 300 s.
- **Modelo de membro `null`:** usa o modelo padrão da conexão.
- **Membro de provider removido:** continua na lista, marcado como
  indisponível, e abstém-se.
- **Arquivo inválido:** o Conselho fica desligado e o motivo aparece na
  tela de configuração.

## Histórico

| Evento | Quando | `data` |
| ------ | ------ | ------ |
| `COUNCIL_CONFIGURED` | configuração salva | modo, membros, opções |
| `COUNCIL_DELIBERATED` | o Conselho deliberou (ou o cache respondeu) | `deliberationId`, tarefa (até 500 caracteres), atividade, preferência, candidatos com nota, votos, decisão, uso, `cached`, `autoApply` |
| `ROUTE_DECIDED` | uma sessão abriu com um modelo escolhido | `deliberationId`, `sessionId`, provider, modelo, `by` (`user`/`council`), `followedRecommendation`, `recommended`, atividade |

- **Recomendações sem tokens:** as do modo Desligado, ou com um único
  candidato, não geram `COUNCIL_DELIBERATED`, só `ROUTE_DECIDED` quando a
  sessão abre.
- **Origem `council`:** no Full, a sessão, o turno e o `ROUTE_DECIDED`
  registram `{"type": "council", "deliberationId": …}`.
- **Deliberações guardadas (Fase 6):** o `RouterService` recebe um
  `DeliberationStore` (`with_store`). O app usa o banco local: cada
  deliberação é gravada inteira, as 50 últimas são carregadas ao iniciar, e
  salvar a configuração limpa também o cache guardado.

## `AIProvider::complete`

Resposta avulsa: uma chamada com instruções, texto e modelo. Não abre
sessão, não guarda conversa e **não oferece ferramentas**, de modo que o
membro não tem como agir no sistema.

- **Implementação:** o padrão é `UNSUPPORTED`. As conexões de API
  implementam com a mesma chamada HTTP dos turnos, e o `echo` devolve o
  texto.
- **Capacidade:** `completion` indica suporte. Só providers com ela
  aparecem como membros possíveis.
- **Conexões com ferramentas desligadas** (`toolMode: none`) funcionam como
  membros normalmente.

## IPC

| Comando | Faz |
| ------- | --- |
| `router_recommend(request)` | só o ranking do roteador (sem tokens) |
| `council_get()` | configuração, perfis das atividades, máximo de membros, aviso de carga |
| `council_save(settings)` | valida, grava e registra `COUNCIL_CONFIGURED` |
| `council_run(request)` | delibera; no Full, abre a sessão e envia a tarefa → `{deliberation, started}` |
| `council_history()` | últimas 50 deliberações (mais recentes primeiro), inclusive de execuções anteriores |
| `route_start_session(request)` | abre a sessão com o modelo aprovado/escolhido (`ROUTE_DECIDED`) e, se pedido, envia a tarefa |

`request` de `council_run` e `router_recommend`:
`{ task, activity?, preference?, needsTools?, minContext?, force? }`. O de
`route_start_session`:
`{ deliberationId?, provider, model?, title?, task?, sendTask? }`. Sem
título, a primeira linha da tarefa vira o título da sessão.

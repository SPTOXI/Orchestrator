# Roteador de modelos e Conselho de IAs

Referência de `packages/router` (crate `orchestrator-router`). Decisões
registradas em [ADR-0011](./adr/0011-roteador-de-modelos-e-conselho.md)
(roteador, Fase 5) e [ADR-0024](./adr/0024-conselho-que-analisa-junto.md)
(o Conselho analisa junto e executa com reservas).

Com várias APIs cadastradas ([api-connections.md](./api-connections.md)),
há dois níveis:

- **Roteador:** ordena todos os modelos por regras, sem gastar tokens, e
  explica cada nota. O usuário escolhe um à mão.
- **Conselho:** de 1 a 5 membros (provider + modelo) **analisam a demanda
  juntos**, com o contexto do projeto. Um deles junta as análises num plano,
  e o 1º membro disponível o executa, com os outros de **reserva**. Modelos
  de fora do Conselho nunca são usados por ele.

```text
demanda ──▶ CONSELHO (modos Sugerir / Full)          cache (mesma demanda,
            │                                         projeto e membros = 0 tokens)
            ├─ cada membro analisa, em paralelo: AIProvider::complete
            │  (contexto do projeto, sem sessão, sem ferramentas)
            ├─ síntese: o 1º que respondeu junta tudo no Plano do Conselho
            │  (falhou? o próximo; ninguém? as análises lado a lado)
            └─ fila: ordem dos membros; quem falhou na análise ou está
               indisponível vai para o fim
            │
            ▼
   Sugerir: o usuário vê o plano e clica      Full: executa sozinho
   "Executar com <membro>" (council_execute)  (origem council)
            │
            ▼
   sessão com o 1º da fila, reservas = os outros, 1ª mensagem = demanda + plano
   turno falhou (sobrecarga, queda, erro)? ──▶ a próxima reserva assume a
   sessão no mesmo turno, com um resumo do que já foi feito (SESSION_FAILOVER)
```

## Na interface

- **AI PROVIDERS → Conselho → Configurar:**
  - modo;
  - membros em ordem: as setas ↑ ↓ definem quem executa (1º) e a ordem das
    reservas;
  - cache e prazo por membro;
  - preferência do roteador;
  - enviar a tarefa como 1ª mensagem, para a escolha à mão;
  - deliberações recentes.
- **AI PROVIDERS → Nova sessão com o Conselho:** descreva a demanda.
  Ctrl+Enter envia ao Conselho. O resultado mostra:
  - o **Plano do Conselho**: quem o escreveu, avisos, uso, custo e tempo;
  - **Quem executa**: a fila, com o papel de cada membro (executa, 1ª
    reserva…) e por que algum foi para o fim;
  - **Executar com <membro>** e **Analisar de novo**;
  - as **análises dos membros**, uma por membro, com o erro de quem não
    respondeu.
- **Só o roteador (grátis):** mostra o ranking de **todos** os modelos
  cadastrados, com a nota, os motivos, os excluídos e "Usar este" para
  iniciar à mão. No modo Desligado, é o próprio resultado, com atividade,
  preferência, ferramentas e contexto mínimo.
- **Sessão:** quando a reserva assume, a conversa mostra "X falhou: motivo.
  A reserva Y assumiu a sessão".
- **HISTORY:** filtros `COUNCIL_CONFIGURED`, `COUNCIL_DELIBERATED`,
  `ROUTE_DECIDED` e `SESSION_FAILOVER`. A origem "Conselho (Full)" marca o
  que o Conselho executou sozinho.

## Modos

| Modo | Tokens | O que acontece |
| ---- | ------ | -------------- |
| **Desligado** (`off`) | nenhum | o roteador ordena e recomenda; o usuário inicia com o recomendado ou outro |
| **Sugerir** (`suggest`) | os da análise | os membros analisam e um escreve o plano; o usuário clica em "Executar com <membro>" |
| **Full** (`full`) | os da análise | depois da análise, o 1º da fila abre a sessão sozinho com a demanda e o plano (origem `council`) |

Regras:

- Sugerir e Full exigem pelo menos um membro.
- Executar **não** dá permissão para operações. As ferramentas que a sessão
  pedir passam pelo gate de autonomia (Assistido / Autônomo / Acesso
  Irrestrito, Fase 9, [autonomy.md](./autonomy.md)).
- A sessão recebe o contexto do projeto no primeiro turno, como qualquer
  sessão (Fase 7, [context.md](./context.md)).
- Sem nenhuma análise, a demanda segue sozinha para quem executa, com
  aviso. O Full executa mesmo assim.
- O handoff (Fase 7) e a task (Fase 8a, "Sugerir com o roteador") usam só o
  roteador, sem gastar tokens ([tasks.md](./tasks.md)).

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
etiquetas, preços, contexto e suporte a ferramentas na conexão.

## Conselho: análise em conjunto

1. **Análise.** Cada membro recebe, via `AIProvider::complete`:
   - instruções fixas (`council::ANALYSIS_SYSTEM`): até 350 palavras com
     **Entendimento**, **Abordagem**, **Riscos e dúvidas** e **O que
     verificar no código**;
   - a demanda (até 8.000 caracteres);
   - o **contexto do projeto**, montado pelo Context Builder como no
     primeiro turno de uma sessão (perfil, regras, memória, decisões e
     tasks), sem apontar ferramentas.

   Não recebe chaves nem ferramentas. Os membros respondem **em paralelo**,
   cada um com o prazo do Conselho (padrão 60 s). Um membro lento não segura
   os outros. Erro da API, prazo estourado, provider indisponível ou
   resposta vazia viram "sem análise", com o motivo.
2. **Síntese.** Com duas ou mais análises, o 1º membro que respondeu (na
   ordem do Conselho) recebe a demanda e todas as análises
   (`council::SYNTHESIS_SYSTEM`) e escreve o **Plano do Conselho**:
   **Consenso**, **Divergências** com a decisão, **Plano** em passos e
   **Cuidados**. Se ele falhar, o próximo que respondeu tenta. Se ninguém
   conseguir, o plano é as análises lado a lado. Com uma análise só, ela é o
   plano.
3. **Fila.** A ordem dos membros define quem executa:
   - o 1º disponível executa;
   - quem falhou na análise ou tem o provider indisponível vai para o fim
     (continua como reserva);
   - membros com a conexão removida ficam de fora.
4. **Execução** (`RouterService::execute`):
   - a sessão abre com o 1º da fila e `reserves` = os outros;
   - se ele não consegue abrir a sessão, o próximo abre, e a resposta lista
     os que pularam (`skipped`);
   - a 1ª mensagem é a demanda, o plano e o papel de quem executa: seguir o
     plano, confirmar no código o que ele supõe e avisar antes de seguir
     outro caminho.

Uso e custo de cada análise e da síntese saem dos preços do modelo do
membro. O total aparece no resultado, no histórico e em Tokens e custo
(como `conselho`).

## Reserva automática

Uma sessão aberta com `reserves` (as do Conselho) não para quando a IA
falha. Quando um turno falha por qualquer motivo que não seja o
cancelamento, o `SessionManager`:

1. abre a sessão da próxima reserva ainda não tentada neste turno;
2. torna essa IA a da sessão, daqui em diante. Quem falhou vai para o fim
   das reservas;
3. reenvia o pedido com o contexto do projeto, um resumo da conversa (as
   trocas mais recentes, até 12 mil caracteres) e o que a tentativa que
   falhou já fez neste pedido (ferramentas chamadas, com o resultado, e o
   texto), pedindo para conferir o estado antes de refazer algo;
4. registra `FailedOver` na conversa e `SESSION_FAILOVER` no histórico.

Se todas as reservas falharem, o turno falha com o motivo de cada uma. As
reservas ficam guardadas com a sessão (coluna `spec`) e valem depois de
reiniciar o app.

## Cache

Uma pergunta igual não gasta tokens de novo.

- **Chave:** as palavras da demanda (sem diferenciar maiúsculas, espaços ou
  acentos), o projeto e os membros, na ordem. Outro projeto, outros membros
  ou outra ordem geram outra chave.
- **O que vale:** as análises e o plano. A fila é refeita na hora, com a
  disponibilidade atual.
- **Validade:** padrão de 60 minutos, 0 desliga. Guarda até 100 entradas
  em memória. Desde a Fase 6, cada deliberação que pode ser reaproveitada
  também vai para o banco (tabela `deliberations`), com a chave e a
  validade, e o cache vale entre execuções do app (ADR-0012).
- **Acerto:** vira uma deliberação nova (`cached: true`, `cachedFrom`, uso
  zero e `savedUsage` com o que foi economizado) e também é registrado.
- **Limpeza:** "Analisar de novo" ignora o cache, e salvar a configuração o
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

- **Ordem dos membros:** o 1º disponível executa, e os outros são as
  reservas, nessa ordem.
- **Limites:** até 5 membros, sem repetição; cache de 0 a 1.440 minutos;
  prazo de 5 a 300 s, valendo para cada análise e para a síntese.
- **`shortlist`:** não é mais usado (ADR-0024). Fica no arquivo por
  compatibilidade e é validado de 2 a 10.
- **`preference` e `sendTask`:** valem para o roteador e a escolha à mão.
- **Modelo de membro `null`:** usa o modelo padrão da conexão.
- **Membro de provider removido:** continua na lista, marcado como
  indisponível, fica sem análise e fora da fila.
- **Arquivo inválido:** o Conselho fica desligado e o motivo aparece na
  tela de configuração.

## Histórico

| Evento | Quando | `data` |
| ------ | ------ | ------ |
| `COUNCIL_CONFIGURED` | configuração salva | modo, membros, opções |
| `COUNCIL_DELIBERATED` | o Conselho analisou (ou o cache respondeu) | `deliberationId`, tarefa (até 500 caracteres), `projectPath`, análises (membro, modelo, erro, tamanho, uso, tempo; nunca o texto), plano (origem, autor, uso, falhas), fila (`seats`), decisão, uso, `cached`, `autoApply` |
| `ROUTE_DECIDED` | uma sessão abriu com um modelo | `deliberationId`, `sessionId`, provider, modelo, `by` (`user`/`council`), `followedRecommendation`, `recommended`, atividade, `reserves` |
| `SESSION_FAILOVER` | a reserva assumiu uma sessão cuja IA falhou | `sessionId`, `turnId`, `from` e `to` (provider e modelo), motivo, falhas do turno |

- **Recomendações sem tokens:** as do modo Desligado não geram
  `COUNCIL_DELIBERATED`, só `ROUTE_DECIDED` quando a sessão abre.
- **Deliberações antigas** (até a 0.1.0, com votos) continuam no histórico
  e abrem com a tabela de votos.
- **Origem `council`:** no Full, a sessão, o turno e o `ROUTE_DECIDED`
  registram `{"type": "council", "deliberationId": …}`.
- **Deliberações guardadas (Fase 6):** o `RouterService` recebe um
  `DeliberationStore` (`with_store`). O app usa o banco local: cada
  deliberação é gravada inteira, as 50 últimas são carregadas ao iniciar, e
  salvar a configuração limpa também o cache guardado.

## `AIProvider::complete`

Resposta avulsa: uma chamada com instruções, texto e modelo. Não abre
sessão, não guarda conversa e **não oferece ferramentas**, de modo que o
membro não tem como agir no sistema. O Conselho a usa para as análises e a
síntese.

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
| `council_run(request)` | Conselho ligado: análise, plano e fila; no Full, também executa → `{deliberation, started, startError}`. Desligado: a recomendação do roteador |
| `council_execute(deliberationId)` | executa um plano aprovado: o 1º da fila abre a sessão, os outros são reservas → `{session, turnId, sendError, skipped}` |
| `council_history()` | últimas 50 deliberações (mais recentes primeiro), inclusive de execuções anteriores |
| `route_start_session(request)` | abre a sessão com um modelo escolhido à mão (`ROUTE_DECIDED`) e, se pedido, envia a tarefa |

`request` de `council_run` e `router_recommend`:
`{ task, activity?, preference?, needsTools?, minContext?, force? }`. O de
`route_start_session`:
`{ deliberationId?, provider, model?, title?, task?, sendTask? }`. Sem
título, a primeira linha da tarefa vira o título da sessão.

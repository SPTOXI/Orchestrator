# ADR-0024 — Conselho que analisa junto, com reserva entre os membros

- **Estado:** Aceita
- **Fase:** depois da 12 (pedido do usuário)
- **Substitui:** a votação de modelos do Conselho (ADR-0011, itens 5, 7 e 11)

## Contexto

O usuário relatou: "retirei o OpenRouter do Conselho e deixei Gemini e
Claude, e ambas, quando solicitada a execução, acionam o OpenRouter. Não
quero o OpenRouter porque toda hora diz que está sobrecarregado. O Conselho
é para analisar em conjunto e não definir qual fará a leitura, e se um tiver
problemas a demanda deve ser direcionada a outro membro do Conselho".

No Conselho da ADR-0011, os membros não trabalhavam na demanda:

- votavam em qual modelo deveria executá-la;
- os candidatos eram **todos** os modelos cadastrados, e não só os membros.
  O roteador escolhia os 6 melhores por regras, e uma conexão com muitos
  modelos baratos ou gratuitos, como o OpenRouter, ocupava a lista;
- o vencedor abria a sessão sozinho. Se o servidor dele estava
  sobrecarregado, o turno falhava e nada passava a demanda adiante.

Tirar o OpenRouter dos membros não mudava nada: ele continuava candidato.

## Decisão

### Só os membros trabalham

Nos modos Sugerir e Full, o Conselho não escolhe mais modelos. Ele só usa os
próprios membros. Modelos de fora do Conselho nunca são consultados nem
executam a demanda.

O roteador por regras continua:

- no modo Desligado;
- no botão "Só o roteador (grátis)", que mostra o ranking de todos os
  modelos para o usuário escolher um **à mão**;
- na recomendação de modelo de tasks e handoffs.

### Análise em conjunto

1. **Cada membro analisa a demanda**, em paralelo e dentro do prazo do
   Conselho. Recebe o mesmo contexto do projeto que o primeiro turno de uma
   sessão recebe: perfil, regras, memória, decisões e tasks, montados pelo
   Context Builder sem ferramentas. Responde em até 350 palavras com
   **Entendimento**, **Abordagem**, **Riscos e dúvidas** e **O que verificar
   no código**.
2. **Síntese:**
   - com duas ou mais análises, o primeiro membro que respondeu, na ordem do
     Conselho, junta todas num **Plano do Conselho**: consenso, divergências
     com a decisão, passos e cuidados;
   - se ele falhar, o próximo que respondeu escreve a síntese;
   - se ninguém conseguir, o plano é as análises lado a lado;
   - com uma só análise, ela é o plano.
3. **Quem executa:** a ordem dos membros é a fila.
   - O 1º disponível executa.
   - Vão para o fim da fila, como reservas, os membros que falharam na
     análise ou cujo provider está indisponível.
   - Membros cuja conexão foi removida ficam de fora.
   - A ordem é do usuário: setas na tela do Conselho. O Conselho não
     escolhe.
4. **Modos:**
   - **Sugerir:** o usuário vê as análises, o plano e a fila, e clica em
     "Executar com <membro>" (comando `council_execute`).
   - **Full:** executa sozinho logo depois da análise.

   Em ambos, a sessão abre com o 1º da fila. A primeira mensagem é a
   demanda, o Plano do Conselho e o papel de quem executa: seguir o plano,
   confirmar no código o que ele supõe e avisar antes de seguir outro
   caminho. Se o 1º não consegue abrir a sessão (CLI ausente, login
   vencido), o próximo abre.
5. **Nenhuma análise:** a demanda segue sozinha para quem executa, com um
   aviso.
6. **Cache:** a mesma demanda, no mesmo projeto e com os mesmos membros na
   mesma ordem, reaproveita as análises e o plano até a validade do cache.
   "Analisar de novo" ignora o cache.

### Reserva automática nas sessões

`StartRequest.reserves` é a lista de quem assume a sessão, em ordem. As
sessões do Conselho levam os outros membros. As demais sessões continuam
sem reservas.

Quando um turno falha por qualquer motivo que não seja o cancelamento (por
exemplo, sobrecarga, servidor fora do ar, chave inválida ou crédito
esgotado), o `SessionManager` faz a troca no mesmo turno:

1. abre a sessão da próxima reserva ainda não tentada neste turno;
2. a partir daí, a sessão é dessa IA. Quem falhou vai para o fim das
   reservas e pode voltar se a nova também falhar;
3. reenvia o pedido com:
   - o contexto do projeto de novo, porque a reserva começa sem nada;
   - um resumo da conversa até ali (as trocas mais recentes, até 12 mil
     caracteres);
   - o que a tentativa que falhou já tinha feito neste pedido: ferramentas
     chamadas, com o resultado, e o texto escrito;
   - o aviso para conferir o estado antes de refazer algo;
4. registra a troca:
   - `FailedOver` na conversa, que a tela mostra como "X falhou: motivo. A
     reserva Y assumiu a sessão";
   - `SESSION_FAILOVER` no histórico.

Se todas as reservas falharem, o turno falha com o motivo de cada uma. As
reservas ficam guardadas com a sessão e valem depois de reiniciar o app.

### Privacidade

A ADR-0011 dizia que os membros recebiam só a tarefa e os dados dos
modelos. Agora eles recebem também o contexto do projeto, o mesmo que
qualquer sessão recebe. Continuam sem ferramentas e sem chaves.

## Consequências

- Uma conexão fora do Conselho, como o OpenRouter, nunca mais é usada por
  ele. Para usá-la, o usuário a põe entre os membros ou a escolhe à mão
  pelo roteador.
- Cada demanda custa uma análise por membro e uma síntese, antes da
  execução. O custo aparece na deliberação e em Tokens e custo, como
  "conselho". O cache evita pagar duas vezes a mesma análise.
- Sobrecarga, queda ou erro do membro que executa não param o trabalho: a
  sessão segue com o próximo, no mesmo turno. A troca aparece na conversa
  e no histórico.
- A reserva recebe um resumo, não a conversa nativa de quem falhou. Por
  isso ela é instruída a conferir arquivos e comandos antes de refazer algo.
- `council.json` não muda de formato. O campo `shortlist` deixa de ser
  usado e é mantido para compatibilidade.
- Deliberações antigas, com votos, continuam no histórico e abrem com a
  tabela de votos.
- `COUNCIL_DELIBERATED` passa a registrar as análises (sem o texto), o
  plano e a fila. `ROUTE_DECIDED` passa a registrar as reservas.

### Fica para depois

- Reservas para tasks e agentes: hoje só as sessões do Conselho têm.
- Uma rodada de réplica, em que cada membro lê as análises dos outros antes
  da síntese.

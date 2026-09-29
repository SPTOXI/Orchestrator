# ADR-0011 — Roteador de modelos e Conselho de IAs

- **Estado:** Aceita
- **Fase:** 5

## Contexto

A Fase 4 (ADR-0010) deixou o usuário cadastrar quantas APIs quiser. Cada
modelo cadastrado tem contexto, preços, suporte a ferramentas, etiquetas
livres e disponibilidade. Com vários modelos, escolher o certo para cada
atividade vira trabalho manual.

O usuário pediu:

- um **Conselho** de IAs (ou uma única IA "gerenciadora") que recomende o
  melhor modelo para cada atividade;
- o modo **Sugerir**, em que o usuário aprova;
- o modo **Full**, em que o conselho decide e aplica sozinho, ligado aos
  modos Autônomo e Acesso Irrestrito (Fase 9);
- deliberações registradas no histórico, com cache para economizar tokens.

Faltam três peças: um jeito de pedir uma resposta curta a um modelo sem
abrir uma sessão, uma pontuação que não gaste tokens e a deliberação.

## Decisão

1. **Crate novo, `packages/router` (`orchestrator-router`).**
   - Módulos: catálogo, atividades, pontuação, Conselho, cache, configuração
     e registro de deliberações.
   - Depende só de `orchestrator-core` e `orchestrator-providers`: funciona
     com qualquer `AIProvider`, não só com as conexões de API.
   - `packages/orchestrator` continua reservado ao motor da Fase 7.

2. **`AIProvider::complete`: resposta avulsa, sem ferramentas.**
   - Uma chamada única com instruções, texto e modelo. Não abre sessão, não
     guarda conversa e **não recebe ferramentas**: quem só completa texto
     não tem como agir no sistema.
   - Implementação padrão: `UNSUPPORTED`. Nova capacidade `completion`.
   - O provider de API implementa com a mesma chamada HTTP dos turnos, e o
     custo sai dos preços do modelo. O `echo` devolve o texto (desenvolvimento).
   - Só providers com `completion` podem ser membros do Conselho.

3. **Atividades.**
   - Tipos: `code` (implementar), `debug` (depurar), `review` (revisar),
     `tests` (testes), `planning` (planejar/arquitetura), `docs`
     (documentação/escrita), `summary` (resumo/pergunta rápida) e `general`.
   - Cada tipo tem um perfil: etiquetas afins (em português e inglês), se
     precisa de ferramentas e a preferência padrão.
   - A atividade é detectada por palavras-chave na descrição da tarefa, sem
     gastar tokens. O usuário pode trocar.

4. **Roteador: pontuação sem tokens.**
   - **Filtros**, com motivo para cada modelo excluído:
     - modelo desativado;
     - provider indisponível no último `inspect`, guardado por 5 minutos;
     - atividade que precisa de ferramentas e modelo ou provider sem elas;
     - contexto conhecido menor que o mínimo pedido.
   - **Critérios**, de 0 a 1:
     - afinidade de etiquetas com a atividade e com a tarefa;
     - custo (preço combinado 3:1 entrada/saída, em escala log entre os
       candidatos);
     - contexto;
     - ferramentas;
     - qualidade e velocidade estimadas pelas etiquetas (`raciocínio`,
       `rápido`…) e por dicas no nome do modelo (`pro`, `opus`, `mini`,
       `flash`, `haiku`…).
   - **Preferência:** `quality`, `balanced`, `cost` ou `speed`. Cada uma dá
     um peso a cada critério.
   - **Saída:** o ranking com a nota e os motivos de cada modelo em
     português, e os excluídos com o motivo.
   - A pontuação é uma heurística e diz isso na UI. O julgamento fica com o
     Conselho.

5. **Conselho: deliberação.**
   - **Membros:** de 1 a 5, cada um um par provider + modelo. Com 1 membro,
     ele é o "gerenciador".
   - **Lista curta:** o roteador escolhe os K melhores candidatos (padrão 6).
     Cada membro recebe a tarefa, a atividade, os requisitos e a lista, com
     ids curtos (`c1`…`cK`) em vez de nomes de modelo, para não haver erro
     de digitação.
   - **Resposta:** JSON com `choice`, `ranking`, `confidence` e `reason`, no
     idioma do usuário. A leitura tolera cercas de código e texto em volta.
     Um id fora da lista vira abstenção com o erro.
   - **Execução:** os membros respondem em paralelo, cada um com prazo
     (padrão 60 s).
   - **Agregação:** contagem de Borda ponderada pela confiança (0,2 a 1). O
     empate é decidido pela nota do roteador. Resultado: a decisão e a
     **concordância** (fração dos membros que escolheram o mesmo modelo).
   - **Falhas:** se todos falharem, a decisão é a primeira do roteador, com
     aviso. Nada trava.
   - **Registro:** uso e custo de cada membro e o total ficam na
     deliberação.

6. **Cache.**
   - Deliberações iguais não gastam tokens de novo. A chave junta a tarefa
     normalizada, a atividade, os requisitos, a lista curta (modelos, preços,
     etiquetas) e os membros.
   - Validade padrão de 60 minutos, até 100 entradas, só em memória até a
     Fase 6.
   - Um acerto também é registrado (`cached: true`, custo zero). "Deliberar
     de novo" ignora o cache.

7. **Modos do Conselho.**
   - `off`: só o roteador, sem tokens. O usuário escolhe.
   - `suggest` (**Sugerir**): o Conselho delibera e mostra decisão, votos e
     motivos. O usuário aprova ou escolhe outro modelo.
   - `full` (**Full**): o Conselho decide e o Orchestrator aplica sozinho.
     Abre a sessão com o modelo escolhido e envia a tarefa como primeira
     mensagem.
   - Nesta fase o Conselho escolhe o modelo de sessões novas. Nas Fases 8–9
     o mesmo serviço escolhe o modelo de tasks e agentes. O Full **não**
     dispensa o gate de autonomia da Fase 9: escolher o modelo não autoriza
     operações.

8. **Configuração.**
   - Modo, membros, K, validade do cache, prazo e preferência padrão ficam
     em `<app-data>/council.json`, sem segredos, até o SQLite.
   - Salvar gera `COUNCIL_CONFIGURED`.
   - Um membro cujo provider foi removido continua na lista, marcado como
     indisponível, e é ignorado nas deliberações.

9. **Histórico.** Três eventos novos:
   - `COUNCIL_DELIBERATED`: tarefa (até 500 caracteres), atividade,
     candidatos com nota, votos, decisão, concordância, uso, custo e se veio
     do cache;
   - `ROUTE_DECIDED`: o modelo aplicado a uma sessão, quem decidiu (`user`
     ou `council`), se seguiu a recomendação, a deliberação e a sessão;
   - `COUNCIL_CONFIGURED`.

   As últimas 50 deliberações também ficam em memória para a UI.

10. **Comandos Tauri.** `router_recommend`, `council_get`, `council_save`,
    `council_deliberate`, `council_history` e `route_start_session`. O
    último registra `ROUTE_DECIDED`, abre a sessão e, se pedido, envia a
    tarefa. Tudo com origem `user`, exceto a aplicação automática do modo
    Full, que usa a origem `council`.

11. **Privacidade.** Os membros recebem só a descrição da tarefa e os dados
    dos modelos (nomes, preços, contexto, etiquetas). Não recebem arquivos
    do projeto, chaves ou ferramentas.

## Consequências

- Escolher o modelo passa a ter três níveis: manual, roteador (grátis e
  instantâneo) e Conselho (julgamento com custo visível e cache).
- O trait `AIProvider` ganha um método com implementação padrão. Os
  adapters existentes continuam compilando, e só quem implementa
  `complete` pode ser membro.
- `CallOrigin` ganha a origem `council`, para distinguir no histórico o que
  o Conselho aplicou sozinho.
- Etiquetas bem preenchidas melhoram o roteador. A UI mostra o motivo de
  cada nota para o usuário ajustar etiquetas e preços.
- O cache e o registro de deliberações se perdem ao fechar o app até a
  Fase 6. O histórico (`audit.jsonl`) guarda tudo.

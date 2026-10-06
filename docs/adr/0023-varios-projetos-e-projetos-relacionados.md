# ADR-0023 — Vários projetos abertos e projetos relacionados

- **Estado:** Aceita
- **Fase:** depois da 12 (pedido do usuário)

## Contexto

O usuário pediu: "permitir a adição de projetos na aba lateral. Hoje abrimos
1 e quando abrimos outro, não vemos mais o primeiro. … e o mais interessante:
se tiverem correlação, podemos realizar interações entre as IAs que os
desenvolvem".

Até aqui o app tinha **um** projeto aberto:

- O painel PROJECT mostrava um projeto. Abrir outro trocava tudo, e o
  primeiro só voltava pelos recentes.
- O runtime tinha uma pasta de trabalho global (`base_dir`), trocada por
  `project.open`. Toda chamada de ferramenta de uma IA (caminho relativo,
  comando sem `cwd`) caía nessa pasta. Uma sessão do projeto A que ainda
  trabalhava quando o usuário abria o projeto B passava a ler, escrever e
  executar **em B**. A autonomia julgava o "dentro do projeto" com a mesma
  pasta global.
- As abas (arquivos, sessões, memória, board…) eram uma lista só, sem dono.
- Projetos não sabiam uns dos outros. Uma API e o app que a consome eram,
  para as IAs, mundos separados.

## Decisão

### Cada chamada trabalha no projeto da sua sessão

`ToolCall` ganha `workspace` (opcional). `TurnContext::call_tool` preenche
com a pasta do projeto da sessão. Isso vale para todas as IAs: API,
assinaturas por CLI (pelo MCP local), agentes e subagentes.

O runtime resolve caminhos relativos e o diretório de comandos, terminais e
processos pelo `workspace` da chamada. Só sem ele, nas chamadas da tela,
usa o projeto aberto. A autonomia (`context_of`) julga "dentro do projeto"
e a pasta de trabalho pelo mesmo `workspace`.

Uma sessão de A continua em A, seja qual for o projeto que a tela mostra.

### Projetos abertos lado a lado

- **Migração 5 do banco:**
  - `projects.open_rank` é a posição do projeto na barra lateral, ou `NULL`
    se ele está fechado;
  - `project_links` guarda os pares relacionados, com a nota do usuário.

  A migração passa pelo backup de antes de migrar (ADR-0022).
- **Abrir:** `PROJECT_OPENED` põe o projeto no fim da lista, se ainda não
  está nela. "Abrir pasta…", um recente, "Procurar" e uma IA que chama
  `project.open` fazem isso igual.
- **Painel PROJECT → "Abertos":**
  - um clique troca de projeto;
  - cada projeto mostra quantas IAs trabalham nele agora;
  - o × fecha o projeto **só na barra lateral**. As sessões, a memória, o
    histórico e os arquivos ficam, e as IAs que trabalhavam nele continuam.
    Fechar o projeto mostrado abre o próximo da lista.
- **Abas por projeto:**
  - cada aba pertence a um projeto, ou a nenhum (Configurações, APIs,
    Conselho, Procurar, Tokens e custo);
  - trocar de projeto mostra as abas dele, na que estava aberta;
  - abas que um projeto tem uma vez só (memória, board, contexto, perfil,
    novo PR, nova task, rota) têm uma por projeto;
  - arquivos de outro projeto continuam montados, escondidos, para nada sem
    salvar se perder; um projeto com arquivo alterado só fecha depois de
    salvar ou descartar;
  - abrir uma sessão de outro projeto (aviso de "terminou", barra de status,
    "Abrir a conversa") troca para o projeto dela.
- **O projeto mostrado** continua sendo o "atual" do banco (padrão de
  tasks, agentes, memória e autonomia na tela) e a pasta de "Nova sessão",
  terminais e processos da tela.

### Projetos relacionados

O usuário relaciona dois projetos no painel PROJECT ("Relacionados", +) e
diz como eles se relacionam, por exemplo "o app consome a API deste
projeto". A relação vale nos dois sentidos. Desfazer não apaga nada.
`PROJECT_LINKED` registra a mudança no histórico.

A relação é o consentimento: as IAs de um projeto só alcançam, por estas
ferramentas, os projetos relacionados a ele.

| Ferramenta | O que faz | Autonomia |
| ---------- | --------- | --------- |
| `projects.related` | Lista os relacionados: pasta, stack, relação, memória fixada, decisões aceitas, tasks abertas e as sessões que trabalham neles | consulta |
| `projects.ask` | Pergunta à IA de um relacionado e espera a resposta (veja abaixo) | escrita: pergunta no Assistido |
| `projects.request` | Deixa uma task na lista do relacionado, dizendo quem pediu | escrita: pergunta no Assistido |

Os arquivos de um relacionado já se leem pelo caminho absoluto, como
qualquer arquivo da máquina (ADR-0020). As ferramentas dão o que só o
outro projeto sabe: a sua memória, o seu histórico e a IA que trabalha nele.

**`projects.ask`:**

- A pergunta roda como um turno de uma sessão **do outro projeto**, a
  "Conversa com <projeto que pergunta>". O primeiro turno dela recebe o
  contexto, as regras, as skills e a autonomia daquele projeto. Essa sessão
  é reaproveitada nas perguntas seguintes, então o outro lado lembra a
  conversa; `fresh: true` começa outra.
- O provider e o modelo são os da sessão mais recente do outro projeto ("a
  IA que o desenvolve"). Se ele não tem sessão, valem os de quem pergunta.
- O usuário acompanha a conversa na lista de sessões daquele projeto: a
  pergunta aparece como "IA do projeto X".
- Na sessão de quem perguntou, a chamada mostra "Abrir a conversa".
- Se a conversa já está ocupada com outra pergunta, a nova espera a vez.
- Cancelar o turno de quem pergunta cancela também a resposta.
- A resposta volta como saída da ferramenta, e `PROJECT_ASKED` registra a
  pergunta e a resposta no histórico dos dois projetos.
- **Sem laços:** uma sessão que está respondendo a outro projeto não pode
  chamar `projects.ask`. Ela responde com o que sabe, e quem perguntou
  consulta mais depois.

**Contexto:** a seção nova **RELATED PROJECTS** do Context Builder mostra,
no primeiro turno de cada sessão, os relacionados (nome, pasta, stack,
relação) e como alcançá-los. A instrução de sistema das APIs diz o mesmo
quando as ferramentas `projects.*` existem.

## Consequências

- Várias frentes andam ao mesmo tempo: as IAs de A seguem trabalhando em A
  enquanto o usuário olha B, e a barra lateral mostra quem está trabalhando
  onde.
- Uma API e o app que a usa se consultam sem o usuário copiar e colar
  contratos entre conversas. Cada resposta custa um turno da IA do outro
  projeto, contado no custo daquele projeto.
- `project.open` continua mudando a pasta das chamadas da tela. Uma IA que
  o chama troca o projeto que a tela mostra, mas não a pasta das próprias
  chamadas, que segue a sessão.
- O banco passa ao esquema 5. A versão anterior do app não abre um banco
  migrado e usa os backups (ADR-0022).
- Agentes e travas continuam por projeto (ADR-0015). Uma task deixada por
  `projects.request` é uma task comum do outro projeto: alguém a começa,
  ou um agente.

### Fica para depois

- Reordenar os abertos arrastando (o banco e o comando
  `projects_reorder` já aceitam a ordem).
- Uma visão de "conversas entre projetos" que junte as perguntas e
  respostas de todos os relacionados.
- Agentes que, num pedido de `projects.request`, começam sozinhos no outro
  projeto, conforme a autonomia de lá.

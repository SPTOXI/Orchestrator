# ADR-0016 — Autonomia: Assistido, Autônomo, Acesso Irrestrito e Pause

- **Estado:** Aceita
- **Fase:** 9

## Contexto

O documento mestre define, na seção 10, três modos de autonomia:

- **Assistido:** o sistema pede autorização quando necessário;
- **Autônomo:** o sistema executa de acordo com as políticas configuradas
  pelo usuário;
- **Acesso Irrestrito:** *não existem políticas operacionais de restrição
  impostas pelo Orchestrator* — sem confirmações ocultas, sem bloqueios
  silenciosos, sem lista interna de comandos proibidos, sem operações
  transformadas em aprovação obrigatória, e sem reinterpretar o modo como
  "autônomo, porém limitado".

Em todos eles, **observabilidade não é restrição**: logs, histórico,
eventos de auditoria, saída de terminal e chamadas de ferramenta continuam
registrados. A seção 11 acrescenta `Pause`, `Cancel` e `Stop All Agents`
como controles de operação do usuário, que não são políticas impostas ao
agente. A seção 24 pede o `Autonomy Mode` visível o tempo todo, e a seção
30 fecha: **o usuário decide o nível de autonomia**; **Acesso Irrestrito
significa acesso irrestrito**.

O que já existe depois da Fase 8b:

- **um caminho só para as IAs agirem:** `TurnContext::call_tool` →
  executor de ferramentas das sessões (`AgentTools` → `EngineTools` →
  `RuntimeTools` → `ToolRuntime::invoke`). O runtime não tem lista de
  comandos proibidos nem confirmação escondida, e a ADR-0003 reservou
  exatamente esse ponto para o gate;
- **o usuário age por outro caminho** (`runtime_invoke`, origem `user`),
  que nunca passou pelas sessões;
- **agentes** que executam tasks sozinhos, com fila, teto de turnos, teto
  de paralelismo, travas de arquivo, `Parar` e `Parar todos` (ADR-0015);
- **o Conselho em modo Full**, que abre a sessão e envia a tarefa sozinho
  e que "não dispensa o gate da Fase 9" (ADR-0011).

Ou seja: hoje toda IA age como se estivesse em Acesso Irrestrito, sem que
o usuário tenha escolhido isso. Esta ADR decide o gate, os três modos, as
políticas do Autônomo, os pedidos de autorização e o `Pause`.

## Decisão

### 1. Onde o gate fica

O gate é o **executor mais de fora** das sessões:

```text
TurnContext::call_tool
  └─ AutonomyGate        modo, políticas, pedidos de autorização, pausa   (novo)
      └─ AgentTools      ferramentas do agente e travas de arquivo        (ADR-0015)
          └─ EngineTools ferramentas de memória                           (ADR-0013)
              └─ RuntimeTools → ToolRuntime::invoke                        (ADR-0002)
```

- **Só as IAs passam por ele.** O usuário usa o `runtime_invoke` e nunca
  espera autorização de si mesmo.
- Mais de fora porque tudo o que uma IA pede — inclusive `memory.save` e
  `agent.delegate`, que não são do runtime — tem de passar pela decisão do
  usuário; e uma chamada negada não chega a tomar trava de arquivo.
- **Código:** contratos (`AutonomyMode`, regras, pedidos, respostas) no
  `orchestrator-core`; avaliação, configuração, pedidos e pausa no
  `orchestrator-engine` (módulo `autonomy`, como o `ARCHITECTURE.md`
  previa para o `packages/orchestrator`). O gate não conhece o runtime: sabe
  quais ferramentas são consultas pelas definições que o executor de
  dentro oferece (`readOnly`), e o app lhe diz qual é o diretório de
  trabalho do runtime.
- **Esperar tem de poder ser cancelado.** `ToolExecutor` ganha
  `execute_with(call, cancel)`, que por padrão só chama `execute`. O
  `TurnContext` passa o token do turno; o gate, que é o único que espera,
  usa. Cancelar o turno, parar o agente ou `Parar todos` resolvem um
  pedido pendente como cancelado.
- **Nenhuma ferramenta de IA muda o modo.** Modo e políticas mudam só por
  comandos da UI, com origem `user`.

### 2. Os modos e a quem eles valem

| Onde se escolhe | Vale para |
| --------------- | --------- |
| **Modo do projeto** (chip `Autonomia` e aba Autonomia) | toda sessão e todo agente do projeto |
| **Modo padrão** | projetos em que o usuário ainda não escolheu; começa em **Assistido** |
| **Modo do agente** (ao pôr um agente para executar a task) | só aquele agente e os subagentes que ele criar |

O modo de uma chamada é o do agente que conduz a sessão, se o usuário deu
um a ele; senão, o do projeto da sessão; senão, o padrão.

- "Se o usuário conceder Acesso Irrestrito, **o agente autorizado**
  poderá…": o modo por agente é o jeito de conceder a um agente sem
  conceder ao projeto inteiro.
- Subagentes herdam o modo concedido ao pai: a concessão é para aquela
  linha de trabalho, e o pai não escolhe um modo diferente para o filho.
- **Começar em Assistido** é uma mudança de comportamento para quem vinha
  da Fase 8b (onde tudo passava). É a direção certa: quem sobe o nível é o
  usuário, por escolha consciente, e não a atualização do app.

### 3. Assistido

Regras fixas, mostradas na aba Autonomia (não editáveis):

| # | Quando | Decisão |
| - | ------ | ------- |
| 1 | `filesystem.*` num caminho `.env*` | perguntar |
| 2 | qualquer ferramenta fora da pasta do projeto | perguntar |
| 3 | consultas (`readOnly`) | permitir |
| 4 | `agent.finish` | permitir |
| 5 | qualquer outra ação | perguntar |

"Quando necessário" é, portanto: **toda ação**, e **toda leitura de fora do
projeto ou de arquivos de ambiente**, que costumam ter segredos e iriam
para a API do provider. `agent.finish` não pergunta porque só entrega o
resultado: a task vai para revisão, e quem conclui é o usuário
(ADR-0015).

### 4. Autônomo: as políticas do usuário

Uma lista ordenada de regras. **A primeira que casa decide** — como um
firewall, fácil de ler e de explicar ("a regra 6 decidiu").

| Campo | Casa quando | Exemplo |
| ----- | ----------- | ------- |
| `tools` | o nome da ferramenta casa com um dos padrões (exato, `grupo.*` ou `*`) | `git.push`, `filesystem.*` |
| `access` | a ferramenta é consulta (`read`) ou ação (`write`) | `read` |
| `where` | o alvo está dentro (`inside`) ou fora (`outside`) da pasta do projeto | `outside` |
| `command` | o comando casa com o padrão (`*` vale qualquer coisa) | `rm *`, `npm test*` |
| `path` | o caminho casa com o padrão, como no `.gitignore` | `.env*`, `src/**` |
| `decision` | — | `allow`, `ask` ou `deny` |

Campos vazios casam com tudo. Nenhuma regra casou: **perguntar**.

**Alvos.** Uma chamada pode ter vários alvos: os caminhos que recebe
(`path`, `from`, `to`, `cwd`) e as partes do comando. Cada combinação de
caminho e parte de comando é avaliada pela lista, e **vale a decisão mais
restritiva** (`deny` > `ask` > `allow`). Assim, `filesystem.move` de dentro
para fora do projeto é tratado como fora, e `npm test && rm -rf build` não
passa por uma regra `npm test*`.

- **Comando:** vale para `shell.execute` e `process.start` (o comando) e
  para `terminal.write` (o texto digitado). É dividido em partes por `;`,
  `&&`, `||`, `|`, `&` e quebras de linha. Um comando com substituição
  (`$(…)` ou crases) não é analisável: **regras com padrão de comando não
  se aplicam a ele**, e decide a primeira regra sem padrão de comando.
  Regra com padrão de comando nunca casa com ferramenta sem comando.
- **Caminho:** padrão sem `/` casa com o nome do arquivo; com `/`, com o
  caminho relativo à pasta do projeto (ou o absoluto, quando o alvo está
  fora). `*` e `?` não passam de uma pasta para outra; `**` passa. Regra
  com padrão de caminho nunca casa com alvo sem caminho.
- **Dentro ou fora:** caminhos relativos são resolvidos como o runtime
  resolve (diretório de trabalho do runtime, `~` expandido), `..` é
  aplicado, e a parte que já existe é resolvida no disco, então um link
  simbólico dentro do projeto que aponta para fora conta como fora. Uma
  chamada sem caminho tem como alvo o diretório de trabalho do runtime.

Regras padrão do Autônomo (visíveis, editáveis e restauráveis):

| # | Quando | Decisão |
| - | ------ | ------- |
| 1 | `filesystem.*` num caminho `.env*` | perguntar |
| 2 | qualquer ferramenta fora da pasta do projeto | perguntar |
| 3 | consultas | permitir |
| 4 | `filesystem.delete` | perguntar |
| 5 | `git.push`, `git.reset` | perguntar |
| 6 | `package.install` | perguntar |
| 7 | comando `rm *` | perguntar |
| 8 | comando `sudo *` | perguntar |
| 9 | comando `git push*` | perguntar |
| 10 | `terminal.write` | perguntar |
| 11 | qualquer outra | permitir |

`terminal.write` pergunta por padrão porque o texto digitado num terminal
chega em pedaços e não é um comando que se possa analisar inteiro.

A aba Autonomia tem um **"Experimentar"**: o usuário escreve uma ferramenta
e um comando ou caminho e vê qual regra decide e por quê, com o mesmo
código que o gate usa.

### 5. Acesso Irrestrito

O gate **não avalia nada**: a chamada vai direto para o executor de
dentro. Não existe regra, lista, pedido ou exceção nesse modo — nem as
regras fixas do Assistido, nem as do usuário.

O que continua existindo, e por que não é restrição:

- **Auditoria** (`TOOL_CALLED` e os eventos de domínio): observabilidade,
  como a seção 10 manda.
- **Travas de arquivo** (ADR-0015): coordenação entre os agentes do
  próprio usuário, exigida pela seção 13 para todos os modos. Não impede
  nenhuma operação sobre o sistema, a recusa é explícita (`LOCKED`, com o
  nome de quem tem o arquivo) e o usuário nunca é travado.
- **Teto de turnos e de paralelismo** (`agents.json`): ajustes de custo do
  usuário, visíveis, iguais em todos os modos, e que não dizem *o que* um
  agente pode fazer, só *quanto* ele roda.
- **Pausar, Parar e Parar todos:** controles do usuário (seção 11).

Escolher Acesso Irrestrito na UI mostra o que o modo significa e pede um
clique em "Conceder Acesso Irrestrito". Isso é a escolha consciente que o
documento mestre descreve — não uma confirmação por operação.

### 6. Pedidos de autorização

Quando a decisão é **perguntar**, o gate cria um pedido e espera:

| Campo | Conteúdo |
| ----- | -------- |
| `id`, `callId` | o pedido e a chamada |
| `tool`, `summary`, `detail` | o que a IA quer, em palavras ("Executar `npm test`", "Escrever src/app.ts (2,1 KB)") e o detalhe (comando, caminhos, começo do conteúdo) |
| `reason` | por que pergunta: o modo e a regra |
| quem | sessão, agente, task, provider e projeto |
| `requestedAt` | desde quando espera |

Respostas:

- **Permitir** — executa esta chamada.
- **Permitir nesta sessão** — executa e não pergunta de novo, nesta
  sessão, o que **a mesma regra** perguntaria sobre a **mesma ferramenta**
  (e, com comando, o **mesmo comando**). Uma escrita fora do projeto
  continua perguntando mesmo depois de liberar as escritas de dentro.
  Liberações ficam só em memória, aparecem na aba e podem ser revogadas;
  salvar as regras ou mudar o modo as apaga.
- **Negar**, com um motivo opcional — a IA recebe o erro `DENIED` com o
  motivo ("use o pnpm, não o npm"), e pode seguir por outro caminho.

Regras do pedido:

- **Sem prazo.** Esperar o humano é o objetivo do modo; o agente fica
  "esperando autorização", e o usuário pode pará-lo.
- **Cancelado** quando o turno é cancelado, o agente é parado ou
  `Parar todos` é usado — a IA recebe `CANCELLED`, como já acontece com
  uma ferramenta pedida depois do cancelamento (ADR-0009).
- **Mudar o modo ou as regras reavalia os pedidos pendentes:** o que passa
  a ser permitido é executado, o que passa a ser negado é negado, o resto
  continua esperando. Quem troca para Acesso Irrestrito não precisa
  aprovar a fila um por um.
- Uma regra `deny` recusa na hora, com a regra no motivo.

`ToolErrorKind` ganha **`DENIED`**: o usuário, ou uma regra dele, recusou.
A mensagem sempre diz quem e por quê. Não existe recusa sem motivo.

### 7. Pause (seção 11)

- **Pausar IAs** (tudo): toda chamada de ferramenta de qualquer IA fica
  retida no gate até "Retomar"; agentes não começam turno novo; a fila não
  inicia agentes. O modelo que já estava gerando uma resposta termina de
  gerar — o que ele pedir para fazer espera.
- **Pausar um agente** (em execução): o mesmo, só para ele, na próxima
  chamada de ferramenta ou no próximo turno, o que vier antes.
- Um agente pausado **continua com a vaga e com as travas**: retomar não
  pode esbarrar num limite que a pausa liberou para outro.
- "Pausado" não é um estado novo gravado no agente: é o que o gate sabe
  agora, mostrado no painel e no board. Depois de reiniciar o app nada
  roda (ADR-0015), então não há pausa para lembrar.
- `Cancel` e `Stop All Agents` já existem (cancelar turno, `Parar`,
  `Parar todos`) e agora também cancelam o que estava esperando no gate.

### 8. Persistência

`<app-data>/autonomy.json`, como `context.json` e `agents.json`:

```json
{
  "version": 1,
  "defaultMode": "assisted",
  "projects": { "<projectId>": "autonomous" },
  "rules": [ { "tools": ["git.push"], "decision": "ask" } ]
}
```

- Arquivo ilegível ou inválido: Assistido com as regras padrão, e um aviso.
  O erro nunca abre permissões.
- O modo concedido a um agente vai no próprio agente (campo `autonomy`, no
  JSON da linha): **nenhuma migração**, o esquema continua 4.
- Liberações "nesta sessão" e pausas vivem só em memória.
- O arquivo é lido ao abrir o app; mudar pela UI grava na hora.

### 9. Histórico

Cinco tipos de evento novos, pela mesma razão do `TASK_UPDATED`
(ADR-0014): sem eles, quem autorizou o quê sumiria do histórico.

| Evento | Quando |
| ------ | ------ |
| `AUTONOMY_CHANGED` | o modo de um projeto, o modo padrão ou as regras mudaram |
| `APPROVAL_REQUESTED` | uma IA pediu algo que precisa de autorização |
| `APPROVAL_DECIDED` | o pedido foi permitido, permitido na sessão, negado ou cancelado, e em quanto tempo |
| `EXECUTION_PAUSED` / `EXECUTION_RESUMED` | as IAs, ou um agente, foram pausados e retomados |

- Pedido e decisão levam o `callId`: o histórico de uma chamada mostra
  quem pediu, quem autorizou e o `TOOL_CALLED` da execução.
- **Recusa nunca é silenciosa:** uma chamada negada (pelo usuário, por
  regra, ou cancelada enquanto esperava) não chega ao runtime, então o
  próprio gate registra o `TOOL_CALLED` com `ok: false`, o erro e a regra.
- Correção da Fase 8b: o `AgentTools` passa a registrar `TOOL_CALLED` das
  suas ferramentas (`agent.finish`, `agent.delegate`) e das escritas
  recusadas com `LOCKED`, como a ADR-0015 dizia e o código não fazia.

### 10. Interface

- **Chip `Autonomia`** na barra superior (era um traço): o modo do
  projeto aberto, com "pausado" e o número de pedidos; clicar abre a aba.
- **Faixa de autorização** logo abaixo da barra, enquanto houver pedido:
  quem pede, o quê, por quê, e Permitir / Permitir nesta sessão / Negar —
  de qualquer tela.
- **Aba Autonomia:** o modo do projeto (três cartões, com o que cada um
  faz), o modo padrão, Pausar/Retomar IAs, os pedidos com o detalhe, as
  liberações da sessão, as regras do Assistido (fixas) e as do Autônomo
  (editor com ordem, restaurar padrão e "Experimentar").
- **Agentes:** "Pausar"/"Retomar" no painel, no board e na aba da task; o
  modo escolhido ao executar a task ("Modo do projeto" por padrão); e o
  progresso diz "esperando sua autorização" ou "pausado".
- **HISTORY:** filtros para os cinco eventos novos.

## Consequências

**Ganhos**

- O usuário decide o nível de autonomia, por projeto e por agente, e vê o
  modo o tempo todo — o que as seções 10, 24 e 30 pedem.
- Acesso Irrestrito é irrestrito de verdade: o gate nem avalia.
- O Autônomo tem políticas que o usuário lê, edita, testa e entende pela
  regra que decidiu, sem lista escondida.
- Todo pedido, resposta, recusa e pausa fica no histórico, ligado à
  chamada.
- `Pause` existe e é honesto: retém no único caminho por onde as IAs agem.

**Custos e riscos**

- **Padrões de comando e de caminho não são sandbox.** Eles julgam o que a
  ferramenta recebe, não o que o comando faz: um script pode apagar fora
  do projeto, um redirecionamento (`>`) escreve onde quiser, e um link
  criado entre a avaliação e a execução não é visto. O Autônomo é tão forte
  quanto as regras do usuário; a aba diz isso, e as regras padrão
  perguntam pelo que é destrutivo ou remoto.
- **O Assistido pergunta muito.** É o preço do modo; "Permitir nesta
  sessão" e o modo por agente existem para isso.
- Um pedido sem prazo segura o agente (e a vaga) até o usuário responder.
- As políticas são uma lista só, para todos os projetos; o que muda por
  projeto é o modo.
- Começar em Assistido muda o comportamento de quem já usava agentes: eles
  passam a pedir autorização até o usuário escolher outro modo.

**Fica para depois**

- Políticas por projeto e perfis de política prontos.
- Supervisão de processos contra queda do app (Job Objects no Windows,
  supervisão no Unix), que o `tool-runtime.md` previa "junto dos controles
  globais": não é controle do usuário sobre as IAs, é robustez do runtime,
  e fica para a Fase 11.
- Orçamento de custo por agente ou por projeto (Fase 11).

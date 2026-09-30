# Autonomia: Assistido, Autônomo, Acesso Irrestrito e Pause

Referência da Fase 9. Decisão em
[ADR-0016](./adr/0016-autonomia-e-pause.md); contratos no
`orchestrator-core` (`autonomy.rs`) e regras no
[`packages/orchestrator`](../packages/orchestrator/README.md) (módulo
`autonomy`).

> O usuário decide o nível de autonomia. Acesso Irrestrito significa acesso
> irrestrito. Observabilidade não é restrição.

```text
IA pede uma ferramenta ─▶ AutonomyGate ─┬─ Irrestrito ───────────────────────▶ executa
                                        ├─ regra: permitir ──────────────────▶ executa
                                        ├─ regra: negar ─▶ DENIED (registrado)
                                        └─ regra: perguntar ─▶ pedido ─┬─ Permitir ────────▶ executa
                                                                       ├─ Nesta sessão ────▶ executa
                                                                       ├─ Negar (+ motivo) ▶ DENIED
                                                                       └─ turno cancelado ▶ CANCELLED
```

## Onde o gate fica

O `AutonomyGate` é o executor de ferramentas mais de fora das sessões:
`AutonomyGate` → `AgentTools` (travas, ADR-0015) → `EngineTools`
(memória, ADR-0013) → `RuntimeTools` → `ToolRuntime::invoke`.

- Só as chamadas das IAs (origem `agent`) passam por ele. O usuário usa o
  `runtime_invoke` e nunca espera autorização de si mesmo.
- Uma ferramenta que não existe passa direto e falha como `UNKNOWN_TOOL`:
  não há o que autorizar.
- Nenhuma ferramenta de IA muda o modo, as regras ou a pausa.

## Os modos

| Modo | O que acontece |
| ---- | -------------- |
| **Assistido** | consultas rodam; toda ação pede autorização, e também ler fora do projeto ou arquivos `.env*` |
| **Autônomo** | as regras do usuário decidem: permitir, perguntar ou negar |
| **Acesso Irrestrito** | o gate não avalia nada; nenhuma confirmação, nenhuma lista, nenhuma exceção |

A que chamada vale qual modo:

1. o modo que o usuário deu ao **agente** ao pô-lo para executar a task
   (herdado pelos subagentes dele);
2. senão, o modo do **projeto** da sessão;
3. senão, o **modo padrão** (começa em Assistido).

Sem projeto aberto, tudo é "fora do projeto".

### Regras do Assistido (fixas)

| # | Quando | Decisão |
| - | ------ | ------- |
| 1 | `filesystem.*` num caminho `.env*` | perguntar |
| 2 | qualquer ferramenta fora da pasta do projeto | perguntar |
| 3 | consultas | permitir |
| 4 | `agent.finish` | permitir |
| 5 | qualquer outra ação | perguntar |

### Regras do Autônomo (do usuário)

Lista ordenada; **a primeira que casa decide**; se nenhuma casar, pergunta.

| Campo | Casa quando |
| ----- | ----------- |
| `tools` | o nome casa com um dos padrões: exato (`git.push`), grupo (`git.*`) ou `*` |
| `access` | `read` (consultas) ou `write` (ações) |
| `where` | `inside` ou `outside` da pasta do projeto |
| `command` | o comando casa com o padrão; `*` vale qualquer coisa |
| `path` | o caminho casa com o padrão, como no `.gitignore` |
| `decision` | `allow`, `ask` ou `deny` |
| `note` | por que a regra existe (vai no motivo) |

Regras padrão (restauráveis na aba):

| # | Quando | Decisão |
| - | ------ | ------- |
| 1 | `filesystem.*` num caminho `.env*` | perguntar |
| 2 | qualquer ferramenta fora do projeto | perguntar |
| 3 | consultas | permitir |
| 4 | `filesystem.delete` | perguntar |
| 5 | `git.push`, `git.reset` | perguntar |
| 6 | `package.install` | perguntar |
| 7 | comando `rm *` | perguntar |
| 8 | comando `sudo *` | perguntar |
| 9 | comando `git push*` | perguntar |
| 10 | `terminal.write` | perguntar |
| 11 | qualquer outra | permitir |

### Como uma chamada é julgada

Uma chamada tem **alvos**: cada caminho que recebe (`path`, `from`, `to`,
`cwd`, `roots`) — ou o diretório de trabalho do runtime, se não recebe
nenhum — combinado com cada parte do comando. Cada combinação é julgada
pela lista, e **vale a decisão mais restritiva** (`deny` > `ask` > `allow`).

- **Comando** (`shell.execute`, `process.start`: o comando; `terminal.write`:
  o texto digitado; `shell.execute` com `stdin`: o que vai no stdin
  também). Dividido por `;`, `&&`, `||`, `|`, `&` e quebra de linha;
  redirecionamentos como `2>&1` ficam inteiros. Com `$(…)`, crases ou
  `<(…)` o comando é **opaco**: regras com padrão de comando não valem, e
  decide a primeira regra sem padrão de comando.
- **Caminho.** Padrão sem `/` casa com o nome do arquivo; com `/`, com o
  caminho relativo ao projeto (ou o absoluto, fora dele). `*` e `?` não
  passam de uma pasta para outra; `**` passa; `**/` também casa com nada.
  No Windows e no macOS, sem diferença de maiúsculas.
- **Dentro ou fora.** Resolvido como o runtime resolve (`~`, relativo ao
  diretório de trabalho), com `..` aplicado e a parte que existe resolvida
  no disco: um link dentro do projeto que aponta para fora conta como fora.

Exemplos com as regras padrão:

| Chamada | Decisão |
| ------- | ------- |
| `shell.execute` `npm test` | permitir (regra 11) |
| `shell.execute` `npm test && rm -rf dist` | perguntar (regra 7, pela segunda parte) |
| `filesystem.write` `src/app.ts` | permitir (regra 11) |
| `filesystem.write` `../outro/app.ts` | perguntar (regra 2) |
| `filesystem.read` `.env.local` | perguntar (regra 1) |
| `git.commit` | permitir (regra 11) |
| `git.push` | perguntar (regra 5) |

A aba Autonomia tem **Experimentar**, que usa o mesmo código do gate e as
regras do editor antes de salvar.

## Pedidos de autorização

Quando a decisão é perguntar, a chamada espera um pedido com: o que a IA
quer em palavras ("Executar `npm test`", "Escrever src/app.ts (2,1 KB)"), o
detalhe (argumentos, com conteúdo longo cortado), o motivo (modo e regra),
quem pede (agente, task, sessão, provider, projeto) e desde quando.

| Resposta | Efeito |
| -------- | ------ |
| **Permitir** | executa esta chamada |
| **Permitir nesta sessão** | executa, e não pergunta de novo nesta sessão o que a mesma regra perguntaria sobre a mesma ferramenta (e o mesmo comando) |
| **Negar** (+ motivo) | a IA recebe `DENIED` com o motivo |

- Sem prazo: o agente fica "esperando sua autorização".
- Cancelar o turno, parar o agente ou "Parar todos" cancelam o pedido
  (`CANCELLED`).
- Mudar o modo ou salvar as regras reavalia os pedidos: o que passa a ser
  permitido executa, o que passa a ser negado é negado. As liberações "nesta
  sessão" são apagadas.
- Liberações ficam só em memória; a aba lista e permite revogar.

## Pause (seção 11)

| Controle | Efeito |
| -------- | ------ |
| **Pausar IAs** | toda chamada de ferramenta de qualquer IA espera no gate; agentes não começam turno; a fila não inicia agentes |
| **Pausar agente** | o mesmo, só para ele, na próxima chamada ou no próximo turno |
| **Retomar** | tudo segue; a fila anda |
| **Parar / Parar todos** | já existiam; agora também cancelam o que esperava no gate |

Um agente pausado mantém a vaga e as travas. "Pausado" não é um estado
gravado: depois de reiniciar o app nada roda (ADR-0015).

## O que continua valendo em todos os modos

- **Histórico:** `TOOL_CALLED` de toda chamada, inclusive as recusadas pelo
  gate (com `autonomy.mode` e `autonomy.rule`) e as respondidas pelo
  próprio `AgentTools` (`agent.finish`, `agent.delegate`, `LOCKED`).
- **Travas de arquivo** entre agentes (coordenação, seção 13).
- **Teto de turnos e de paralelismo** (`agents.json`): quanto um agente
  roda, não o que ele pode fazer.
- **Pausar e Parar**, que são do usuário.

## Eventos

| Evento | Quando | Dados |
| ------ | ------ | ----- |
| `AUTONOMY_CHANGED` | modo de um projeto, modo padrão ou regras | `scope` (`project`, `default`, `rules`), `mode`, `previous`, `projectId` |
| `APPROVAL_REQUESTED` | uma IA pediu algo que precisa de autorização | `approvalId`, `tool`, `summary`, `reason`, `mode`, `rule`, `sessionId`, `agentId`, `taskId` |
| `APPROVAL_DECIDED` | permitido, permitido na sessão, negado ou cancelado | `approvalId`, `answer`, `by` (`user`, `rules`, `turn`), `note`, `waitedMs` |
| `EXECUTION_PAUSED` / `EXECUTION_RESUMED` | IAs ou um agente pausados / retomados | `scope` (`all`, `agent`), `agentId` |

Pedido, decisão e execução levam o mesmo `callId`.

## Persistência

`<app-data>/autonomy.json`:

```json
{
  "version": 1,
  "defaultMode": "assisted",
  "projects": { "<projectId>": "autonomous" },
  "rules": [{ "tools": ["git.push"], "decision": "ask", "note": "remoto" }]
}
```

Arquivo ilegível ou com regra inválida: Assistido com as regras padrão, e o
aviso aparece na aba. O modo dado a um agente fica no JSON do agente
(`autonomy`); o esquema do banco continua 4.

## UI

- **Chip `Autonomia`** na barra superior: o modo, "pausado" e os pedidos;
  clicar abre a aba.
- **Faixa de autorização** sob a barra, em qualquer tela, com o pedido mais
  antigo e Permitir / Nesta sessão / Negar / Detalhes.
- **Aba Autonomia:** cartões dos modos (Acesso Irrestrito explica o que
  significa antes de conceder), modo padrão, Pausar/Retomar IAs, pedidos
  com detalhe e motivo, liberações da sessão, regras do Assistido, editor
  das regras do Autônomo (ordem, restaurar padrão) e Experimentar.
- **Agentes:** Pausar/Retomar no painel, no board e na aba da task; o modo
  ao executar a task ("modo do projeto" ou um modo só para o agente); o
  progresso diz "esperando sua autorização" ou "pausado".
- **HISTORY:** filtros dos cinco eventos.

## IPC

| Comando | Faz |
| ------- | --- |
| `autonomy_get(projectId?)` | modo, regras, pausa, pedidos, liberações e aviso |
| `autonomy_set_mode(projectId?, mode?)` | modo do projeto (`null` volta ao padrão); sem projeto, o padrão |
| `autonomy_set_default(projectId?, mode)` | modo padrão |
| `autonomy_save_rules(projectId?, rules)` / `autonomy_reset_rules(projectId?)` | regras do Autônomo |
| `autonomy_try(projectId?, mode?, rules?, tool, args?)` | Experimentar |
| `approvals_pending()` | pedidos, do mais antigo ao mais novo |
| `approval_answer(id, answer, note?)` | `approve`, `approveSession` ou `deny` |
| `autonomy_revoke(grantId)` | revoga uma liberação da sessão |
| `execution_pause()` / `execution_resume()` | Pausar / Retomar IAs |
| `agent_pause(id)` / `agent_resume(id)` | Pausar / Retomar um agente |
| `agent_start({…, autonomy?})` | o modo do agente e dos subagentes dele |

## Limitações

- **As regras não são sandbox.** Julgam o que a ferramenta recebe, não o que
  o comando faz depois: um script pode apagar fora do projeto, `>` escreve
  onde quiser, e um link criado entre a avaliação e a execução não é visto.
- Aspas não são interpretadas ao dividir comandos: um separador dentro de
  aspas vira uma parte a mais, o que só pode tornar a decisão mais estrita.
- O Assistido pergunta muito; "Permitir nesta sessão" e o modo por agente
  existem para isso.
- Um pedido sem resposta segura o agente (e a vaga) até o usuário decidir.
- As regras do Autônomo são uma lista só para todos os projetos.
- Liberações e pausas não sobrevivem a um reinício (nada roda depois dele).

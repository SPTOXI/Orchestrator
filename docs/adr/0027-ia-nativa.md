# ADR-0027 — IA Nativa: uma IA local que opera o Orchestrator e o computador

- **Estado:** Proposta
- **Fase:** depois da 12 (pedido do usuário)
- **Depende de:** [ADR-0016](./0016-autonomia-e-pause.md) (autonomia),
  [ADR-0020](./0020-acesso-total-da-ia.md) (acesso total),
  [ADR-0025](./0025-motor-local-proprio.md) (motor local),
  [ADR-0026](./0026-economia-de-tokens-nas-ferramentas-e-nos-pedidos.md)
  (economia de tokens)

## Contexto

O usuário pediu uma **IA própria do Orchestrator**, que:

1. rode em notebooks e computadores com **até 8 GB de RAM**, sem API paga
   nem internet;
2. **gerencie o Orchestrator** (projetos, tasks, agentes, providers,
   autonomia, backups, atualizações);
3. **gerencie o computador** onde ele está instalado, podendo fazer tudo o
   que o usuário pode fazer, sem restrição;
4. tenha uma **lista de funções** a executar (rotinas);
5. **aprenda** o que o usuário faz e o que ele quer que ela faça.

O que já existe e será reaproveitado:

- o motor `llama-server` gerenciado pelo Orchestrator (ADR-0025);
- o Tool Runtime com arquivos, shell, terminais, processos, git, web e
  segredos (ADR-0020), auditado;
- o modo **Acesso Irrestrito** (ADR-0016), que não avalia nada;
- Task Manager, agentes, memória L1/L2/L3 em SQLite (ADRs 0012–0015);
- grupos de ferramentas por sessão e schemas enxutos (ADR-0026).

O problema real é que um modelo que cabe em 8 GB (3–4 bilhões de
parâmetros, quantizado) **erra chamadas de ferramenta** e se perde com 10
mil tokens de definições. A IA Nativa só funciona se o Orchestrator fizer
o trabalho pesado em volta do modelo.

## Decisão

### 1. A "Nativa" é um provider local com perfil fixo

Um provider novo, `native`, sobre o motor da ADR-0025, com o modelo
escolhido pela RAM detectada:

| RAM total | Modelo padrão (GGUF) | Contexto | RAM usada (aprox.) |
| --------- | -------------------- | -------- | ------------------ |
| ≤ 4 GB | 1,5B Q4_K_M | 4.096 | ~1,3 GB |
| 6–8 GB | 3B–4B Q4_K_M | 8.192 | ~2,8–3,5 GB |
| ≥ 12 GB | 7B–8B Q4_K_M | 16.384 | ~5,5 GB |

- O catálogo lista modelos **com suporte a tool calling** (famílias Qwen,
  Llama 3.x, Phi); o usuário pode trocar.
- Cache KV em `q8_0`, `--mlock` desligado, `--threads` = núcleos físicos;
  o motor desliga sozinho depois de N minutos ocioso (configurável) e o
  modelo sai da memória.
- Um segundo modelo pequeno de **embeddings** (~30–130 MB) roda no mesmo
  motor (`--embedding`) para a busca na memória e nas habilidades.
- Teto de memória: se a RAM livre ficar abaixo de 1 GB, a Nativa reduz o
  contexto ou pausa e avisa, em vez de travar o computador.

### 2. Chamadas de ferramenta com gramática

O modelo não escolhe ferramentas em texto livre:

- as chamadas saem pelo `json_schema`/GBNF do `llama-server`, então a
  saída **sempre** é uma chamada válida ou uma resposta;
- **roteamento em dois passos**: primeiro o modelo escolhe um **grupo**
  (`orchestrator`, `system`, `files`, `shell`, `web`, `routine`), depois só
  as ferramentas desse grupo vão ao prompt (≤ ~1.500 tokens);
- argumentos validados antes de executar; em erro, uma nova tentativa com
  a mensagem do erro (até 2).

### 3. Ferramentas novas

**`orchestrator.*`** — expõe à Nativa os comandos do gateway IPC
(ADR-0003) que o usuário já usa na interface: abrir/fechar projetos,
criar/mover tasks, pôr agentes para trabalhar, trocar provider, mudar o
modo de autonomia, fazer backup, buscar atualização. Diferença da
ADR-0016: **só a Nativa** recebe as ferramentas que mudam a autonomia, e
só quando o usuário liga "A Nativa administra o Orchestrator".

**`system.*`** — o computador, por sistema operacional (Windows, macOS,
Linux), com implementação em Rust e fallback para o shell:

| Ferramenta | Faz |
| ---------- | --- |
| `system.info` | CPU, RAM, disco, bateria, rede, SO, uptime |
| `system.processes` / `system.kill` | listar e encerrar processos |
| `system.services` | listar, iniciar, parar e habilitar serviços |
| `system.packages` | instalar, atualizar e remover programas (winget, brew, apt/dnf/pacman) |
| `system.disk` | uso, limpeza de temporários e caches |
| `system.network` | interfaces, Wi-Fi, conexões, testes |
| `system.power` | suspender, reiniciar, desligar, perfil de energia |
| `system.schedule` | tarefas agendadas do SO |
| `system.open` | abrir arquivos, pastas, URLs e programas |
| `system.clipboard` / `system.notify` | área de transferência e notificações |
| `system.elevate` | roda um comando como administrador (UAC/sudo/polkit) |

"Sem restrição" significa: tudo o que **a conta do usuário** pode fazer.
Ações de administrador passam pela elevação do próprio sistema; o usuário
pode guardar a senha de sudo no cofre (ADR-0020) para que a Nativa não
pergunte. O Orchestrator não contorna as proteções do SO.

### 4. Rotinas: a lista de funções

Uma **rotina** é uma lista ordenada de passos guardada no SQLite:

```yaml
nome: fim do dia
gatilho: { cron: "0 18 * * 1-5" }   # manual | cron | evento | ao iniciar
passos:
  - git.status: { todos_os_projetos: true }
  - pedir: "faça commit do que estiver pronto e me resuma"
  - orchestrator.backup: {}
  - system.power: { acao: suspender, se_ocioso_min: 30 }
```

- Passos podem ser chamadas diretas de ferramenta (não gastam modelo) ou
  pedidos em linguagem natural (a Nativa decide).
- Gatilhos de evento: projeto aberto, task concluída, bateria baixa,
  arquivo mudou, CI falhou, horário.
- Painel **ROTINAS**: criar, editar, ligar/desligar, rodar agora, ver o
  histórico de cada execução.
- A fila de funções aceita também pedidos avulsos: "quando terminar isso,
  faça aquilo".

### 5. Aprendizado sem treinar pesos

Em 8 GB não dá para treinar o modelo. A Nativa aprende por **memória e
habilidades**:

1. **Observação.** As ações do usuário (`CallOrigin::User` no runtime e os
   comandos da interface) são gravadas como **trilhas** com horário e
   projeto. Liga/desliga por projeto; trilhas apagáveis.
2. **Mineração.** Um trabalho em segundo plano acha sequências repetidas
   (mesmos passos, ≥ 3 vezes) e propõe: "Notei que toda segunda você faz
   X, Y e Z. Quer que eu vire isso uma rotina?" — com os valores que
   mudam viram parâmetros.
3. **Ensinar mostrando.** Botão "Gravar": o usuário faz a tarefa; ao parar,
   a trilha vira uma **habilidade** nomeada, editável, que a Nativa pode
   chamar.
4. **Correções e preferências.** Quando o usuário corrige ou nega algo, a
   Nativa grava a preferência na memória L3 ("usar pnpm, não npm";
   "nunca desligar o PC com download rodando").
5. **Recuperação.** A cada pedido, as 3–5 habilidades e preferências mais
   próximas (por embeddings) entram no prompt como exemplos. É isso que
   faz um modelo pequeno acertar.
6. **Opcional, depois:** exportar as trilhas como dataset para um ajuste
   LoRA feito em outra máquina e importar o adaptador no motor.

### 6. Autonomia

A Nativa segue os modos da ADR-0016 como qualquer agente. No **Acesso
Irrestrito**, nada é perguntado. O que **sempre** fica, porque não é
restrição:

- auditoria de toda chamada no histórico;
- os botões Parar/Pausar e um atalho global de parada;
- os pontos de restauração (ADR-0022) antes de rotinas que mudam o
  Orchestrator.

## Consequências

- O Orchestrator passa a ter uma IA que funciona offline, de graça, num
  notebook comum, e que pode cuidar dele e do computador.
- Modelos de 3–4B são mais fracos que os de API: tarefas longas de código
  continuam melhores com os providers grandes, e a Nativa pode delegá-las
  (handoff, ADR-0013).
- Com Acesso Irrestrito e `system.*`, um erro do modelo pode apagar
  arquivos ou desligar o computador. O usuário escolhe isso conscientemente;
  a tela de ativação diz isso claramente.
- As trilhas guardam o que o usuário faz: ficam só no computador, nunca
  vão a providers externos sem pedido explícito.

Plano de implementação em [`docs/native-ai.md`](../native-ai.md).

# ADR-0020 — Acesso total da IA: internet, APIs e segredos

- **Estado:** Aceita
- **Fase:** depois da 12 (pedido do usuário)

## Contexto

O usuário pediu que a IA tenha "acesso total, 100%, sem exceções". O que
existe hoje:

- o **Tool Runtime** não restringe caminhos: arquivos de qualquer lugar,
  comandos, terminais, processos, git e GitHub;
- o modo **Acesso Irrestrito** (ADR-0016) não avalia nada: nenhuma
  chamada pede autorização nem é recusada pelo gate;
- mesmo assim, a IA respondia "não consigo acessar suas contas, não tenho
  navegador, não tenho credenciais". Três motivos:
  1. **nenhuma ferramenta de internet**: para ler uma página ou chamar
     uma API, ela teria de improvisar `curl`/`Invoke-WebRequest` no
     terminal, o que os modelos raramente fazem por conta própria;
  2. **nenhuma forma de usar credenciais** de outros serviços sem que o
     usuário as cole na conversa (e elas fiquem no histórico);
  3. a instrução de sistema da última mudança dizia "não tem navegador e
     não pode fazer login em sites", o que reforça a recusa.

O que **não** muda e por quê:

- **auditoria**: toda chamada continua no histórico. Não é limite de
  acesso; é o registro do que foi feito, e é o que torna o acesso total
  aceitável;
- **travas de arquivo entre agentes** (ADR-0015): coordenam dois agentes
  no mesmo arquivo, nunca bloqueiam o usuário nem uma IA sozinha;
- **tetos de custo e orçamento** (ADR-0018): só existem se o usuário os
  configurar;
- **os botões do usuário** (Parar, Pausar, Cancelar).

## Decisão

### 1. Duas ferramentas de internet no Tool Runtime

| Ferramenta | Tipo | Faz |
| ---------- | ---- | --- |
| `web.fetch` | consulta | `GET` de uma URL; devolve o status, o tipo e o texto. Páginas HTML viram texto legível (sem scripts, estilos e tags); `format: "raw"` devolve o HTML |
| `http.request` | ação | qualquer método (`GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD`), cabeçalhos, corpo em texto ou JSON; devolve status, cabeçalhos e corpo (JSON já interpretado quando a resposta é JSON) |

- Só `http`/`https`. Redirecionamentos seguidos (até 10). Tempo padrão de
  60 s (`timeoutMs` muda). Corpo cortado em 1 MB por padrão (`maxBytes`),
  com `truncated: true`.
- Passam pelo mesmo `invoke` auditado de toda ferramenta.

### 2. Segredos por nome, nunca pelo valor

O usuário guarda **segredos** (chaves de API, tokens) no cofre do sistema,
cada um com um nome (`OPENROUTER_KEY`, `STRIPE_TOKEN`…). A IA os usa
escrevendo `{{secret:NOME}}`:

- em `web.fetch` e `http.request`: na URL, nos cabeçalhos e no corpo;
- em `shell.execute`: nos valores de `env` (para CLIs que leem variáveis
  de ambiente).

O Orchestrator troca o marcador pelo valor **na hora de executar**:

- a IA nunca vê o valor; os argumentos no histórico guardam o marcador;
- o valor é **mascarado** (`***`) em tudo o que volta para a IA e para o
  histórico: corpo, cabeçalhos, mensagens de erro, saída do comando;
- um nome que não existe é recusado (`INVALID_ARGS`) dizendo onde
  cadastrar;
- a lista de nomes (sem valores) fica em `<app-data>/secrets.json`; os
  valores, só no cofre (Gerenciador de Credenciais do Windows, Keychain,
  Secret Service).

A instrução de sistema lista os **nomes** dos segredos disponíveis, para a
IA saber o que pode usar.

### 3. Autonomia

| Modo | `web.fetch` | `http.request` |
| ---- | ----------- | -------------- |
| Assistido | roda (consulta) | pede autorização |
| Autônomo (regras padrão de instalações novas) | roda | pede autorização: "envia dados para a internet, às vezes com seus segredos" |
| Acesso Irrestrito | roda | roda — **nada é perguntado nem recusado** |

Instalações que já têm `autonomy.json` mantêm as regras salvas; a regra
nova aparece em "Restaurar padrão".

### 4. Instrução de sistema sem recusa prévia

O parágrafo de capacidades (montado pelos nomes das ferramentas) passa a
dizer:

- o que as ferramentas alcançam, incluindo a internet e as APIs;
- **quem decide o que precisa de autorização é o Orchestrator**: a IA pede
  o que a tarefa precisa e nunca recusa de antemão por "falta de acesso";
- os segredos disponíveis, pelo nome, e como usá-los;
- que não há navegador gráfico: sites que só funcionam com login
  interativo pedem uma chave de API ou um token, guardado como segredo.

## Consequências

- No Acesso Irrestrito, a IA pode ler qualquer arquivo, rodar qualquer
  comando, publicar no GitHub e chamar qualquer API com os segredos
  guardados, **sem perguntar**. É o pedido do usuário; o risco é real:
  um texto lido da internet pode tentar instruir a IA (prompt injection)
  a enviar arquivos ou segredos para fora. Mitigações que não limitam o
  acesso: os segredos nunca aparecem para a IA (ela não consegue copiá-los
  para um texto), tudo fica no histórico, e o modo é escolhido por
  projeto e por agente.
- Sem navegador: login por formulário, captcha e autenticação em duas
  etapas continuam fora do alcance. O caminho é a API do serviço.

## Fica para depois

- Navegador controlado (Playwright) como ferramenta.
- OAuth pelo navegador para serviços comuns.
- Segredos por projeto (hoje valem para todos).

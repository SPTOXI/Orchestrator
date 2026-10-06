# ADR-0021 — Configurações, regras e skills, MCP, assinaturas por CLI e modelos offline

- **Estado:** Aceita; os modelos offline pelo Ollama (item 5) foram
  substituídos pelo motor próprio da
  [ADR-0025](./0025-motor-local-proprio.md)
- **Fase:** depois da 12 (pedido do usuário)

## Contexto

O usuário pediu, numa mensagem só:

1. uma **área de configuração** do Orchestrator para skills, CLI, MCP,
   políticas, regras de desenvolvimento etc.;
2. a lista das IAs na barra lateral **retrátil**, porque ocupa espaço;
3. **logins para usar as assinaturas** (ChatGPT, Claude, Antigravity…) por
   CLI ou MCP, como alternativa às APIs;
4. **baixar e usar modelos offline**.

Até aqui as IAs só entravam por API (ADR-0010) e as configurações estavam
espalhadas em abas soltas (Autonomia, GitHub, Contexto, Sobre). A
arquitetura impõe uma regra que nenhuma dessas novidades pode quebrar: **o
provider nunca toca no sistema** (ADR-0009); toda ação de uma IA passa pelo
`TurnContext::call_tool`, pelo gate de autonomia (ADR-0016), pelas travas
(ADR-0015) e pelo histórico.

## Decisão

### 1. Uma aba "Configurações"

Engrenagem na barra de atividades; uma aba só, com navegação por grupos:

| Grupo | Seções |
| ----- | ------ |
| IAs | Regras de desenvolvimento · Skills · Políticas e segredos · Contexto e compactação |
| Modelos | Assinaturas (CLI) · Modelos offline |
| Integrações | Servidores MCP · GitHub |
| Orchestrator | Sobre e atualizações |

As abas antigas (Autonomia, GitHub, Sobre) viram seções: o chip de
autonomia, o link do GitHub e a versão na barra de status abrem a seção
certa.

### 2. Regras de desenvolvimento e skills (`orchestrator-engine::guidance`)

- **Regras**: as do usuário (`<dados>/rules.md`, valem para todo projeto) e
  os arquivos de instruções do projeto (`AGENTS.md`, `CLAUDE.md`,
  `.orchestrator/rules.md`; só lidos, até 20.000 caracteres cada) vão
  **inteiros** nas instruções de cada sessão nova, antes do contexto do
  projeto, numa seção `## DEVELOPMENT RULES`. Não entram no orçamento do
  contexto: regra cortada não é regra.
- **Skills**: o formato do Claude Code — uma pasta com `SKILL.md` (front
  matter `name`, `description`, depois as instruções). Três origens, nesta
  ordem: as do Orchestrator (`<dados>/skills`, editáveis na tela), as do
  projeto (`.orchestrator/skills`, `.claude/skills`) e as do usuário no
  Claude Code (`~/.claude/skills`). Uma skill com o mesmo nome de outra que
  vem antes fica "sombreada". As IAs recebem só **nome e descrição**
  (`## SKILLS`) e leem a skill inteira com `skill.read` quando a tarefa
  pede — o mesmo "carregar sob demanda" do Claude Code, para não gastar
  contexto com o que não se usa.
- `GuidedContext` embrulha o Context Builder: regras e skills chegam mesmo
  com o contexto do projeto desligado. `skill.list` e `skill.read` são
  consultas; `SkillTools` entra na cadeia de ferramentas, dentro do gate.
- Nada é gravado dentro do projeto.

### 3. Servidores MCP (`orchestrator-mcp`)

- Cliente MCP (protocolo 2025-06-18) por **stdio** e **Streamable HTTP**
  (resposta em JSON ou SSE, `Mcp-Session-Id`): `initialize`, `tools/list`
  paginado, `tools/call` com tempo limite e cancelamento
  (`notifications/cancelled`), resposta ao `ping` e ao `roots/list` do
  servidor, e `tools/list_changed`.
- `mcp.json` guarda os servidores; a tela importa o JSON do Claude
  Desktop, Claude Code e Cursor (`{"mcpServers": …}`). Variáveis e
  cabeçalhos aceitam `{{secret:NOME}}` (ADR-0020); o valor é mascarado no
  que volta.
- As ferramentas chegam a **todas** as IAs como `mcp.<servidor>.<ferramenta>`,
  com nomes que toda API aceita (sem `__`, até 64 caracteres no fio). Ficam
  **dentro** do gate: `readOnlyHint` conta como consulta, o resto como
  ação. Como os servidores conectam depois de o app abrir, o gate, ao ver
  um nome que não conhece, consulta a lista de novo antes de deixá-lo
  passar.
- No Windows, `npx`/`uvx` (scripts `.cmd`) rodam por `cmd /C`, sem janela.

### 4. Assinaturas por CLI (`orchestrator-provider-cli`)

Não há API pública para usar uma **assinatura** (Claude Pro/Max, ChatGPT
Plus/Pro, conta Google) fora dos programas dos próprios fornecedores. O
caminho oficial são as CLIs deles, que fazem o login no navegador e
aceitam rodar sem interface:

| Provider | CLI | Roda | Sessão | Ferramentas próprias |
| -------- | --- | ---- | ------ | -------------------- |
| `claude-code` | Claude Code | `claude -p --output-format stream-json` | `--session-id` / `--resume` | `--tools ""`, `--strict-mcp-config` |
| `codex` | Codex | `codex exec --json` | `thread_id` → `exec resume` | `-s read-only` |
| `gemini-cli` | Gemini CLI | `gemini -o stream-json -p ""` | `--session-id` / `--resume` | `tools.exclude` |

- Cada turno roda a CLI uma vez, com a mensagem pela entrada padrão; a
  saída vira texto, raciocínio e uso. A sessão da CLI é retomada pelo id,
  guardado no snapshot (sobrevive a reinícios).
- **As ferramentas são as do Orchestrator**, entregues por MCP: o
  `ToolServer` (em `orchestrator-mcp`) serve, em `127.0.0.1` e com um token
  aleatório por turno, um endpoint MCP que roda cada chamada pelo
  `TurnContext` daquele turno — gate, travas e histórico, como numa API. As
  ferramentas próprias das CLIs ficam **desligadas**; há uma opção, por
  CLI, para ligá-las, com o aviso de que ficam fora das regras.
- Claude Code recebe as instruções (contexto, regras, skills) por
  `--append-system-prompt-file`; Codex e Gemini, na primeira mensagem da
  sessão.
- Os arquivos de uma execução ficam em `<dados>/cli/<sessão>`, numa pasta
  só do usuário (0700): o Gemini CLI ignora configurações em pastas que
  outros podem alterar.
- Sem login, sem instalação, limite de uso: o erro diz o que fazer.
  Cancelar mata a CLI. `complete()` (Conselho) roda sem ferramentas e sem
  guardar sessão.
- **Custo**: a assinatura não cobra por token; o turno registra US$ 0 e os
  tokens que a CLI informa.
- A tela mostra instalado/versão/login (`claude auth status`, `codex login
  status`, as credenciais do Gemini) e tem **Instalar** e **Entrar**, que
  rodam o comando num terminal do Orchestrator.
- **Antigravity** é um editor do Google sem CLI que outros programas
  possam usar; a conta Google entra pelo Gemini CLI.

### 5. Modelos offline (Ollama)

- O Orchestrator não embute um motor de inferência: o **Ollama** é
  gratuito, roda em Windows, macOS e Linux, usa a placa de vídeo e serve
  uma API compatível com a OpenAI — ou seja, os modelos entram como uma
  conexão de API comum (ADR-0010), sem código novo de protocolo.
- Configurações → Modelos offline: Ollama instalado/rodando
  (`OLLAMA_HOST` ou `127.0.0.1:11434`), **Iniciar**, **Instalar** num
  terminal (`winget`, `brew` ou o script oficial) ou pelo site; os modelos
  do computador com o que o Ollama diz deles (`/api/show`: ferramentas,
  imagens, contexto); sugestões que rodam num computador comum; baixar
  qualquer modelo com progresso (`/api/pull`) e cancelar; apagar.
- **Usar nas sessões** cria (ou atualiza) a conexão `ollama`: sem chave,
  preço zero, `firstResponseSecs` de 600 (carregar um modelo na memória
  demora), modelos marcados `local`/`offline`. Novos downloads entram nela
  sozinhos.
- Um modelo marcado sem chamada de ferramentas recebe as ferramentas **por
  prompt** (ADR-0010) em vez de um pedido que a API recusaria — vale para
  qualquer conexão.

### 6. IAs retráteis

A lista de IAs, o Conselho e as conexões desativadas viram seções que
recolhem (a escolha fica salva); cada IA mostra uma linha e abre os
detalhes num clique. A lista vazia aponta para os três caminhos: API,
assinatura e offline.

## Consequências

- **Três jeitos de ter IAs** — API, assinatura e offline — e todos com as
  mesmas ferramentas, regras, skills e servidores MCP, sob o mesmo gate.
- As CLIs mudam de versão por conta própria. O Orchestrator usa só opções
  documentadas e testou as versões atuais (Claude Code 2.1, Codex 0.160,
  Gemini CLI 0.62); a tela tem "Argumentos extras" e "Caminho do
  programa" para acomodar o que mudar. `tools.exclude` já aparece como
  obsoleto no Gemini CLI.
- O limite de uso de uma assinatura é do fornecedor; o Orchestrator não o
  vê antes de a CLI recusar.
- Modelos offline são menores: bons para o dia a dia e para privacidade,
  piores que os grandes em tarefas longas. O catálogo indica a memória
  necessária.
- O `ToolServer` escuta só em `127.0.0.1`, com um token novo por turno que
  some quando o turno acaba.

### Fica para depois

- Login por OAuth do próprio Orchestrator para MCP remoto (hoje: token em
  cabeçalho, como segredo).
- Recursos (`resources/*`) e prompts de MCP; transporte SSE antigo.
- LM Studio e llama.cpp além do Ollama.
- Regras por projeto editadas na tela (hoje: memória do projeto, tipo
  "regra", ou os arquivos do projeto).

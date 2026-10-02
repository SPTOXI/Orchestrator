# Configurações

A engrenagem na barra de atividades (embaixo, à esquerda) abre a aba
**Configurações**, com tudo o que muda o que as IAs recebem e como elas
rodam. Decisão: [ADR-0021](./adr/0021-configuracoes-assinaturas-offline-e-mcp.md).

| Grupo | Seção | O que faz |
| ----- | ----- | --------- |
| IAs | Regras de desenvolvimento | o que toda IA segue, em todo projeto |
| IAs | Skills | instruções por tipo de trabalho, lidas sob demanda |
| IAs | Políticas e segredos | modo de autonomia, regras do Autônomo, segredos ([autonomy.md](./autonomy.md)) |
| IAs | Contexto e compactação | o contexto do projeto e quando compactar ([context.md](./context.md), [tokens.md](./tokens.md)) |
| Modelos | Assinaturas (CLI) | Claude Code, Codex e Gemini CLI com a sua conta |
| Modelos | Modelos offline | modelos no seu computador, pelo Ollama |
| Integrações | Servidores MCP | ferramentas novas para todas as IAs |
| Integrações | GitHub | token e servidor ([github.md](./github.md)) |
| Orchestrator | Sobre e atualizações | versão, pastas, atualizações ([release.md](./release.md)) |

O chip de autonomia, o link do GitHub e a versão na barra de status abrem a
seção certa.

## Regras de desenvolvimento

- **As suas regras** (um texto livre, até 40.000 caracteres) ficam em
  `<dados>/rules.md` e valem para todo projeto.
- **Os arquivos do projeto** — `AGENTS.md`, `CLAUDE.md` e
  `.orchestrator/rules.md` — são lidos (nunca alterados), até 20.000
  caracteres cada.
- Tudo vai **inteiro** nas instruções de cada sessão nova (sessões, agentes,
  tasks, Conselho em Full), na seção `## DEVELOPMENT RULES`, antes do
  contexto do projeto e fora do orçamento dele. Vale também com o contexto
  do projeto desligado.
- Regras de um projeto só: a memória do projeto (tipo "regra") ou os
  arquivos do próprio projeto.

## Skills

Uma skill é uma pasta com um `SKILL.md`:

```markdown
---
name: revisar-pr
description: Ao revisar um pull request antes do merge
---

1. Leia a descrição e o diff inteiro.
2. Rode os testes.
```

- Origens, nesta ordem: **Orchestrator** (`<dados>/skills`, criadas e
  editadas na tela), **projeto** (`.orchestrator/skills`, `.claude/skills`)
  e **Claude Code** (`~/.claude/skills`). Com o mesmo nome, a primeira vale
  e as outras aparecem como sombreadas.
- As IAs recebem só **nome e descrição** (`## SKILLS`); quando a tarefa
  combina, chamam `skill.read` e seguem as instruções. `skill.list` lista as
  disponíveis. As duas são consultas (não pedem autorização).
- Cada skill pode ser desligada; cada origem também.

## Servidores MCP

MCP (Model Context Protocol) é o padrão para dar ferramentas novas às IAs:
navegador, banco de dados, Jira, Figma, documentação…

- **Adicionar**: programa local (stdio: comando, argumentos, variáveis,
  pasta) ou servidor remoto (HTTP: URL e cabeçalhos). **Importar JSON**
  aceita o formato do Claude Desktop, Claude Code e Cursor:

  ```json
  {"mcpServers": {
    "playwright": {"command": "npx", "args": ["-y", "@playwright/mcp@latest"]},
    "github": {"type": "http", "url": "https://api.githubcopilot.com/mcp/",
               "headers": {"Authorization": "Bearer {{secret:GITHUB_TOKEN}}"}}
  }}
  ```

  Modelos prontos: Playwright (um navegador de verdade), Context7
  (documentação de bibliotecas) e o servidor oficial do GitHub.
- Valores secretos: `{{secret:NOME}}` em variáveis e cabeçalhos (o valor
  vem do cofre e é mascarado no que volta).
- As ferramentas chegam a **todas** as IAs — APIs, offline e assinaturas —
  como `mcp.<servidor>.<ferramenta>`. Passam pelo gate de autonomia (as que
  o servidor marca como consulta contam como consulta) e ficam no histórico.
  Cada ferramenta pode ser escondida das IAs.
- O servidor conecta ao abrir o app e ao salvar; a tela mostra o estado, as
  ferramentas e as últimas linhas que ele escreveu (stderr). No Windows,
  `npx`/`uvx` funcionam sem abrir janela.
- Tempo máximo por chamada: 300 s (ajustável). Cancelar o turno cancela a
  chamada.
- Arquivo: `<dados>/mcp.json`.

## Assinaturas (CLI)

Use as assinaturas que você já paga, sem chave de API:

| IA | Assinatura | Instalar | Entrar |
| -- | ---------- | -------- | ------ |
| Claude Code | Claude Pro ou Max (ou chave da Anthropic) | `npm install -g @anthropic-ai/claude-code` | `claude auth login` |
| Codex | ChatGPT Plus, Pro, Business ou Enterprise | `npm install -g @openai/codex` | `codex login` |
| Gemini CLI | conta Google (gratuita, Google AI Pro ou Ultra) | `npm install -g @google/gemini-cli` | `gemini` → "Login with Google" |

- Os botões **Instalar** e **Entrar** rodam o comando num terminal do
  Orchestrator (o login abre o navegador). A instalação pelo npm precisa do
  Node.js.
- **Usar no Orchestrator** liga a IA: ela aparece em AI Providers, nas
  sessões, nos agentes e no Conselho.
- Cada turno roda a CLI oficial com **as ferramentas do Orchestrator**,
  entregues por MCP (um endereço local, só desta máquina, com um token novo
  por turno). As ferramentas próprias da CLI ficam desligadas: tudo passa
  pelas regras de autonomia, pelas travas e pelo histórico. A opção "Deixar
  a CLI usar também as ferramentas próprias dela" existe, por sua conta.
- A conversa continua entre turnos (a CLI retoma a própria sessão), também
  depois de reiniciar o app.
- **Custo**: a assinatura não cobra por token; o turno conta US$ 0 e mostra
  os tokens que a CLI informa. O limite de uso é o da sua assinatura.
- Opções: modelos (ex.: `sonnet, opus` no Claude Code), modelo padrão,
  caminho do programa (se não estiver no PATH) e argumentos extras.
- **Google Antigravity** é um editor, sem CLI que outros programas possam
  usar; a conta Google entra pelo Gemini CLI.
- Arquivo: `<dados>/clis.json`.

## Modelos offline

Modelos que rodam no seu computador: sem internet, sem chave, sem custo
por token. Pelo **Ollama** (gratuito).

1. **Instalar o Ollama**: o botão roda, num terminal, `winget install
   Ollama.Ollama` (Windows), `brew install ollama` (macOS) ou o script
   oficial (Linux); ou **Baixar do site**. Se estiver instalado e parado,
   **Iniciar o Ollama**.
2. **Baixar modelos**: sugestões que funcionam com as ferramentas do
   Orchestrator, com o tamanho e a memória que pedem, ou qualquer nome de
   [ollama.com/library](https://ollama.com/library). O progresso aparece na
   tela; dá para cancelar (o Ollama retoma depois).
3. **Usar nas sessões**: cria a conexão `ollama` (compatível com a OpenAI,
   sem chave, preço zero, espera de até 600 s pela primeira resposta,
   porque carregar o modelo na memória demora). Novos downloads entram nela
   sozinhos.

Modelos sem chamada de ferramentas (o Ollama informa) recebem as
ferramentas por prompt. O endereço é `127.0.0.1:11434`, ou o de
`OLLAMA_HOST`.

## IAs na barra lateral

A lista de IAs, o Conselho e as conexões desativadas recolhem (a seta no
título; a escolha fica salva). Cada IA ocupa uma linha — nome, estado e
modelo padrão — e abre os detalhes num clique na seta.

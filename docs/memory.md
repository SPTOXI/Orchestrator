# Banco local, memória e histórico

Referência da Fase 6. Decisão em
[ADR-0012](./adr/0012-sqlite-memoria-e-historico.md); código em
[`packages/memory`](../packages/memory/README.md) (crate
`orchestrator-memory`).

> A memória pertence ao projeto, não ao provider. Trocar de IA não apaga o
> que o projeto sabe.

## O banco

- **Onde:** um arquivo por instalação, `<app-data>/orchestrator.db`:
  - Linux: `~/.local/share/dev.orchestrator.desktop/`;
  - Windows: `%APPDATA%\dev.orchestrator.desktop\`;
  - macOS: `~/Library/Application Support/dev.orchestrator.desktop/`.
- **Nunca dentro do projeto:** nada aparece no `git status` do usuário.
- **Motor:** SQLite embutido (`rusqlite` com `bundled`), o mesmo nos três
  sistemas, em modo WAL, com chaves estrangeiras.
- **Migrações:** `PRAGMA user_version`. Um banco gravado por uma versão mais
  nova do Orchestrator é recusado, em vez de corrompido.
- **Se o arquivo não abrir** (disco cheio, permissão), o app usa um banco
  em memória. A barra de status avisa que nada será guardado ao fechar, e
  o rodapé do HISTORY mostra `banco: (memória)`.
- **Segredos:** nunca vão para o banco. As chaves de API continuam só no
  cofre do sistema.
- **Configuração:** continua em arquivos (`connections.json`,
  `council.json`).

### Esquema (v1)

| Tabela | Conteúdo |
| ------ | -------- |
| `projects` | caminho (único), nome, criado em, último acesso, stack detectada (JSON), `hidden` (fora da lista de recentes) |
| `audit_events` | histórico: cada `AuditEvent`, com `project_id` e `session_id` |
| `tool_calls` | *view* sobre `audit_events` (`TOOL_CALLED`): ferramenta, ok, duração |
| `sessions` | sessões de provider: `SessionInfo`, sessão nativa (com a conversa), instruções e modelo pedido |
| `session_entries` | transcript de cada sessão, por `seq` |
| `memory_entries` | memória L2 |
| `decisions` | decisões do projeto |
| `deliberations` | deliberações do Conselho, com a chave e a validade do cache |
| `search_index` | índice FTS5 da busca L3 |

As tabelas das próximas fases (tasks, agentes, file locks, artefatos,
operações Git) entram com as migrações dessas fases.

## Histórico

O `DesktopSink` grava cada `AuditEvent` antes de emiti-lo para a UI e marca
o projeto, nesta ordem:

1. `projectId` ou `projectPath` nos dados do evento;
2. o projeto da sessão (`sessionId` nos dados ou na origem `agent`);
3. o projeto aberto no app.

A chamada `project.open` é gravada logo antes do seu `PROJECT_OPENED`, com o
mesmo `call_id`. O `PROJECT_OPENED` a remarca com o projeto aberto por ela.

- **Consulta:** `history_query` pagina por cursor (`before` = `next` da
  página anterior). Cada página vem do mais antigo para o mais novo.
- **Filtros:** projeto, tipos de evento, texto (no resumo) e
  `hideReads` (esconde as chamadas de ferramentas somente leitura que deram
  certo).
- **UI (HISTORY):**
  - "Só este projeto" filtra pelo projeto aberto;
  - "Carregar mais antigos" pede a página anterior (500 eventos);
  - o rodapé mostra o arquivo do banco.
- **Migração:** na primeira execução, o `audit.jsonl` das Fases 1–5 é
  importado e renomeado para `audit.jsonl.imported`. Pode ser apagado.

## Projetos

- **Registro:** `PROJECT_OPENED` registra o projeto (ou atualiza o último
  acesso e a stack). Na primeira vez, gera `PROJECT_CREATED`.
- **Identidade:** o projeto é identificado pelo caminho. Mover a pasta
  cria outro projeto.
- **Recentes:** a lista vem do banco. A lista antiga do `localStorage` é
  importada uma vez.
- **Esquecer** tira da lista; memória e histórico ficam.

## Sessões e conversas

O `SessionManager` grava as sessões pelo `SessionStore`
([providers.md](./providers.md#persistência-sessionstore-fase-6)):

- **Quando grava:** ao abrir, encerrar e retomar, e ao fim de cada turno.
  Grava o transcript novo e a sessão nativa de `AIProvider::snapshot`.
- **Conexões de API:** a sessão nativa leva a conversa inteira enviada à
  API, com as partes nativas (assinaturas de raciocínio).
- **Ao reiniciar,** as sessões voltam **encerradas** (e são gravadas assim),
  com o transcript, os turnos, o uso e o custo. "Retomar" reconstrói a
  conversa, e a próxima requisição leva todas as mensagens anteriores.

## Memória do projeto

### L1 — trabalho

Derivada do histórico a cada consulta, sem tabela própria:

| Seção | O que mostra | Limite |
| ----- | ------------ | ------ |
| Sessões | título, provider/modelo, estado, turnos | 8 mais recentes |
| Arquivos alterados | caminho (relativo ao projeto), mudança, quem, quando | 15, sem repetir caminho |
| Comandos | comando, código de saída (ou "em segundo plano") | 10 |
| Erros recentes | ferramentas com `ok: false`, turnos `failed`, comandos e processos com código ≠ 0 (processos parados pelo usuário não contam) | 10 |

### L2 — projeto

Entradas editáveis:

- **Tipos:** `architecture`, `stack`, `convention`, `rule`, `note`.
- **Campos:** título, conteúdo, etiquetas e "fixar no topo".
- **Origem:** `user`, `agent` ou `detector`.
- **Stack detectada:** ao abrir o projeto, a entrada "Stack detectada"
  (origem `detector`, fixada) é criada ou atualizada com linguagens,
  frameworks, gerenciadores de pacote, runtimes, bancos, ferramentas e
  Docker. Depois que o usuário a edita, ela vira `user` e o detector não a
  sobrescreve mais.

### Decisões

- **Campos:** título, contexto, decisão, consequências e estado
  (`proposed`, `accepted`, `superseded`, `rejected`).
- **Nunca são apagadas:** mudam de estado.
- **Evento:** `DECISION_SAVED`, com a mudança no resumo, por exemplo
  "decisão proposta → aceita · Stripe como gateway".

### L3 — busca

Busca de texto (FTS5) sem acento e sem diferenciar maiúsculas. Cada palavra
é tratada como prefixo ("retent" encontra "retentativas"). Cobre:

- memória L2 (título, conteúdo, etiquetas);
- decisões;
- mensagens das sessões: o que o usuário enviou e o texto das respostas;
- eventos notáveis: commits, pushes, comandos, ferramentas e turnos que
  falharam.

O resultado vem por relevância, com o trecho e os termos marcados. Um clique
abre a entrada, a decisão ou a sessão.

### Eventos

| Evento | Quando | `data` |
| ------ | ------ | ------ |
| `PROJECT_CREATED` | primeira abertura de um projeto | `projectId`, `path`, `name` |
| `MEMORY_SAVED` | entrada L2 criada ou alterada | `projectId`, `entryId`, `kind`, `title`, `source`, `pinned`, `created`, `chars` (o conteúdo não vai para o evento) |
| `MEMORY_REMOVED` | entrada L2 apagada | `projectId`, `entryId`, `kind`, `title` |
| `DECISION_SAVED` | decisão registrada ou alterada | `projectId`, `decisionId`, `title`, `status`, `previousStatus`, `source` |

O que a UI grava tem origem `user`. Ferramentas para as IAs lerem e
escreverem a memória entram com o Context Builder (Fase 7).

## Conselho

As deliberações vão para `deliberations`. O cache vale entre execuções: uma
deliberação reaproveitável guarda a chave e a validade. As 50 últimas
voltam ao histórico do Conselho ao iniciar. Salvar a configuração do
Conselho limpa o cache guardado ([router.md](./router.md#cache)).

## UI

- **Painel MEMORY (barra lateral):**
  - busca L3;
  - contagens (L2, decisões, sessões, eventos);
  - entradas fixadas;
  - um resumo do L1 (sessões, arquivos, erros).
- **Aba "Memória do projeto":** seções Trabalho (L1), Projeto (L2),
  Decisões e Busca (L3), com os editores de entradas e decisões.
- **Atualização:** as seções se atualizam com os eventos do histórico
  (arquivos, comandos, turnos, memória).

## IPC

Ver [ipc.md](./ipc.md#histórico-projetos-e-memória-fase-6-adr-0012).

# ADR-0012 — SQLite, memória do projeto e histórico

- **Estado:** Aceita
- **Fase:** 6

## Contexto

Até a Fase 5, o que o Orchestrator sabe se perde ou fica espalhado:

- **Histórico:** em `audit.jsonl`, lido de uma janela em memória
  (ADR-0005).
- **Sessões e transcripts:** só em memória (ADR-0009). A conversa enviada às
  APIs vive no adapter (ADR-0010).
- **Deliberações e cache do Conselho:** só em memória (ADR-0011).
- **Projetos recentes:** no `localStorage` da webview (ADR-0008).
- **Memória de projeto:** não existe. O documento mestre prevê memória L1
  (trabalho), L2 (projeto) e L3 (histórico), além de decisões, e diz que
  **a memória pertence ao projeto, não ao provider**.

A Fase 6 traz o banco local e a memória. O Context Builder, que decide o
que dessa memória vai para cada IA, é a Fase 7.

## Decisão

1. **Crate `packages/memory` (`orchestrator-memory`).**
   - Usa `rusqlite` com o SQLite **embutido** (`bundled`), igual nos três
     sistemas e sem dependência do sistema.
   - Depende só de `orchestrator-core`. Os demais crates recebem traits
     (`SessionStore`), e o app liga as pontas.

2. **Um banco por instalação: `<app-data>/orchestrator.db`.**
   - WAL, chaves estrangeiras e migrações por `PRAGMA user_version`.
   - Tudo o que pertence a um projeto tem `project_id`: a memória pertence
     ao projeto, e o banco nunca escreve dentro da pasta do projeto (nada
     aparece no `git status` do usuário).
   - O projeto é identificado pelo caminho. Mover a pasta cria outro
     projeto; migrar isso fica para depois.
   - Se o banco não abrir (disco, permissão), o app usa um banco em memória
     e mostra o aviso. Perder o banco nunca impede o uso.

3. **Esquema v1.**

   | Tabela | Conteúdo |
   | ------ | -------- |
   | `projects` | caminho, nome, criado em, último acesso, stack detectada |
   | `audit_events` | histórico durável (substitui o `audit.jsonl`), com `project_id` e `session_id` |
   | `sessions` | sessões de provider: provider, modelo, título, pai, estado, uso, sessão nativa (inclui a conversa), projeto |
   | `session_entries` | transcript numerado de cada sessão |
   | `memory_entries` | memória L2: arquitetura, stack, convenções, regras, notas, com etiquetas, fixação e origem |
   | `decisions` | decisões: contexto, decisão, consequências, estado, origem |
   | `deliberations` | deliberações do Conselho, com a chave e a validade do cache |
   | `search_index` | índice FTS5 da memória, das decisões e das mensagens das sessões (L3) |

   - `tool_calls` é uma *view* sobre `audit_events` (`TOOL_CALLED`): uma
     tabela a mais duplicaria o histórico.
   - As tabelas das próximas fases (`tasks`, `agents`, `file_locks`,
     `artifacts`, `git_operations`) entram com as migrações dessas fases.

4. **Histórico no banco.**
   - O sink do app grava cada `AuditEvent` em `audit_events` e marca o
     projeto:
     - `projectId` ou `projectPath` dos dados, se houver;
     - senão, o projeto da sessão (`sessionId` nos dados ou na origem);
     - senão, o projeto aberto.
   - A chamada `project.open` é gravada logo antes do seu `PROJECT_OPENED`
     (mesmo `call_id`), ainda com o projeto anterior. O `PROJECT_OPENED` a
     remarca com o projeto que ela abriu.
   - A consulta é paginada por cursor (`before`), com filtro por projeto,
     tipo de evento e texto.
   - Na primeira execução, o `audit.jsonl` é importado e renomeado para
     `audit.jsonl.imported`.

5. **Projetos.**
   - `PROJECT_OPENED` registra o projeto. Na primeira vez, gera o
     `PROJECT_CREATED` previsto desde a ADR-0008.
   - Os projetos recentes passam a vir do banco. A lista do `localStorage`
     é importada uma vez.
   - `PROJECT_OPENED` passa a levar também gerenciadores de pacote,
     runtimes, bancos e ferramentas detectados.

6. **Sessões e conversas sobrevivem ao reinício.**
   - O `SessionManager` recebe um `SessionStore`:
     - salva a sessão ao abrir, encerrar e retomar, e ao fim de cada turno
       (o transcript novo e a sessão nativa atualizada);
     - carrega as sessões ao iniciar.
   - Sessões carregadas voltam **encerradas**, com o transcript, e são
     gravadas assim (o L1 não as mostra abertas). "Retomar" reabre pelo
     provider.
   - Novo `AIProvider::snapshot(native)`: devolve a sessão nativa com o
     estado necessário para retomar depois. O padrão é devolvê-la como
     está.
     - O provider de API guarda a conversa inteira em `native.data`, com as
       partes nativas (assinaturas de raciocínio).
     - O `resume` a reconstrói. Assim a conversa continua após reiniciar,
       como a ADR-0010 previa.
   - O transcript é gravado por turno. Um turno interrompido por queda do
     app perde só o próprio andamento.

7. **Memória L1, L2 e L3.**
   - **L1 (trabalho):** derivada, sem tabela própria, de sessões ativas,
     arquivos alterados, comandos com código de saída e erros recentes do
     projeto (ferramentas e turnos que falharam). É calculada a cada
     consulta a partir do histórico.
   - **L2 (projeto):** entradas editáveis.
     - Tipos: `architecture`, `stack`, `convention`, `rule`, `note`.
     - Cada entrada tem etiquetas, fixação e origem (`user`, `agent`,
       `detector`).
     - Ao abrir o projeto, a entrada "Stack" (origem `detector`) é criada
       ou atualizada a partir da detecção. Uma edição do usuário vira
       origem `user` e deixa de ser sobrescrita.
   - **L3 (histórico):** busca de texto (FTS5, sem acento e sem diferenciar
     maiúsculas) na memória, nas decisões, nas mensagens das sessões e nos
     resumos do histórico.
   - **Decisões:** título, contexto, decisão, consequências, estado
     (`proposed`, `accepted`, `superseded`, `rejected`) e origem. Não são
     apagadas: mudam de estado, porque o histórico de decisões é parte da
     memória.
   - **Eventos novos:** `MEMORY_SAVED`, `MEMORY_REMOVED` e
     `DECISION_SAVED`.

8. **Conselho.** As deliberações vão para `deliberations`. O cache passa a
   valer entre execuções: uma entrada guarda a chave e a validade, e o
   histórico de deliberações da UI vem do banco.

9. **Configuração continua em arquivos.** `connections.json` e
   `council.json` ficam como estão. São configuração pequena, legível e fácil
   de copiar ou versionar, sem consultas. Isso revisa o "até o SQLite" das
   ADRs 0010 e 0011: dados vão para o banco, configuração fica em arquivo.
   Segredos continuam só no cofre do SO.

10. **Comandos Tauri.**
    - Histórico: `history_query` substitui a janela de `history_recent`,
      que continua existindo.
    - Projetos: `projects_recent`, `project_forget`,
      `projects_import_recent`.
    - Memória: `memory_overview` (L1 + contagens), `memory_list`,
      `memory_save`, `memory_delete` e `memory_search` (L3).
    - Decisões: `decisions_list` e `decision_save`.

    As escritas da UI têm origem `user`. Ferramentas para as IAs lerem e
    escreverem a memória entram com o Context Builder (Fase 7).

## Consequências

- Histórico, sessões, conversas, memória e deliberações sobrevivem ao
  reinício. O Context Builder da Fase 7 terá de onde escolher contexto.
- **Nova dependência:** `rusqlite` com SQLite embutido, que compila um
  arquivo C. O compilador C já é exigido pelo Tauri e pelo `ring`.
- **Tamanho do banco:** a conversa inteira de cada sessão vai para o banco
  a cada turno. Sessões muito longas ocupam espaço; a compactação é a Fase
  11.
- **Projeto marcado pelo contexto:** um evento sem projeto nos dados recebe
  o projeto aberto. O app abre um projeto por vez.
- **Arquivos antigos:** `audit.jsonl` vira `audit.jsonl.imported` e pode
  ser apagado pelo usuário.

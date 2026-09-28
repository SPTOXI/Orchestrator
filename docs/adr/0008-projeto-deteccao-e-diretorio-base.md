# ADR-0008 — Projeto: detecção no runtime, diretório base e ferramentas auxiliares

- **Estado:** Aceita
- **Fase:** 2

## Contexto

A seção 7 do documento mestre pede descoberta de projetos e um PROJECT
PROFILE (nome, caminho, Git root, branch, remote, linguagem, framework,
package manager, runtime, Docker, banco, arquivos importantes, status do
Git). A estrutura de pacotes não tem um lugar explícito para isso, e o
catálogo da seção 9 não lista ferramentas `project.*`. A seção 9 lista
`package.install`, `package.run`, `runtime.node`, `runtime.python` e
`runtime.docker`, que dependem da detecção de projeto.

## Decisão

1. **Tipos** do projeto (`ProjectProfile`, `ProjectCandidate`, `GitSummary`)
   ficam em `packages/core` (contratos, sem I/O). A **detecção** (ler
   arquivos, varrer diretórios) fica em `packages/runtime/src/project.rs`,
   pois é I/O sobre o workspace; o Git é consultado via `packages/git`.
2. Ferramentas novas, registradas no catálogo:
   - `project.discover` — varre raízes procurando projetos;
   - `project.profile` — perfil de uma pasta (padrão: projeto aberto);
   - `project.open` — define o projeto ativo; o **diretório base do runtime
     passa a ser a raiz do projeto** (caminhos relativos e `cwd` padrão) e é
     emitido `PROJECT_OPENED`.
3. `package.install`/`package.run` usam o gerenciador detectado no perfil
   (pnpm, yarn, npm, bun, pip, poetry, uv, pipenv, cargo, go) e executam pelo
   mesmo caminho de `shell.execute` (ou `process.start` com
   `background: true`). `runtime.node`/`runtime.python`/`runtime.docker`
   informam disponibilidade, caminho e versão.
4. A detecção é **heurística e baseada em evidências** (`markers`): cada
   conclusão vem de um arquivo ou dependência encontrada. O conteúdo de
   `.env` **nunca** é lido (só a existência do arquivo é registrada).
5. O catálogo passa a marcar cada ferramenta como `readOnly` (consulta) ou
   não (altera o estado). O evento `TOOL_CALLED` carrega essa marca; a UI a
   usa para filtrar o histórico e a Fase 9 poderá usá-la em políticas.
6. `PROJECT_CREATED` fica para a Fase 6: sem persistência não há como saber
   se um projeto é novo. Projetos recentes ficam no armazenamento local da UI
   até lá.

## Consequências

- Agentes (Fase 3+) recebem o mesmo perfil que a UI mostra, via
  `project.profile`, e podem usá-lo no Context Builder (Fase 7).
- O runtime tem estado mutável (projeto ativo), protegido por lock.

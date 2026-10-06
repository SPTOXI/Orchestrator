# ADR-0004 — Operações auxiliares do Tool Runtime

- **Estado:** Aceita
- **Fase:** 1

## Contexto

A seção 9 do documento mestre lista as operações do Tool Runtime. Para que a
UI e, depois, uma IA consigam operar terminais e processos de forma completa,
faltam algumas operações de consulta.

## Decisão

Adicionar ao catálogo:

| Operação | Motivo |
| -------- | ------ |
| `shell.list` | descobrir os shells disponíveis no SO (PowerShell, CMD, WSL, Bash…) — necessário para `terminal.create`/`shell.execute` escolherem o shell |
| `terminal.list` | consultar terminais abertos (análogo a `process.list`) |
| `process.read` | ler a saída de um processo gerenciado (análogo a `terminal.read`); sem ela uma IA não veria a saída de `npm run dev` |

Semânticas adotadas, explícitas e documentadas (não são restrições de política):

- **Leituras incrementais por offset**: `terminal.read`/`process.read`
  recebem `since` (offset em bytes do fluxo de saída) e retornam `next`. Os
  buffers guardam os últimos 1 MiB por terminal/processo; se `since` for
  anterior ao início do buffer, a resposta traz `truncated: true`.
- **Limites de saída de `shell.execute`**: `maxOutputBytes` (padrão 4 MiB por
  stream) com `stdoutTruncated`/`stderrTruncated` explícitos.
- **Timeout**: `shell.execute` aceita `timeoutMs`; sem valor, não há timeout.
  Estouro retorna `timedOut: true` com a saída parcial e encerra a árvore do
  processo.
- **Exit code ≠ 0** não é erro da ferramenta (`ok: true`, `exitCode: N`).
- `filesystem.move` não sobrescreve destino existente, a menos que
  `overwrite: true`; `filesystem.delete` de diretório não vazio exige
  `recursive: true`. São parâmetros da operação, controlados por quem chama.
- Caminhos relativos são resolvidos a partir do diretório base do runtime
  (home do usuário na Fase 1; raiz do projeto a partir da Fase 2). `~` é
  expandido para o home.

## Consequências

- Nenhuma dessas semânticas bloqueia operações: todas são controladas por
  parâmetros visíveis ao chamador.
- Processos encerrados permanecem em `process.list` até o fim da sessão do app
  (limitação conhecida; retenção configurável quando houver SQLite).

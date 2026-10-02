# ADR-0022 — Os dados do usuário sobrevivem às atualizações

- **Estado:** Aceita
- **Fase:** depois da 12 (pedido do usuário)

## Contexto

O usuário pediu: "quando tiver atualização não podemos perder as
informações já elaboradas na versão que será atualizada".

Tudo o que o usuário constrói fica na pasta de dados do app
(`<app-data>`: `~/.local/share/dev.orchestrator.desktop/` no Linux,
`%APPDATA%\dev.orchestrator.desktop\` no Windows,
`~/Library/Application Support/dev.orchestrator.desktop/` no macOS). Isso
inclui:

- o banco `orchestrator.db`, com o histórico, os projetos, as sessões e
  conversas, a memória, as decisões, as tasks, os agentes e os handoffs;
- os arquivos de configuração: `connections.json`, `clis.json`,
  `council.json`, `autonomy.json`, `mcp.json`, `context.json`,
  `agents.json`, `github.json`, `secrets.json` e `guidance.json`;
- as regras (`rules.md`) e as skills (`skills/`).

As chaves e os segredos ficam no cofre do sistema. Os projetos do
usuário ficam onde ele os guarda, e o Orchestrator não grava nada dentro
deles (ADR-0012).

Os instaladores (ADR-0019) não apagam essa pasta: nem o NSIS no modo de
atualização, nem o MSI, nem o `deb`/`rpm` (que não tocam a pasta do
usuário), nem o AppImage. Ainda assim, quatro caminhos podiam perder
dados:

1. **Migração do banco:** cada migração roda numa transação. Mesmo assim,
   uma versão nova com um defeito numa migração poderia estragar o banco,
   e não havia cópia de antes.
2. **Configuração que a versão nova não lê:** um arquivo inválido (ou de um
   formato que a versão não entende) fazia o app voltar ao padrão. No
   primeiro "salvar", o padrão era gravado por cima do arquivo do usuário.
   Em `connections.json`, uma conexão que não passava na validação sumia
   da lista e, no próximo salvar, do arquivo.
3. **Gravação interrompida:** alguns arquivos (`rules.md`, `SKILL.md`,
   `clis.json`, `github.json`, `secrets.json`, `updates.json`) eram
   gravados direto. Uma queda ou o fechamento do app para instalar a
   atualização no meio da gravação deixaria o arquivo pela metade.
4. **Nome do app:** a pasta de dados vem do `identifier` do Tauri, e as
   entradas do cofre vêm do nome do serviço. Trocar qualquer um faria a
   versão nova abrir como uma instalação nova, sem nada do usuário.

## Decisão

### Backups automáticos e manuais (`backups/`)

Um backup é uma pasta `<app-data>/backups/<data>-<motivo>/` com:

- uma cópia consistente do banco, feita pelo próprio SQLite com
  `VACUUM INTO` (funciona com o app em uso e inclui o que ainda está no
  WAL);
- os arquivos de configuração (`*.json`, `*.md`);
- as skills;
- um `manifest.json` (motivo, versão, esquema do banco, arquivos,
  tamanho).

A pasta só aparece quando a cópia termina. Uma cópia interrompida fica
como `.partial-…` e é removida depois. Ficam de fora o estado de uma
execução (`updates.json`, `processes.json`), as pastas temporárias das
CLIs e os dados da webview. As chaves e os segredos continuam no cofre do
sistema, que as atualizações não tocam.

| Quando | Motivo | Se o backup falhar |
| ------ | ------ | ------------------ |
| Antes de o atualizador instalar uma versão nova (depois de parar os agentes) | `update` | a atualização **não é instalada**, e o erro diz por quê |
| Ao abrir uma versão nova que não veio pelo atualizador (instalador baixado, `dpkg -i`, versão anterior) | `newVersion` | o aviso aparece; o app abre normalmente |
| Antes de migrar o banco para um esquema mais novo | `migration` | **o banco não é migrado nem aberto**; o app usa um banco em memória e avisa |
| Quando o usuário pede | `manual` | o erro aparece na tela |
| Antes de uma restauração | `beforeRestore` | **nada é restaurado** |

Os backups das duas primeiras linhas e o de migração são feitos antes de
qualquer coisa abrir os arquivos. Os 5 automáticos mais recentes são
guardados; os que o usuário faz ficam até ele apagar.

### Restaurar

Configurações → Dados e backups → Restaurar. O pedido fica em
`restore-pending.json`. Os agentes param com handoff e o app reinicia. Na
abertura seguinte, antes de qualquer coisa abrir os arquivos, o estado
atual vira um backup (`beforeRestore`), o que torna a restauração
reversível. Depois, o banco e os arquivos do backup voltam ao lugar, e o
que não existia no backup sai. Um backup de um esquema antigo é migrado
normalmente, com o seu próprio backup de migração.

### Nunca gravar por cima do que não foi lido

Quando um arquivo de configuração não pode ser usado, inteiro ou em parte
(uma conexão inválida), o app guarda uma cópia dele ao lado antes de
seguir com o padrão: `<nome>.unreadable-<data>`. Uma cópia com o mesmo
conteúdo não é repetida. O aviso diz onde a cópia está e aparece na barra
de status ("Dados: veja o aviso") e em Dados e backups. O arquivo original
fica no lugar até o usuário salvar algo ali, então voltar para a versão
anterior ainda o encontra.

### Gravação atômica

Todo arquivo de configuração é gravado num arquivo temporário na mesma
pasta e depois renomeado por cima. O arquivo é sempre o antigo inteiro ou
o novo inteiro.

### Nomes fixos

Um teste falha se o `identifier` do Tauri ou o nome do serviço do cofre
deixar de ser `dev.orchestrator.desktop`.

### Avisos

`app_info.dataNotices` lista o que a abertura fez: o backup feito, a
restauração ou os arquivos guardados. A barra de status mostra um chip
("Backup feito ao abrir", "Dados restaurados", "Dados: veja o aviso") que
abre Dados e backups. A aba Sobre diz que a atualização começa por um
backup e mostra qual foi feito.

## Consequências

- Uma atualização só é instalada depois que existe uma cópia de tudo; uma
  migração só roda depois que existe uma cópia do banco.
- Os backups ocupam espaço (o banco cresce com o histórico). O limite de
  5 automáticos e o `VACUUM INTO` (que compacta) o mantêm contido; a tela
  mostra o tamanho de cada um.
- Restaurar exige reiniciar o app. Primeiro o backup é copiado para perto
  dos dados; se isso falhar, nada é tocado. A troca espera alguns segundos
  pela execução anterior soltar os arquivos (no Windows, um arquivo aberto
  não pode ser apagado).
- O cofre do sistema não volta numa restauração: um segredo ou uma chave
  apagados depois do backup continuam apagados, e a conexão ou o segredo
  restaurados aparecem sem valor até o usuário informar de novo.
- Dados da webview (o que fica recolhido na barra lateral, por exemplo)
  não entram nos backups; eles também não se perdem numa atualização, mas
  não voltam numa restauração.
- O banco de uma versão mais nova (voltar para uma versão anterior) não é
  aberto nem alterado. O app usa um banco em memória e avisa; os backups
  daquela versão continuam disponíveis.

### Fica para depois

- Exportar e importar um backup em outro computador.
- Backups agendados (hoje: atualização, versão nova, migração e manual).
- Escolher a pasta dos backups.

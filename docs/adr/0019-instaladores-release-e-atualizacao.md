# ADR-0019 — Instaladores, release e atualização automática

- **Estado:** Aceita
- **Fase:** 12

## Contexto

As Fases 0–11 do documento mestre estão concluídas, mas o Orchestrator só
roda a partir do código (`pnpm dev`) ou de um binário de release copiado à
mão. O usuário escolheu, como Fase 12, transformar o app em produto
instalável: instaladores para Windows, macOS e Linux, um processo de
release reproduzível e atualização automática.

O que já existe:

- `tauri.conf.json` com `bundle.active` e `targets: "all"`, ícones e
  categoria, nunca exercitados (os relatórios usam `--no-bundle`);
- a versão `0.1.0` repetida em `Cargo.toml` (workspace), nos dois
  `package.json` e no `tauri.conf.json`, sem nada que as mantenha iguais;
- a CI (`ci.yml`) roda lint e testes nos três sistemas, sem gerar pacotes;
- a webview só tem `core:default` e os comandos do Orchestrator: nenhum
  plugin com acesso ao sistema é exposto a ela (ADR-0001, `ipc.md`).

Os princípios que valem aqui:

- **o usuário decide**: o app nunca se atualiza sozinho; ele avisa e o
  usuário instala;
- **nenhum segredo no repositório**: chaves privadas de assinatura vivem
  nos segredos do GitHub, nunca em arquivo, log ou UI;
- **a webview continua sem acesso direto**: atualizar é um comando do
  Orchestrator, como todo o resto.

## Decisão

### 1. Pacotes por sistema

| Sistema | Pacotes | Observação |
| ------- | ------- | ---------- |
| Windows | `nsis` (instalador `.exe`, por usuário, sem pedir administrador) e `msi` (para instalação gerenciada) | idioma do instalador: português do Brasil e inglês; WebView2 baixado pelo instalador quando falta |
| macOS | `app` e `dmg`, **universal** (Apple Silicon e Intel num binário só) | sistema mínimo 11 (Big Sur) |
| Linux | `deb`, `rpm` e `AppImage` | o `deb` e o `rpm` recomendam `git`; o AppImage leva as bibliotecas |

Metadados no `tauri.conf.json`: nome, editor (`publisher`), copyright,
descrição curta e longa, página do projeto. Sem licença: o repositório
ainda não tem uma, e a escolha é do usuário.

### 2. Uma versão só

A versão do produto mora no `package.json` da raiz. O script
`scripts/version.mjs`:

- `node scripts/version.mjs 0.2.0` grava a versão no `package.json` da
  raiz, no do desktop, no `Cargo.toml` do workspace e no
  `tauri.conf.json` (e o `Cargo.lock` acompanha no próximo build);
- `node scripts/version.mjs --check [tag]` falha se alguma diverge — ou se
  diverge da tag `vX.Y.Z` do release.

A CI roda o `--check` em todo push.

### 3. Release pelo GitHub Actions

Workflow novo `release.yml`:

- **tag `v*`** (ou execução manual): confere a versão contra a tag, gera
  os pacotes nos três sistemas e cria um **release em rascunho** no GitHub
  com os arquivos e o manifesto de atualização (`latest.json`). Quem
  publica o release é o usuário, na página do GitHub;
- **pull request que mexe no empacotamento** (configuração do Tauri,
  workflow, script de versão): gera os pacotes nos três sistemas e os
  guarda como artefatos do workflow, **sem release** — para o
  empacotamento não quebrar escondido.

Assinatura:

- **atualizações** (obrigatória para o updater): par de chaves do Tauri
  (`tauri signer generate`). A chave privada e a senha vão nos segredos
  `TAURI_SIGNING_PRIVATE_KEY` e `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`; a
  pública, na variável `UPDATER_PUBKEY`. Sem a chave privada, o workflow
  gera os pacotes sem os artefatos de atualização e avisa;
- **código no macOS** (opcional): certificado Developer ID e notarização
  pelos segredos `APPLE_*` que o `tauri-action` lê. Sem eles, o app sai sem
  assinatura e o macOS pede confirmação na primeira abertura;
- **código no Windows** (opcional, fora desta fase): sem certificado, o
  SmartScreen avisa na primeira execução. A documentação explica o
  caminho (`bundle.windows.signCommand`).

### 4. Atualização automática

Com o `tauri-plugin-updater` (2.x), **só do lado Rust**:

- o plugin é registrado com a chave pública e o endereço do manifesto
  embutidos na compilação (`ORCHESTRATOR_UPDATER_PUBKEY` e
  `ORCHESTRATOR_UPDATER_ENDPOINT`, que o workflow de release preenche com
  `releases/latest/download/latest.json` do próprio repositório). Um build
  sem a chave (desenvolvimento, build local) **não procura atualizações** e
  a tela diz isso;
- a webview **não** ganha as permissões do plugin. Comandos do
  Orchestrator: `update_status`, `update_check`, `update_install`,
  `update_settings_save`, e o progresso do download chega por evento;
- a atualização baixada tem a **assinatura conferida** antes de instalar
  (feito pelo plugin; uma assinatura errada é recusada com o motivo);
- **instalar é sempre do usuário**: "Baixar e instalar" pede confirmação,
  diz quantos agentes estão rodando e os para com handoff (como "Parar
  todos") antes de instalar; depois, "Reiniciar agora". No Windows, o
  instalador fecha o app e o reabre;
- **procurar sozinho** (ligado por padrão, desligável): 15 segundos depois
  de abrir e a cada 6 horas. Só avisa — um chip na barra de status e a
  aba "Sobre e atualizações". A consulta manda ao servidor apenas a
  versão, o sistema e a arquitetura;
- configuração em `<app-data>/updates.json`
  (`{ "autoCheck": true, "lastVersion": "0.1.0" }`).

### 5. Histórico

Evento novo **`APP_UPDATED`** (`from`, `to`, `via`): gravado ao abrir,
quando a versão mudou desde a última execução — por qualquer caminho
(`via: "updater"` quando foi o updater do app, `"installer"` quando o
usuário rodou um instalador). Nenhuma migração: o banco continua no
esquema 4.

### 6. Interface

- **Aba "Sobre e atualizações"**: versão, sistema, arquitetura, tipo de
  instalação, pastas de dados, estado do updater, "Procurar atualizações",
  notas da versão nova, progresso do download, "Reiniciar agora", opção de
  procurar sozinho.
- **Barra de status**: a versão abre a aba; um chip "Atualização x.y.z"
  aparece quando há uma.

## Consequências

- O app passa a ser instalado e atualizado como um produto; um release é
  "mudar a versão, criar a tag, revisar o rascunho e publicar".
- O usuário precisa criar o par de chaves e os segredos uma vez; sem isso,
  os instaladores saem, mas o updater fica desligado nos builds.
- Instaladores sem assinatura de código mostram avisos do sistema
  (SmartScreen, Gatekeeper) até o usuário configurar certificados.
- Uma atualização com `deb`/`rpm` instala pelo gerenciador de pacotes do
  sistema e pode pedir a senha de administrador.
- A webview continua sem acesso a plugins do sistema.

**Fica para depois**

- Canais (estável/beta) e atualização em segundo plano com instalação ao
  fechar.
- Assinatura de código no Windows e publicação em lojas (Microsoft Store,
  Mac App Store, Flathub, Homebrew, winget).
- ARM no Windows e no Linux.

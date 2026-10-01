# Instalação, release e atualizações

Referência da Fase 12. A decisão está na
[ADR-0019](./adr/0019-instaladores-release-e-atualizacao.md).

## Instalar

Os instaladores de cada versão ficam na página de **Releases** do
repositório no GitHub.

| Sistema | Arquivo | Como instalar |
| ------- | ------- | ------------- |
| Windows 10/11 | `Orchestrator_X.Y.Z_x64-setup.exe` | executar; instala só para o seu usuário, sem pedir administrador |
| Windows (gerenciado) | `Orchestrator_X.Y.Z_x64_en-US.msi` | `msiexec /i Orchestrator_….msi` ou pela ferramenta de distribuição da empresa |
| macOS 11+ (Apple Silicon e Intel) | `Orchestrator_X.Y.Z_universal.dmg` | abrir e arrastar para Aplicativos |
| Debian, Ubuntu e derivados | `Orchestrator_X.Y.Z_amd64.deb` | `sudo apt install ./Orchestrator_X.Y.Z_amd64.deb` |
| Fedora, openSUSE e derivados | `Orchestrator-X.Y.Z-1.x86_64.rpm` | `sudo dnf install ./Orchestrator-X.Y.Z-1.x86_64.rpm` |
| Qualquer Linux | `Orchestrator_X.Y.Z_amd64.AppImage` | `chmod +x` e executar |

O Orchestrator usa o `git` do sistema (painel GIT, contexto, agentes): os
pacotes `deb` e `rpm` o recomendam; no Windows e no macOS, instale-o à
parte.

**Instaladores sem assinatura de código.** Enquanto o projeto não tiver
certificados:

- **Windows:** o SmartScreen avisa na primeira execução — "Mais
  informações" → "Executar assim mesmo";
- **macOS:** o Gatekeeper bloqueia a primeira abertura — clique com o
  botão direito no app → "Abrir" (ou
  `xattr -dr com.apple.quarantine /Applications/Orchestrator.app`).

**Dados.** Desinstalar não apaga o banco, as conexões e as configurações,
que ficam em:

- Linux: `~/.local/share/dev.orchestrator.desktop/`
- Windows: `%APPDATA%\dev.orchestrator.desktop\`
- macOS: `~/Library/Application Support/dev.orchestrator.desktop/`

As chaves de API e o token do GitHub ficam no cofre do sistema.

## Atualizações

- **O app procura, você instala.** Com "Procurar sozinho" ligado (o
  padrão), o Orchestrator consulta o manifesto do último release 15
  segundos depois de abrir e a cada 6 horas. Quando há versão nova, a barra
  de status mostra "⬆ Atualização X.Y.Z"; nada é baixado sem você.
- **A consulta** manda ao GitHub só a versão, o sistema e a arquitetura.
- **Instalar** (aba "Sobre e atualizações" → "Baixar e instalar…"):
  1. os agentes em execução param, com handoff, como em "Parar todos";
  2. o pacote é baixado e a **assinatura é conferida** com a chave pública
     embutida no app — uma assinatura errada é recusada e nada é
     instalado;
  3. a instalação usa o mesmo tipo de pacote que você instalou: o
     instalador `.exe`/`.msi` no Windows (o app fecha e o instalador abre
     a versão nova), o `.app` no macOS, o AppImage, ou o `deb`/`rpm` pelo
     gerenciador de pacotes (que pode pedir a senha de administrador);
  4. "Reiniciar agora" abre a versão nova. As sessões voltam encerradas,
     com a conversa ("Retomar" continua).
- **O histórico** registra `APP_UPDATED` (de qual para qual versão, e se
  foi pelo app ou por um instalador) na primeira abertura da versão nova.
- **Builds locais e de desenvolvimento** não procuram atualizações: só os
  instaladores do release trazem a chave pública. A aba diz isso.
- `<app-data>/updates.json`:
  `{ "autoCheck": true, "lastVersion": "0.2.0", "pending": null }`.

## Fazer um release

### Uma vez: as chaves de atualização

1. Gere o par de chaves (guarde a senha):

   ```bash
   pnpm tauri signer generate -w ~/.tauri/orchestrator.key
   ```

2. No GitHub, em *Settings → Secrets and variables → Actions*:
   - segredo `TAURI_SIGNING_PRIVATE_KEY`: o conteúdo de
     `~/.tauri/orchestrator.key`;
   - segredo `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: a senha;
   - variável `UPDATER_PUBKEY`: o conteúdo de
     `~/.tauri/orchestrator.key.pub`.

**Guarde a chave privada fora do repositório e faça backup.** Sem ela, as
cópias já instaladas não aceitam versões novas e cada pessoa precisa
reinstalar à mão. Sem os segredos, o workflow ainda gera os instaladores,
mas sem atualização automática (e avisa).

### A cada versão

```bash
pnpm version:set 0.2.0          # package.json, Cargo.toml e Cargo.lock
pnpm version:check              # tudo igual?
git commit -am "Versão 0.2.0"
git tag v0.2.0
git push origin HEAD v0.2.0
```

O workflow **Release** (`.github/workflows/release.yml`):

1. confere que a versão do código é a da tag;
2. gera os instaladores no Windows, no macOS (universal) e no Linux
   (Ubuntu 22.04, para rodar em sistemas mais novos também);
3. cria um **release em rascunho** com os instaladores, as assinaturas e o
   `latest.json`.

Revise o rascunho (notas da versão, arquivos) e clique em **Publish
release**. A partir daí, `releases/latest/download/latest.json` aponta
para a versão nova e as cópias instaladas passam a oferecê-la.

O mesmo workflow roda, sem criar release, quando um push mexe no
empacotamento (configuração do Tauri, ícones, workflow, script de versão):
os instaladores ficam como artefatos do workflow por 7 dias. Também dá
para rodá-lo à mão em *Actions → Release → Run workflow*.

### Assinatura de código (opcional)

- **macOS:** segredos `APPLE_CERTIFICATE` (o `.p12` em base64),
  `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` e, para
  notarizar, `APPLE_ID`, `APPLE_PASSWORD` (senha de app) e
  `APPLE_TEAM_ID`. Sem eles, o app sai sem assinatura.
- **Windows:** ainda não configurado. O caminho é
  `bundle.windows.signCommand` no `tauri.conf.json` (por exemplo com o
  Azure Trusted Signing) e os segredos do serviço escolhido.

## Build local

```bash
pnpm build                          # instaladores do sistema atual, sem updater
pnpm tauri build --no-bundle        # só o executável
```

Os arquivos ficam em `target/release/bundle/`. No Linux, o AppImage baixa
as ferramentas `linuxdeploy` na primeira vez.

## Problemas comuns

| Sintoma | Causa e saída |
| ------- | ------------- |
| "Este build não procura atualizações" | build local ou de desenvolvimento, sem a chave pública. Instale pelo release. |
| "Não foi possível procurar atualizações: …" | sem rede, ou o release ainda não foi publicado (rascunhos não contam). |
| "A atualização não foi instalada: … signature …" | o pacote não foi assinado pela chave que o app conhece: release gerado com outra chave. |
| Linux `deb`/`rpm`: pede senha e falha | a instalação precisa de administrador (`pkexec` ou `sudo`). Instale o pacote novo pelo terminal. |
| Workflow: "falta a variável UPDATER_PUBKEY" | há a chave privada nos segredos, mas não a pública correspondente na variável. |

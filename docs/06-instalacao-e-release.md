# Instalar e publicar o Bookferry

## Instalar (usuário)

| Sistema | Arquivo | Como |
|---|---|---|
| Ubuntu, Debian, Mint, Pop!_OS (22.04+ / 12+) | `Bookferry_X.Y.Z_amd64.deb` | `sudo apt install ./Bookferry_X.Y.Z_amd64.deb` |
| Fedora, openSUSE | `Bookferry-X.Y.Z-1.x86_64.rpm` | `sudo dnf install ./Bookferry-X.Y.Z-1.x86_64.rpm` |
| Qualquer outra distro | `Bookferry_X.Y.Z_amd64.AppImage` | `chmod +x` no arquivo e executar; precisa de `libwebkit2gtk-4.1` na distro |
| NixOS | `nixos/bookferry.nix` | ver `nixos/` |
| Windows 10/11 | `Bookferry_X.Y.Z_x64-setup.exe` | executar; instala só para o usuário atual |
| macOS (Apple Silicon) | `Bookferry_X.Y.Z_aarch64.dmg` | abrir e arrastar para Aplicativos |
| macOS (Intel) | `Bookferry_X.Y.Z_x64.dmg` | idem |
| Android (arm64) | `Bookferry_X.Y.Z_android-arm64.apk` | permitir "instalar apps desconhecidos" e abrir o arquivo |

Confira o download com `sha256sum -c SHA256SUMS` (Linux/macOS) ou `Get-FileHash` (Windows).

### Avisos esperados enquanto os instaladores não forem assinados

- **Windows:** SmartScreen mostra "O Windows protegeu o computador". Clique em *Mais informações* e *Executar assim mesmo*.
- **macOS:** o Gatekeeper bloqueia o app. Clique com o botão direito no app, *Abrir*; ou, se aparecer "danificado", `xattr -dr com.apple.quarantine /Applications/Bookferry.app`.
- **Android:** o APK é assinado (a chave está em `docs/` abaixo), mas fora da Play Store o sistema avisa sobre fonte desconhecida.

### Firewall

O PC escuta na porta **47600** (TCP) e anuncia o serviço por mDNS. Se o celular não achar o PC:

- Ubuntu/Debian com `ufw`: `sudo ufw allow 47600/tcp`
- Fedora com `firewalld`: `sudo firewall-cmd --permanent --add-port=47600/tcp && sudo firewall-cmd --reload`
- Windows: aceitar o aviso do Firewall no primeiro uso (rede privada).
- macOS: aceitar "Permitir conexões de entrada" e a permissão de "Rede local".
- Roteadores com "isolamento de clientes" (Wi-Fi de visitantes) impedem o pareamento, qualquer que seja o instalador.

## Publicar uma versão (mantenedor)

1. Alterar a versão em `app/src-tauri/tauri.conf.json` (e `Cargo.toml` do workspace, para ficarem iguais).
2. Commit em `main`; `git tag vX.Y.Z && git push origin vX.Y.Z`.
3. O workflow **Release** (`.github/workflows/release.yml`) falha se a tag diferir da versão do app. Ele gera
   `.deb`, `.rpm`, AppImage, `.exe`, dois `.dmg`, o APK assinado e `SHA256SUMS`, e abre um **release em rascunho**.
4. Baixar e abrir pelo menos um instalador por sistema antes de publicar o rascunho.

`versionCode` do Android vem de `app/src-tauri/gen/android/app/tauri.properties` (calculado a partir da versão do app).

### O que o CI valida a cada push

`.github/workflows/ci.yml`: testes JS, `cargo fmt`, `clippy -D warnings` e `cargo test` em Linux e Windows. O macOS roda
sob demanda (Actions > CI > Run workflow > macos).

### Segredos (Settings > Secrets > Actions)

`ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_PASSWORD`, `ANDROID_KEY_ALIAS`.

### Chave de assinatura do Android — não perder

Original em `~/.config/bookferry/release-keystore/` (`bookferry-release.jks` + `keystore.properties`), fora do repositório.
**Sem esta chave não é possível atualizar um app já instalado**: faça backup em pelo menos dois lugares fora deste computador
(gerenciador de senhas, pendrive). Impressão digital SHA-256 do certificado:
`61:A3:44:1C:32:64:27:E7:79:06:35:A1:C7:CE:12:96:9B:3F:33:1C:06:FC:D9:B3:72:AA:2E:32:11:2C:E4:21`

Build local assinado: copiar `keystore.properties` para `app/src-tauri/gen/android/` (está no `.gitignore`) e
`cargo tauri android build --apk --target aarch64`. Sem o arquivo, o release sai sem assinatura.

## O que ainda não existe

Assinatura de código no Windows e macOS (custa dinheiro; ver `05-plano-entrega.md`), atualização automática, Play Store,
Flatpak/AUR, Android x86_64/armv7.

## O que foi verificado (14/11/2026)

- Linux: `.deb` instalado e aberto sob Xvfb em Ubuntu 22.04, 24.04 e Debian 12; `.rpm` em Fedora; AppImage em Ubuntu 22.04.
- Windows e macOS (arm64): instalar/abrir sem erro e processo vivo após 15 s, no runner do GitHub. **Ninguém abriu ainda a janela
  em uma máquina real**: o pareamento com o celular nesses sistemas continua sem teste.
- Android: APK de release assinado, verificado com `apksigner`; ainda não instalado no S23.

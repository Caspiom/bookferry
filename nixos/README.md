# Módulo NixOS de desenvolvimento

`bookferry-dev.nix` é a cópia versionada de `/etc/nixos/modules/bookferry-dev.nix`: toolchain
Rust (alvos Android), Android SDK/NDK, webkit2gtk/GTK, cargo-tauri, xvfb e o comando
`bookferry-run`.

Num PC novo: copie para `/etc/nixos/modules/`, importe em `configuration.nix`
(`imports = [ ./modules/bookferry-dev.nix ];`) e rode `sudo nixos-rebuild switch`.

## Abrir o app desktop

    bookferry-run ./target/debug/bookferry

Fora do `bookferry-run` (ou do `nix develop` em `spike/`) o webview não acha os schemas do
GSettings e a janela sai com texto minúsculo e layout colapsado. O E2E (`app/tests/e2e`) roda
sob Xvfb e não reproduz esse defeito de ambiente; ele só pega CSS quebrado.

## Instalar o app no launcher (plano; ainda não aplicado)

`bookferry.nix` empacota o desktop (release com o frontend embutido, `.desktop` e ícone) e
`bookferry-app.nix` o instala no sistema. Quando o design estiver pronto:

1. Rodar `cargo test --workspace`, `node --test app/tests/logic.test.mjs` e o E2E, e **commitar**
   (o pacote compila o que está na checkout, filtrado por `cleanSourceWith`).
2. Em `/etc/nixos/configuration.nix`, em `imports`, acrescentar
   `/home/gappy/Documents/projetos/bookshelf/nixos/bookferry-app.nix`.
3. `sudo nixos-rebuild test` (compila em release, alguns minutos), conferir que "Bookferry"
   aparece no launcher e que a janela sai normal, e então `sudo nixos-rebuild switch`.
4. Para atualizar depois de mudar o app: commit + `sudo nixos-rebuild switch`. O dia a dia de
   desenvolvimento continua com `bookferry-run ./target/debug/bookferry`.

O app instalado e o de desenvolvimento usam a mesma pasta de dados
(`~/.local/share/dev.gaspari.bookferry`, pelo `identifier`), então biblioteca, pareamentos e capas
são compartilhados. Feche um antes de abrir o outro: os dois querem a porta 47600.

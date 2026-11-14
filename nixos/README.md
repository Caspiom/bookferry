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

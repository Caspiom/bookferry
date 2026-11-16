# Pacote Nix do Bookferry desktop (Tauri v2): binario release com o frontend embutido,
# entrada no launcher (.desktop + icone) e ambiente do webkit2gtk via wrapGAppsHook3.
# Uso: pkgs.callPackage ./bookferry.nix { }   (ver nixos/bookferry-app.nix)
{
  lib,
  rustPlatform,
  cargo-tauri,
  pkg-config,
  wrapGAppsHook3,
  openssl,
  dbus,
  webkitgtk_4_1,
  gtk3,
  libsoup_3,
  glib-networking,
  gsettings-desktop-schemas,
  librsvg,
  libayatana-appindicator,
}:

let
  repoRoot = ../.;
  # So o necessario para compilar: sem target/, .git, docs, spike (flake de teste), nem o
  # projeto Android gerado (app/src-tauri/gen).
  excluded = [
    "target"
    ".git"
    ".cache"
    "docs"
    "spike"
    "gen"
    "screenshots"
    "marca" # kit de marca gerado (rasters); o app so usa app/src e app/src-tauri/icons
    "node_modules"
  ];
in
rustPlatform.buildRustPackage {
  pname = "bookferry";
  version = "0.1.0";

  src = lib.cleanSourceWith {
    src = repoRoot;
    filter = path: type: !(builtins.elem (baseNameOf path) excluded);
  };

  cargoLock.lockFile = ../Cargo.lock;

  # cargo-tauri.hook roda `cargo tauri build` (liga `custom-protocol`: sem isso o release tenta
  # abrir um devUrl e a janela sai em branco) e instala o .desktop e os icones.
  buildAndTestSubdir = "app/src-tauri";
  nativeBuildInputs = [
    cargo-tauri.hook
    pkg-config
    wrapGAppsHook3
  ];
  buildInputs = [
    openssl
    dbus # libdbus-sys (via tao, janela/tray)
    webkitgtk_4_1
    gtk3
    libsoup_3
    glib-networking # TLS/HTTPS dentro do webview
    gsettings-desktop-schemas # sem os schemas o webview sai com layout colapsado
    librsvg
    libayatana-appindicator
  ];

  # Testes (cargo + node + E2E sob xvfb) rodam no repositorio, nao dentro do build do pacote.
  doCheck = false;

  preFixup = ''
    gappsWrapperArgs+=(--set-default WEBKIT_DISABLE_DMABUF_RENDERER 1)
    # O icone da bandeja carrega a libayatana-appindicator por dlopen: estar em buildInputs nao
    # basta, ela precisa estar no LD_LIBRARY_PATH do binario embrulhado.
    gappsWrapperArgs+=(--prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath [ libayatana-appindicator ]})
  '';

  meta = {
    description = "Leitor de livros e quadrinhos com sincronizacao de posicao na rede local";
    mainProgram = "bookferry";
    platforms = lib.platforms.linux;
  };
}

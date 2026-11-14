{ pkgs, lib, ... }:

let
  # rust-overlay fixado na mesma revisao do spike/flake.lock (reprodutivel num PC novo).
  rustOverlay = import (builtins.fetchTarball {
    url = "https://github.com/oxalica/rust-overlay/archive/84edd48f141b0c8910dffdd0296ddd577e829132.tar.gz";
    sha256 = "sha256-oWtItM1eQqkp9OJXq2/Zo10Y9Vu9PDE6O8JRjj4+EkE=";
  });
  devPkgs = import pkgs.path {
    system = "x86_64-linux";
    overlays = [ rustOverlay ];
    config = {
      allowUnfree = true;
      android_sdk.accept_license = true;
    };
  };

  rustToolchain = devPkgs.rust-bin.stable.latest.default.override {
    extensions = [ "rust-src" "rust-analyzer" "clippy" "rustfmt" ];
    targets = [
      "aarch64-linux-android"
      "armv7-linux-androideabi"
      "x86_64-linux-android"
      "i686-linux-android"
    ];
  };

  buildToolsVersion = "37.0.0";
  ndkVersion = "27.3.13750724";
  androidSdk = (devPkgs.androidenv.composeAndroidPackages {
    platformVersions = [ "37.0" "36" ];
    buildToolsVersions = [ buildToolsVersion "36.0.0" ];
    includeNDK = true;
    ndkVersions = [ ndkVersion ];
    includeCmake = false;
    includeEmulator = false;
    includeSystemImages = false;
  }).androidsdk;
  androidHome = "${androidSdk}/libexec/android-sdk";

  # Abre o app desktop (ou qualquer binario Tauri) com o ambiente que o webkit2gtk precisa no
  # NixOS: schemas do GSettings (sem eles o webview sai com texto minusculo e layout colapsado),
  # modulos GIO (TLS/HTTPS) e libs carregadas por dlopen (GL, appindicator).
  # Uso: bookferry-run ./target/debug/bookferry   |   bookferry-run cargo run -p bookferry-app
  bookferryRun = pkgs.writeShellScriptBin "bookferry-run" ''
    if [ "$#" -eq 0 ]; then
      echo "uso: bookferry-run <comando> [args...]" >&2
      exit 2
    fi
    export XDG_DATA_DIRS="${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}''${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}"
    export GIO_EXTRA_MODULES="${pkgs.glib-networking}/lib/gio/modules''${GIO_EXTRA_MODULES:+:$GIO_EXTRA_MODULES}"
    export LD_LIBRARY_PATH="${lib.makeLibraryPath [ pkgs.libayatana-appindicator ]}:/run/opengl-driver/lib''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    export WEBKIT_DISABLE_DMABUF_RENDERER="''${WEBKIT_DISABLE_DMABUF_RENDERER:-1}"
    exec "$@"
  '';
in
{
  environment.systemPackages = with pkgs; [
    bookferryRun
    rustToolchain
    cargo-tauri
    gradle
    jdk17
    androidSdk
    xvfb-run
    imagemagick
    pkg-config
    # libs nativas do Tauri v2 (webkit2gtk-4.1 + GTK3 + libsoup3)
    webkitgtk_4_1 gtk3 libsoup_3 glib cairo pango gdk-pixbuf atk librsvg
    openssl libayatana-appindicator xdotool glib-networking
  ];

  environment.sessionVariables = {
    ANDROID_HOME = androidHome;
    ANDROID_SDK_ROOT = androidHome;
    NDK_HOME = "${androidHome}/ndk/${ndkVersion}";
    ANDROID_NDK_ROOT = "${androidHome}/ndk/${ndkVersion}";
    GRADLE_OPTS = "-Dorg.gradle.project.android.aapt2FromMavenOverride=${androidHome}/build-tools/${buildToolsVersion}/aapt2";
    WEBKIT_DISABLE_DMABUF_RENDERER = "1";
  };
}

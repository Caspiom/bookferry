{
  description = "Bookferry spike - Tauri v2 (Linux desktop + Android) dev shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
        # Aceitos SOMENTE neste flake (nao altera config global do usuario).
        config = {
          allowUnfree = true;
          android_sdk.accept_license = true;
        };
      };

      lib = pkgs.lib;

      rustToolchain = pkgs.rust-bin.stable.latest.default.override {
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

      androidComposition = pkgs.androidenv.composeAndroidPackages {
        platformVersions = [ "37.0" "36" ];
        buildToolsVersions = [ buildToolsVersion "36.0.0" ];
        includeNDK = true;
        ndkVersions = [ ndkVersion ];
        includeCmake = false;
        includeEmulator = false;
        includeSystemImages = false;
      };
      androidSdk = androidComposition.androidsdk;
      androidHome = "${androidSdk}/libexec/android-sdk";

      jdk = pkgs.jdk17;

      # Bibliotecas nativas do Tauri v2 no Linux (webkit2gtk-4.1 + GTK3 + libsoup3)
      tauriLibs = with pkgs; [
        webkitgtk_4_1
        gtk3
        libsoup_3
        glib
        cairo
        pango
        gdk-pixbuf
        atk
        librsvg
        openssl
        libayatana-appindicator
        xdotool
        glib-networking
        gsettings-desktop-schemas
        libGL
        libxkbcommon
        wayland
      ];
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        name = "bookferry-spike";

        nativeBuildInputs = with pkgs; [
          pkg-config
          gobject-introspection
        ];

        buildInputs = tauriLibs ++ (with pkgs; [
          rustToolchain
          cargo-tauri
          nodejs_22
          jdk
          androidSdk
          gradle
          curl
          wget
          file
          unzip
          xvfb-run
        ]);

        ANDROID_HOME = androidHome;
        ANDROID_SDK_ROOT = androidHome;
        NDK_HOME = "${androidHome}/ndk/${ndkVersion}";
        ANDROID_NDK_ROOT = "${androidHome}/ndk/${ndkVersion}";
        JAVA_HOME = "${jdk}/lib/openjdk";

        shellHook = ''
          # Raiz do spike = diretorio deste flake (onde `nix develop` foi chamado)
          # (sobe diretorios ate achar flake.nix, para funcionar de dentro de app/ tambem)
          if [ -z "''${SPIKE_ROOT:-}" ]; then
            _d="$PWD"
            while [ "$_d" != "/" ] && [ ! -f "$_d/flake.nix" ]; do _d="$(dirname "$_d")"; done
            export SPIKE_ROOT="$_d"
          fi
          export SPIKE_CACHE="$SPIKE_ROOT/.cache"
          mkdir -p "$SPIKE_CACHE"/{gradle,android-user,cargo}

          # Tudo que normalmente iria para ~ fica dentro de spike/.cache
          export GRADLE_USER_HOME="$SPIKE_CACHE/gradle"
          export ANDROID_USER_HOME="$SPIKE_CACHE/android-user"
          export CARGO_HOME="$SPIKE_CACHE/cargo"
          export npm_config_cache="$SPIKE_CACHE/npm"
          export XDG_CACHE_HOME="$SPIKE_CACHE/xdg"

          # SDK do Nix e somente leitura: o aapt2 baixado do Maven nao roda no NixOS
          # (binario dinamico generico); usamos o aapt2 do build-tools do Nix.
          export GRADLE_OPTS="-Dorg.gradle.project.android.aapt2FromMavenOverride=$ANDROID_HOME/build-tools/${buildToolsVersion}/aapt2"

          export PATH="$ANDROID_HOME/platform-tools:$ANDROID_HOME/cmdline-tools/latest/bin:$PATH"

          # WebKitGTK sob Wayland + NVIDIA hibrida: DMABUF renderer costuma dar tela branca.
          export WEBKIT_DISABLE_DMABUF_RENDERER="''${WEBKIT_DISABLE_DMABUF_RENDERER:-1}"

          # libs carregadas via dlopen (GL, appindicator) + drivers GPU do host NixOS
          export LD_LIBRARY_PATH="${lib.makeLibraryPath tauriLibs}:/run/opengl-driver/lib''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
          export XDG_DATA_DIRS="${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}:''${XDG_DATA_DIRS:-}"
          export GIO_EXTRA_MODULES="${pkgs.glib-networking}/lib/gio/modules"

          echo "bookferry-spike devShell"
          echo "  ANDROID_HOME=$ANDROID_HOME"
          echo "  NDK_HOME=$NDK_HOME"
          echo "  JAVA_HOME=$JAVA_HOME"
        '';
      };
    };
}

# Spike 01 — Ambiente: Tauri v2 em Linux e Android dentro de um dev shell Nix

Data: 13/11/2026 · Máquina: NixOS 26.5, Nix 2.34.9, fish, niri (Wayland), GPU AMD+NVIDIA · Janela total: ~13 min de relógio (13:56 → 14:08), a maior parte foi download/compilação.

## Resumo

| Item | Resultado |
|---|---|
| devShell Nix reproduzível (`spike/flake.nix` + `flake.lock`) | **OK** |
| (a) Linux: `cargo tauri build --no-bundle` | **OK** (binário gerado) |
| (a) Janela abre sob Wayland/NVIDIA | **NÃO VERIFICADO** (sessão do agente sem display; ver abaixo) |
| (b) Android: `cargo tauri android init` + `build --debug` (aarch64) | **OK** — APK gerado |
| (b) APK roda em dispositivo/emulador | **NÃO VERIFICADO** (nenhum dispositivo; nenhum emulador instalado) |

## Como reproduzir

Tudo a partir da raiz do repositório (`bookshelf/`). Nenhum `sudo`, nada fora do nix store e de `spike/`.

```fish
cd spike
git add -N flake.nix flake.lock          # flakes dentro de repo git só enxergam arquivos rastreados
nix develop                              # 1ª vez: ~2 min + downloads (ver tamanhos)
# dentro do shell:
cd app/src-tauri && cargo tauri build --no-bundle      # (a) Linux
cd .. && cargo tauri android init                       # (b) só na 1ª vez (gen/android já existe agora)
cargo tauri android build --debug --target aarch64 --apk
```

Atalhos não interativos (funcionam de qualquer diretório): [`spike/scripts/build-linux.sh`](../../spike/scripts/build-linux.sh) e [`spike/scripts/build-android.sh`](../../spike/scripts/build-android.sh).

Resultados:
- Linux: `spike/app/src-tauri/target/release/bookferry-spike` (11 MB, release)
- APK: `spike/app/src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk` — **133.843.527 bytes (~128 MiB)**, sha256 `298fdf00…efbcf6`

### O que o flake faz (`spike/flake.nix`)
- `nixpkgs-unstable` (rev `2074cc67`, 2026-11-12) + `rust-overlay` (rev `84edd48f`, 2026-11-13), travados no `flake.lock`.
- Rust estável via rust-overlay com os 4 targets Android (`aarch64-linux-android`, `armv7-linux-androideabi`, `x86_64-linux-android`, `i686-linux-android`).
- `cargo-tauri` do nixpkgs (nome confirmado com `nix eval`), `nodejs_22`, `jdk17`, `gradle` (só para o shell; o build usa o wrapper do projeto), `pkg-config`, `openssl`, `webkitgtk_4_1`, `gtk3`, `libsoup_3`, `librsvg`, `libayatana-appindicator`, `xdotool`, `glib-networking`, etc.
- Android SDK via `androidenv.composeAndroidPackages`: platforms `37.0` e `36`, build-tools `37.0.0` e `36.0.0`, NDK `27.3.13750724`, platform-tools. `android_sdk.accept_license = true` e `allowUnfree = true` **apenas** na instância local de nixpkgs do flake.
- Variáveis: `ANDROID_HOME`, `ANDROID_SDK_ROOT`, `NDK_HOME`, `ANDROID_NDK_ROOT`, `JAVA_HOME`; no `shellHook`: `GRADLE_USER_HOME`, `ANDROID_USER_HOME`, `CARGO_HOME`, `npm_config_cache`, `XDG_CACHE_HOME` apontando para `spike/.cache/` (ignorado por `spike/.gitignore`); `WEBKIT_DISABLE_DMABUF_RENDERER=1`; `LD_LIBRARY_PATH` (libs GTK/GL + `/run/opengl-driver/lib`), `XDG_DATA_DIRS` e `GIO_EXTRA_MODULES` para schemas/TLS do WebKit.
- `GRADLE_OPTS=-Dorg.gradle.project.android.aapt2FromMavenOverride=…/build-tools/37.0.0/aapt2` (o aapt2 baixado do Maven é binário genérico e não roda no NixOS).

## Versões reais (saídas de comando)

| Componente | Versão |
|---|---|
| rustc / cargo | 1.100.0 (0ef1bc386 2026-11-09) / 1.100.0 |
| tauri-cli (`cargo-tauri`, nixpkgs) | 2.12.4 |
| crates: tauri / tauri-build / wry / tao | 2.12.5 / 2.7.4 / 0.57.2 / 0.37.2 |
| node / npm | 22.23.5 / 10.9.9 (node nem é necessário: frontend é HTML/JS estático) |
| JDK | OpenJDK 17.0.21.0.3 |
| webkit2gtk-4.1 / libsoup-3.0 | 2.54.2 / 3.8.0 |
| Android platform / build-tools / NDK | android-37.0 e android-36 / 37.0.0 e 36.0.0 / 27.3.13750724 |
| Template Android gerado pelo Tauri | AGP 9.3.1, Kotlin 2.2.10, Gradle 9.6.1 (wrapper baixa de services.gradle.org), compileSdk/targetSdk 37, minSdk 24 |

APK: pacote `dev.gaspari.bookferry.spike`, versionName 0.0.1, `uses-permission INTERNET`, contém só `lib/arm64-v8a/libbookferry_spike_lib.so` (126.594.032 bytes — **debug sem strip**, por isso o APK é grande; um build release é muito menor, não medido).

## Tempos (medidos)

| Etapa | Tempo |
|---|---|
| `nix flake lock` | 14 s |
| 1º `nix develop` (download SDK/NDK/webkit/rust) | 2 min 03 s |
| `cargo tauri icon` + `cargo generate-lockfile` | 9 s |
| `cargo tauri build --no-bundle` (Linux, release, do zero) | 1 min 27 s |
| `cargo tauri android init` | 0,8 s |
| 1ª tentativa `android build` (falhou no Gradle, ver abaixo; inclui download do Gradle 9.6.1 + compilar Rust) | 2 min 12 s |
| 2ª tentativa `android build` (sucesso, Rust já compilado) | 1 min 10 s |
| Reexecução dos scripts, tudo em cache | 2,5 s (Linux) / 3,1 s (Android) |

## Tamanhos em disco (aproximados)

- Android SDK composto no nix store (platform-tools, 2 platforms, 2 build-tools, NDK): **~4,8 GB**.
- `spike/.cache/gradle` (distribuição Gradle + dependências Maven/AGP): ~1,2 GB; `spike/.cache/cargo`: ~223 MB.
- `spike/app/src-tauri/target`: ~3,9 GB (Linux release + Android debug).
- Total do spike abaixo de ~15 GB (limite do enunciado não foi atingido). Não medi o crescimento total do `/nix/store` (inclui webkitgtk, JDK, rust etc.).

## O que falhou e como foi resolvido

1. **Gradle: variáveis conflitantes do Android.**
   Erro real: `com.android.prefs.AndroidLocationsException: Several environment variables and/or system properties contain different paths to the Android Preferences folder.` (aparecia como `Could not create provider for value source AndroidLocationsBuildService.AndroidDirectoryCreator`).
   Causa: o shellHook exportava `ANDROID_USER_HOME` **e** `ANDROID_SDK_HOME` (deprecated) para `spike/.cache/android-user`, enquanto algum caminho padrão (`~/.android`) ainda era considerado. Correção: removi `ANDROID_SDK_HOME`, mantendo só `ANDROID_USER_HOME`. Build passou em seguida. (Não investiguei qual é a fonte exata do segundo caminho; só confirmei que remover a variável resolve.)
2. **Lição**: o template do Tauri 2.12 pede `compileSdk 37`; com o `androidenv` padrão eu inicialmente tinha plataforma 36. Adicionei `platformVersions = ["37.0" "36"]` e `buildTools 37.0.0` (o diretório vira `platforms/android-37.0`) *antes* da primeira build, por ter lido o template após `android init` — então não houve falha por isso, mas a versão exigida muda a cada release do Tauri.

## (a) Linux — janela sob Wayland/NVIDIA: NÃO verificado

O build compila e o binário existe. Ao tentar executá-lo no shell do agente (que não tem `WAYLAND_DISPLAY` nem `DISPLAY`) obtive:

```
thread 'main' panicked at tao-0.37.2/src/platform_impl/linux/event_loop.rs:219:53:
Failed to initialize gtk backend!: BoolError { message: "Failed to initialize GTK", ... }
```

Isso é o esperado sem display. **Não** apontei `WAYLAND_DISPLAY` para o socket do niri do usuário: o ambiente do agente não recebeu acesso à sessão gráfica e não quis contornar essa fronteira. Portanto o risco 7 (tela branca WebKitGTK/DMABUF no niri com RTX 3060) **continua aberto**. Para testar na sessão real do niri (terminal normal):

```fish
cd spike && nix develop -c bash -c 'cd app/src-tauri && ./target/release/bookferry-spike'
# se aparecer tela branca com o default (shellHook já exporta WEBKIT_DISABLE_DMABUF_RENDERER=1), compare com:
WEBKIT_DISABLE_DMABUF_RENDERER=0 nix develop -c bash -c 'cd app/src-tauri && ./target/release/bookferry-spike'
```
A tela mostra "Bookferry spike" e um botão `ping`; ao clicar deve aparecer `pong, Bookferry (rust 0.0.1 / linux)`.

## Ainda NÃO verificado

- Janela abre / não fica branca no niri + NVIDIA (acima). `WEBKIT_DISABLE_DMABUF_RENDERER=1` está no shellHook por precaução; **não sei se é necessário**.
- O comando `ping` Rust→frontend funcionando em runtime (só compila; o app nunca foi renderizado).
- APK instalando/abrindo em dispositivo ou emulador (sem dispositivo; sem `adb` conectado; emulador não incluído no SDK). O APK também não foi assinado com chave de release (apenas keystore de debug padrão do Gradle).
- Build para os outros ABIs (armv7, x86_64, i686): os targets Rust estão instalados, mas só aarch64 foi compilado.
- `cargo tauri dev` / `android dev` (hot reload), bundling (`.deb`/AppImage), build release do Android.
- Reprodutibilidade em outra máquina/cache limpo: o `flake.lock` fixa Nix/Rust, mas o Gradle/Maven baixa dependências sem trava de versão no nosso lado (Gradle wrapper 9.6.1 e AGP 9.3.1 vêm do template), e o `Cargo.lock` do app foi gerado agora e deve ser commitado.
- Riscos 1–6 e 8 do plano (SAF/fd, WebView 60 fps, CFI, mDNS/TLS, relay etc.): fora do escopo desta fase.

## Observações

- `~/.android` já existia antes (chaves do adb, de jan/2026) e não foi modificado; `~/.gradle` não foi criado. Tudo do Gradle/Cargo/npm ficou em `spike/.cache`.
- `git add -N` foi usado em `spike/flake.nix` (e o Nix marcou `flake.lock`); nada foi commitado.
- Arquivos para commit: `spike/flake.nix`, `spike/flake.lock`, `spike/.gitignore`, `spike/scripts/`, `spike/app/` (incluindo `src-tauri/Cargo.lock` e `gen/android`; `gen/android/.gradle` e `build` já são ignorados pelo `.gitignore` raiz), e este relatório.

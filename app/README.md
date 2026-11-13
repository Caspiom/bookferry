# Bookferry App (`app/`)

Aplicativo **Tauri v2** (Linux Desktop + Android `aarch64`) do projeto Bookferry, com leitor integrado para **EPUB** (`foliate-js`), **PDF** (`pdf.js` 4.10.38 via HTTP Range) e **CBZ** (imagens servidas por entrada ZIP no protocolo customizado `bookferry://`).

- **No Desktop (Linux):** embute `bookferry-server` + `bookferry-client`, pareando-se automaticamente em loopback (`127.0.0.1`), exibindo QR Code SVG + JSON copiável para parear celulares na LAN e permitindo revogar dispositivos.
- **No Android (`aarch64-linux-android`):** compila exclusivamente o cliente (`bookferry-client`), sem incluir o crate do servidor nem mDNS de servidor.

---

## Pré-requisitos de Ambiente (Nix)

Todos os comandos devem ser executados dentro do `devShell` em `spike/flake.nix` mantendo `spike/` como diretório inicial do `nix develop` (para que os caches fiquem em `spike/.cache` e não em `/.cache`):

```bash
cd spike && nix develop -c bash -c 'cd .. && <comando>'
```

---

## 1. Como Rodar no Desktop (Linux)

Para compilar e abrir o aplicativo Desktop:

```bash
cd spike && nix develop -c bash -c 'cd .. && cargo run -p bookferry-app'
```

Na primeira execução:
1. Informe o caminho da sua pasta local de livros (`.epub`, `.pdf`, `.cbz`) na tela inicial ou em **⚙ Ajustes**.
2. O servidor HTTPS local iniciará automaticamente e o computador será pareado em loopback.
3. Para parear um celular na mesma rede Wi-Fi, abra **⚙ Ajustes** para ver o QR Code SVG (com contagem regressiva de 5 minutos e botão **Gerar novo**) ou copiar o JSON de pareamento.

---

## 2. Como Rodar os Testes

### Testes Unitários e de Integração Rust + Clippy + Format

```bash
cd spike && nix develop -c bash -c 'cd .. && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace'
```

### Testes Unitários do Frontend (JS puro com `node --test`)

```bash
cd spike && nix develop -c bash -c 'cd .. && node --test app/tests/logic.test.mjs'
```

### Teste E2E Real no Linux sob `xvfb-run`

Gera fixtures sintéticas (`EPUB`, `PDF`, `CBZ`), sobe o app Desktop com servidor embutido, pareia um segundo cliente simulando celular, exercita os 3 leitores, valida sincronização SSE bidirecional, modal de retomada (`Ask`) e captura screenshots em `app/tests/screenshots/`:

```bash
cd spike && nix develop -c bash -c 'cd .. && bash app/tests/e2e/run-e2e.sh'
```

---

## 3. Como Gerar o APK Debug Android (`aarch64`)

### Verificação rápida de compilação (`cargo check` sem servidor)

```bash
cd spike && nix develop -c bash -c 'cd .. && cargo check --target aarch64-linux-android -p bookferry-app'
```

### Build completo do APK Debug `aarch64`

Caso o projeto Gradle em `app/src-tauri/gen/android` ainda não tenha sido inicializado:

```bash
cd spike && nix develop -c bash -c 'cd ../app/src-tauri && cargo tauri android init'
```

Para compilar o APK debug `aarch64` (sem instalar e sem usar `adb`):

```bash
cd spike && nix develop -c bash -c 'cd ../app/src-tauri && cargo tauri android build --debug --target aarch64 --apk'
```

O APK gerado fica em:
`app/src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`

# MVP Fatia 1 — Servidor de Sincronização (`bookferry-core` e `bookferry-server`)

Data: 13/11/2026 · Ambiente: NixOS (`x86_64-linux`), `rustc 1.100.0` via `spike/flake.nix` · Alvos verificados: `x86_64-unknown-linux-gnu` e `aarch64-linux-android` (NDK `27.3.13750724`).

---

## 1. Arquitetura e Layout do Workspace

O workspace Cargo na raiz do repositório ([`Cargo.toml`](file:///home/gappy/Documents/projetos/bookshelf/Cargo.toml)) é dividido em dois crates sem qualquer código copiado de `spike/server`, `koreader-sync-server` ou `Readest`, e com `#![forbid(unsafe_code)]` em ambos:

1. **[`crates/bookferry-core`](file:///home/gappy/Documents/projetos/bookshelf/crates/bookferry-core/src/lib.rs)** — Tipos de domínio e lógica pura (zero I/O de rede):
   - **Relógio Lógico Híbrido (`Hlc`, `HlcClock`, `Clock`)**:
     - Ordem total determinística `(wall_ms, logical, node)`.
     - Codificação textual canônica ordenável lexicograficamente em ASCII/SQLite: `{wall_ms:015}:{logical:06}:{node}` (testada com `proptest` para garantir `a.cmp(&b) == a.to_canonical_string().cmp(&b.to_canonical_string())`).
     - Regras formais de envio (`now`) e recebimento (`receive`) segundo Kulkarni et al., com relógio físico injetável (`Clock` trait: `SystemClock` em produção e `ManualClock` nos testes).
     - Proteção contra relógios desregulados: rejeita `wall_ms > now_ms + max_future_drift_ms` (padrão: 24 horas = `86_400_000 ms`) com código `hlc_too_far_in_future` (`HTTP 422`).
   - **Resolução de Conflitos LWW (`decide_lww`, `LwwBookRegister`)**:
     - Último-escritor-vence estritamente por HLC total `(wall_ms, logical, node)`.
     - Escritas atrasadas (`incoming <= current`) não sobrescrevem a posição ativa (`applied: false`), mas são preservadas no histórico limitado das últimas 20 entradas por livro.
   - **Identidade de Livro (`BookId`, `BookFormat`, `compute_partial_md5`)**:
     - **Algoritmo exato do Partial MD5 estilo KOReader**: para `i` de `-1` até `10`, calcula-se o offset `0` (quando `i = -1`) e `1024 << (2 * i)` (quando `i >= 0`), resultando nos 12 offsets exatos em bytes: `[0, 1024, 4096, 16384, 65536, 262144, 1048576, 4194304, 16777216, 67108864, 268_435_456, 1_073_741_824]`. Em cada offset, faz-se `seek(SeekFrom::Start(offset))` e lê-se até `1024` bytes em loop até completar 1 KiB ou atingir EOF; se `> 0` bytes forem lidos, alimenta-se o digest MD5; se `0` bytes forem lidos (`offset >= tamanho`), encerra-se a amostragem.
     - O `book_id` combina `{partial_md5_hex_32}-{size_bytes}` (ex.: `d41d8cd98f00b204e9800998ecf8427e-1048576`).
     - `BookId::parse` valida estritamente o formato `<32 hex lowercase>-<u64 decimal>`, rejeitando na origem qualquer tentativa de *path traversal* (`..`, `/`, `\`, `%`, caracteres de controle ou SQL).
   - **Pareamento e Segurança (`PairingQrPayload`, `constant_time_eq_str`, `sha256_hex`)**:
     - Payload de QR Code versionado (`v = 1`), implementação manual de `fmt::Debug` com `[REDACTED]` para segredos e tokens, e comparação em tempo constante via `subtle::ConstantTimeEq`.

2. **[`crates/bookferry-server`](file:///home/gappy/Documents/projetos/bookshelf/crates/bookferry-server/src/lib.rs)** — Biblioteca embarcável no app Tauri + binário fino de desenvolvimento `bookferry-serverd`:
   - **API Pública**: `pub async fn start(config: Config) -> Result<ServerHandle, ServerError>` com desligamento gracioso (`ServerHandle::shutdown(self)`).
   - **Zero estado global, zero caminhos absolutos fixos, zero `std::process::exit` na biblioteca, zero `unwrap`/`expect` nos caminhos de requisição HTTP**.
   - **[`src/tls.rs`](file:///home/gappy/Documents/projetos/bookshelf/crates/bookferry-server/src/tls.rs)**: Geração e persistência de chave privada PKCS#8 DER (`tls_key.der`, permissão `0600` no Unix) e certificado X.509 v3 DER (`tls_cert.der`) em `data_dir`, garantindo que `cert_sha256` e `spki_sha256` permaneçam idênticos após reiniciar o servidor. Inclui `PinnedServerCertVerifier` para verificação de certificado e SPKI sem CA do sistema.
   - **[`src/db.rs`](file:///home/gappy/Documents/projetos/bookshelf/crates/bookferry-server/src/db.rs)**: Persistência SQLite (`rusqlite` `bundled`, `WAL`, `busy_timeout = 5000ms`, `foreign_keys = ON`) com migração versionada via `PRAGMA user_version = 1` e transações `IMMEDIATE` para serializar escritas concorrentes sem corrupção.
   - **[`src/library.rs`](file:///home/gappy/Documents/projetos/bookshelf/crates/bookferry-server/src/library.rs)**: Varredura recursiva somente-leitura de `vault_dir` (`.epub`, `.pdf`, `.cbz`) e verificação de canonicalização (`canonical_file.starts_with(canonical_vault)`) antes de servir arquivos.
   - **[`src/routes.rs`](file:///home/gappy/Documents/projetos/bookshelf/crates/bookferry-server/src/routes.rs)**: Rotas Axum, limite de corpo JSON (`64 KiB`), cabeçalho `X-Bookferry-Api: 1` em todas as respostas e erros JSON uniformes `{error, message}`.
   - **[`src/mdns.rs`](file:///home/gappy/Documents/projetos/bookshelf/crates/bookferry-server/src/mdns.rs)**: Anúncio `_bookferry._tcp.local.` restrito aos IPs e interfaces de `Config` (`disable_interface(IfKind::All)` seguido de `enable_interface` seletivo).

---

## 2. Schema SQLite Completo (DDL — `PRAGMA user_version = 1`)

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

INSERT OR IGNORE INTO meta (key, value) VALUES ('server_seq', '0');

CREATE TABLE IF NOT EXISTS devices (
    id              TEXT PRIMARY KEY NOT NULL,
    name            TEXT NOT NULL,
    token_sha256    TEXT NOT NULL UNIQUE,
    created_at_ms   INTEGER NOT NULL,
    last_seen_at_ms INTEGER NOT NULL,
    revoked_at_ms   INTEGER NULL
);

CREATE INDEX IF NOT EXISTS idx_devices_token_sha256 ON devices(token_sha256);

CREATE TABLE IF NOT EXISTS books (
    book_id       TEXT PRIMARY KEY NOT NULL,
    partial_md5   TEXT NOT NULL,
    size_bytes    INTEGER NOT NULL,
    title         TEXT NOT NULL,
    format        TEXT NOT NULL CHECK (format IN ('epub', 'pdf', 'cbz')),
    mtime_ms      INTEGER NOT NULL,
    rel_path      TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS positions (
    book_id       TEXT PRIMARY KEY NOT NULL REFERENCES books(book_id) ON DELETE CASCADE,
    device_id     TEXT NOT NULL,
    hlc_canonical TEXT NOT NULL,
    hlc_wall_ms   INTEGER NOT NULL,
    hlc_logical   INTEGER NOT NULL,
    hlc_node      TEXT NOT NULL,
    locator       TEXT NOT NULL,
    snippet       TEXT NULL,
    percent       REAL NOT NULL CHECK (percent >= 0.0 AND percent <= 100.0),
    server_seq    INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_positions_hlc ON positions(hlc_canonical);

CREATE TABLE IF NOT EXISTS position_history (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    book_id        TEXT NOT NULL REFERENCES books(book_id) ON DELETE CASCADE,
    device_id      TEXT NOT NULL,
    hlc_canonical  TEXT NOT NULL,
    hlc_wall_ms    INTEGER NOT NULL,
    hlc_logical    INTEGER NOT NULL,
    hlc_node       TEXT NOT NULL,
    locator        TEXT NOT NULL,
    snippet        TEXT NULL,
    percent        REAL NOT NULL,
    applied        INTEGER NOT NULL CHECK (applied IN (0, 1)),
    server_seq     INTEGER NOT NULL,
    recorded_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_position_history_book_id ON position_history(book_id, id DESC);

CREATE TABLE IF NOT EXISTS sse_events (
    seq              INTEGER PRIMARY KEY NOT NULL,
    event_type       TEXT NOT NULL,
    book_id          TEXT NOT NULL,
    origin_device_id TEXT NOT NULL,
    payload_json     TEXT NOT NULL,
    created_at_ms    INTEGER NOT NULL
);
```

---

## 3. Tabela de Rotas HTTP

Todas as respostas incluem `X-Bookferry-Api: 1`. Todos os erros retornam JSON uniforme `{"error": "<codigo_estavel>", "message": "<mensagem>"}`.

| Método | Caminho | Auth (`Bearer`) | Request | Respostas e Códigos de Status |
|---|---|---|---|---|
| `GET` | `/status` | Opcional | — | `200 OK` `{status: "ok", version: "1"}`; com token de aparelho pareado acrescenta `server_name, book_count, uptime_ms` (ver `docs/07-seguranca-e-aparelhos.md`) |
| `POST` | `/pair` | Não (segredo único) | `{secret, device_name}` | `200 OK` `{device_id, device_name, token, cert_sha256, spki_sha256}`; `401` (`invalid_pairing_secret`); `403` (`pairing_secret_already_used`); `410` (`pairing_secret_expired`); `422` (`invalid_device_name`); `429` (`pairing_rate_limited`) |
| `GET` | `/devices` | Sim | — | `200 OK` `{devices: [{id, name, created_at_ms, last_seen_at_ms, revoked_at_ms?, revoked}]}`; `401` (`unauthorized`) |
| `DELETE` | `/devices/:id` | Sim | — | `200 OK` `{revoked: true, id}` (encerra streams SSE ativos desse dispositivo); `404` (`device_not_found`); `401` (`unauthorized`) |
| `POST` | `/library/rescan` | Sim | — | `200 OK` `{scanned_books, duplicates_skipped, total_books}`; `401` (`unauthorized`) |
| `GET` | `/books` | Sim | — | `200 OK` `{books: [{book_id, partial_md5, title, format, size_bytes, mtime_ms, position?}]}`; `401` (`unauthorized`) |
| `GET` | `/books/:id/file` | Sim | Header opcional `Range: bytes=<range>` | `200 OK` (streaming completo); `206 Partial Content` (streaming parcial + `Content-Range`, `ETag`, `Last-Modified`, `Accept-Ranges: bytes`); `416 Range Not Satisfiable` (`invalid_range` + `Content-Range: bytes */<size>`); `400` (`invalid_book_id`); `404` (`book_not_found`); `401` (`unauthorized`) |
| `PUT` | `/books/:id/position` | Sim | `{device_id, hlc, locator, snippet?, percent}` | `200 OK` `{applied, position, server_seq, server_hlc, history_count}`; `400` (`invalid_book_id`); `404` (`book_not_found`); `413` (`payload_too_large`); `422` (`hlc_too_far_in_future`, `invalid_hlc`, `invalid_percent`, `snippet_too_long`, `invalid_locator`, `invalid_device_id`); `401` (`unauthorized`) |
| `GET` | `/books/:id/position` | Sim | — | `200 OK` `{position, history}` (até 20 entradas mais recentes); `400` (`invalid_book_id`); `404` (`book_not_found` ou `position_not_found`); `401` (`unauthorized`) |
| `GET` | `/positions?since=<hlc>` | Sim | Query opcional `since` (HLC) | `200 OK` `{positions: [...]}` ordenado por HLC ascendente; `400` (`invalid_hlc`); `401` (`unauthorized`) |
| `GET` | `/events` | Sim | Header opcional `Last-Event-ID: <seq>` | `200 OK` (`text/event-stream`), emite `event: position` com `id: <seq>` quando `applied == true`, faz replay dos últimos N eventos persistidos no SQLite (`sse_events`), emite `: keepalive` a cada ~5 s, não ecoa evento ao próprio dispositivo escritor e fecha imediatamente se o dispositivo for revogado; `400` (`invalid_last_event_id`); `401` (`unauthorized`) |

---

## 4. Decisões Técnicas e Justificativas

### 4.1. Escolha de Crates (1 linha por escolha)
- **`tokio` + `axum` + `hyper-util`**: Stack HTTP assíncrona padrão da indústria em Rust, com roteamento tipado, suporte nativo a streaming `Body::from_stream` (Range e SSE) e `graceful_shutdown`.
- **`rustls` + `tokio-rustls` (provider `ring`)**: TLS 1.3/1.2 moderno sem dependência de OpenSSL ou CMake, compilando de forma limpa tanto para Linux quanto para `aarch64-linux-android`.
- **`rcgen`**: Geração em Rust puro de pares de chaves ECDSA P-256 e certificados X.509 v3 autoassinados com Subject Alternative Names (IPs e DNS local).
- **`rusqlite` (`bundled`)**: Embarca o SQLite em C puro via `cc` sem depender de `libsqlite3` do sistema operacional hospedeiro ou do Android, evitando o peso de compilação e conexões assíncronas do `sqlx` para um banco local single-node.
- **Migrações com `PRAGMA user_version`**: Nativo do próprio motor SQLite, transacional e determinístico, eliminando dependências externas de migração.
- **`mdns-sd`**: Implementação mDNS/DNS-SD em Rust puro com controle explícito de interfaces (`disable_interface` / `enable_interface`) validada no spike Android/Linux.
- **`sha2`**: Cálculo de SHA-256 para fingerprints do certificado DER, do SubjectPublicKeyInfo (SPKI) e armazenamento seguro de hashes de tokens no SQLite.
- **`md-5`**: Cálculo do Partial MD5 de 12 blocos de 1 KiB compatível com o algoritmo de identificação de arquivos do KOReader.
- **`subtle`**: Comparação em tempo constante (`ConstantTimeEq`) de segredos de pareamento, hashes de tokens Bearer e pins SHA-256 para mitigar ataques de canal lateral de tempo.
- **`getrandom`**: Geração CSPRNG direta do sistema operacional (256 bits de entropia para segredos de pareamento e tokens de dispositivo).
- **`serde` + `serde_json`**: Serialização e desserialização tipada de payloads JSON, eventos SSE e QR Code de pareamento.
- **`tracing` + `tracing-subscriber`**: Instrumentação estruturada assíncrona com redação de segredos nas implementações de `fmt::Debug`.
- **`thiserror`**: Definição concisa e sem boilerplate dos erros de domínio (`CoreError`) e de servidor (`ServerError`).
- **`httpdate`**: Formatação RFC 7231 exata do cabeçalho `Last-Modified` nas respostas de streaming de arquivos.
- **`proptest` + `tempfile` (dev)**: Testes baseados em propriedades para provar ordem total e monotonia do HLC e diretórios temporários isolados nos testes de integração.

### 4.2. Comportamentos de Protocolo Documentados
- **Segredo de pareamento expirado (`410 Gone`)**: Retorna `410 Gone` com código `"pairing_secret_expired"`, diferenciando semanticamente de segredo incorreto (`401 invalid_pairing_secret`), segredo já utilizado (`403 pairing_secret_already_used`) e bloqueio por força bruta (`429 pairing_rate_limited`).
- **Dois arquivos idênticos na biblioteca**: Como `book_id = {partial_md5}-{size_bytes}`, dois arquivos com o mesmo tamanho e amostras MD5 idênticas têm o mesmo `book_id`. A varredura ordena os caminhos relativos lexicograficamente, indexa o primeiro caminho como canônico e contabiliza as cópias adicionais em `duplicates_skipped`, preservando a unicidade da chave primária e a posição de leitura associada.
- **Múltiplos intervalos em HTTP `Range` (`bytes=0-10,20-30`)**: Rejeitados de forma determinística com `416 Range Not Satisfiable` (`error: "invalid_range"`, `Content-Range: bytes */<size>`), pois leitores PDF (`pdf.js`) e clientes de download retomável solicitam intervalos simples (`bytes=start-end`, `bytes=start-` ou `bytes=-suffix`).
- **Filtro de origem e Replay no SSE (`GET /events`)**:
  - Os eventos aplicados (`applied == true`) são persistidos na tabela `sse_events` do SQLite (retendo os últimos `200` eventos por padrão), permitindo que o replay por `Last-Event-ID` funcione inclusive através de reinicializações do servidor.
  - O servidor **não envia o evento de volta ao próprio dispositivo que realizou a escrita** (`origin_device_id == conn_device_id || origin_device_id == conn_device_name`) e também inclui o campo `origin_device_id` no JSON do evento emitido aos demais dispositivos.

---

## 5. Licenças dos Crates (`cargo metadata`)

Verificadas via `cargo metadata --format-version 1`. Nenhum crate na árvore de dependências possui licença `GPL` ou `AGPL` restritiva (todos os pacotes do grafo estão sob licenças permissivas (ver a conferência do orquestrador ao final)):

| Crate | Versão Exata | Licença (`cargo metadata`) |
|---|---|---|
| `axum` | `0.7.9` | `MIT` |
| `getrandom` | `0.2.17` / `0.4.3` | `MIT OR Apache-2.0` |
| `http-body-util` | `0.1.5` | `MIT` |
| `httpdate` | `1.0.3` | `MIT OR Apache-2.0` |
| `hyper` | `1.12.0` | `MIT` |
| `hyper-util` | `0.1.21` | `MIT` |
| `libsqlite3-sys` | `0.30.1` | `MIT` |
| `md-5` | `0.10.6` | `MIT OR Apache-2.0` |
| `mdns-sd` | `0.11.5` | `Apache-2.0 OR MIT` |
| `proptest` | `1.12.0` | `MIT OR Apache-2.0` |
| `rcgen` | `0.13.2` | `MIT OR Apache-2.0` |
| `ring` | `0.17.14` | `Apache-2.0 AND ISC` |
| `rusqlite` | `0.32.1` | `MIT` |
| `rustls` | `0.23.46` | `Apache-2.0 OR ISC OR MIT` |
| `rustls-pki-types` | `1.16.0` | `MIT OR Apache-2.0` |
| `serde` | `1.0.229` | `MIT OR Apache-2.0` |
| `serde_json` | `1.0.154` | `MIT OR Apache-2.0` |
| `sha2` | `0.10.9` | `MIT OR Apache-2.0` |
| `subtle` | `2.6.1` | `BSD-3-Clause` |
| `tempfile` | `3.27.0` | `MIT OR Apache-2.0` |
| `thiserror` | `1.0.69` | `MIT OR Apache-2.0` |
| `tokio` | `1.54.1` | `MIT` |
| `tokio-rustls` | `0.26.6` | `MIT OR Apache-2.0` |
| `tokio-stream` | `0.1.19` | `MIT` |
| `tokio-util` | `0.7.20` | `MIT` |
| `tracing` | `0.1.44` | `MIT` |
| `tracing-subscriber` | `0.3.23` | `MIT` |

---

## 6. Saídas Reais de Verificação (`cargo test`, `clippy`, `fmt` e Android Target)

### 6.1. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` e `cargo test --workspace -- --nocapture`

Comando executado:
```bash
cd spike && nix develop -c bash -c 'cd .. && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace -- --nocapture'
```

Saída real:
```text
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.11s
   Compiling bookferry-server v0.1.0 (/home/gappy/Documents/projetos/bookshelf/crates/bookferry-server)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 3.47s
     Running unittests src/lib.rs (target/debug/build/bookferry-core/5308ab17b5693dfa/out/bookferry_core-5308ab17b5693dfa)

running 8 tests
test tests::hlc_canonical_roundtrip_and_order ... ok
test tests::book_id_partial_md5_and_path_traversal_validation ... ok
test tests::hlc_rejects_absurd_future_drift ... ok
test tests::hlc_clock_monotonic_and_receive_advances ... ok
test tests::pairing_payload_redacts_secret_in_debug ... ok
test tests::lww_tie_breaking_and_history_retention ... ok
test tests::prop_hlc_clock_strictly_monotonic ... ok
test tests::prop_hlc_total_order_matches_canonical_string ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s

     Running unittests src/lib.rs (target/debug/build/bookferry-server/3135e336f23d7140/out/bookferry_server-3135e336f23d7140)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/main.rs (target/debug/build/bookferry-server/1de984446bafa238/out/bookferry_serverd-1de984446bafa238)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/integration.rs (target/debug/build/bookferry-server/6102620d8bc5e2fe/out/integration-6102620d8bc5e2fe)

running 7 tests
test mdns_live_announcement_on_loopback ... ignored, requer suporte a multicast UDP na interface de rede local
test mdns_invalid_interface_never_crashes_server ... ok
test tls_persistent_fingerprint_pinning_and_cleartext_rejected ... ok
test library_scan_duplicates_http_range_and_path_traversal ... ok
test pairing_lifecycle_token_hashing_expiration_bruteforce_and_revocation ... ok
test positions_lww_future_drift_since_persistence_and_concurrency ... ok
[BENCH_LOOPBACK] put_to_sse_latency_us=3932 (3.932 ms) | range_bytes=10488080 elapsed_ms=74.27 throughput_mib_s=134.67
test sse_events_replay_revocation_and_loopback_benchmarks ... ok

test result: ok. 6 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.30s
```

### 6.2. Medidas de Referência em Loopback (`127.0.0.1`, sobre TLS 1.3 com Pinning, perfil `debug`)
- **Latência `PUT /books/{id}/position` → Recepção do evento `position` no cliente SSE**: **`3,93 ms` (`3.932 µs`)**.
- **Vazão de streaming `GET /books/{id}/file` com HTTP `Range` sobre TLS**: **`134,67 MiB/s`** (`10.488.080 bytes` transferidos em `74,27 ms`). *(Apenas referência de CPU/stack em loopback; a medida real em rede Wi-Fi LAN foi de `12,54 MiB/s` no Spike 03).*

### 6.3. Compilação Cruzada para Android (`aarch64-linux-android`)

Comando executado usando o NDK `27.3.13750724` presente em `spike/flake.nix`:
```bash
cd spike && nix develop -c bash -c '
  cd .. &&
  NDK_BIN="$NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin" \
  CC_aarch64_linux_android="$NDK_BIN/aarch64-linux-android24-clang" \
  AR_aarch64_linux_android="$NDK_BIN/llvm-ar" \
  CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$NDK_BIN/aarch64-linux-android24-clang" \
  cargo check --target aarch64-linux-android -p bookferry-server
'
```
Saída real:
```text
   Compiling ring v0.17.14
   Compiling libsqlite3-sys v0.30.1
    Checking bookferry-core v0.1.0 (/home/gappy/Documents/projetos/bookshelf/crates/bookferry-core)
    Checking rusqlite v0.32.1
    Checking tokio-rustls v0.26.6
    Checking hyper v1.12.0
    Checking hyper-util v0.1.21
    Checking axum v0.7.9
    Checking bookferry-server v0.1.0 (/home/gappy/Documents/projetos/bookshelf/crates/bookferry-server)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.79s
```

---

## 7. O Que NÃO Foi Verificado Nesta Fatia

1. **Execução em dispositivo Android físico e rede Wi-Fi real nesta fatia**: Conforme especificado para a Fatia 1, todos os testes foram executados exclusivamente no PC em loopback (`127.0.0.1:0`) sem uso de `adb` ou celular.
2. **Descoberta mDNS multicast real fim-a-fim nesta fatia**: O teste `mdns_live_announcement_on_loopback` está marcado com `#[ignore]` porque depende de suporte a multicast UDP na interface de rede; o que foi testado automaticamente em loopback é que interfaces inválidas ou falhas de mDNS nunca derrubam o servidor (`mdns_invalid_interface_never_crashes_server`).
3. **`cargo audit` e `cargo deny`**: Não estão instalados no `devShell` de `spike/flake.nix` (`error: no such command: audit` e `error: no such command: deny`), portanto a auditoria automatizada de CVEs do RustSec não foi executada; as licenças foram auditadas diretamente via `cargo metadata --format-version 1`.

---

## 8. Divergências Deliberadas em Relação ao Spike (`spike/server`)

| Aspecto | Como era no `spike/server` | Como foi implementado no produto (`crates/bookferry-*`) |
|---|---|---|
| **Stack HTTP/TLS** | Parser HTTP/1.1 e SSE escritos à mão sobre `std::net::TcpListener` e threads bloqueantes (`2.121` linhas em um único `lib.rs`). | Stack assíncrona `tokio` + `axum` + `hyper-util` + `tokio-rustls` modularizada em `bookferry-core` e `bookferry-server`. |
| **Persistência de Estado** | `Mutex<ServerState>` volátil em memória (perdia dispositivos e posições ao reiniciar). | Banco SQLite (`bookferry.db`) em modo `WAL` com migrações `PRAGMA user_version = 1`, preservando dispositivos, livros, posições, histórico LWW e fila de replay SSE. |
| **Identidade TLS** | Gerava certificado e chave novos a cada inicialização, invalidando o pin do QR Code. | Persiste `tls_cert.der` e `tls_key.der` (`0600`) em `data_dir` na 1ª execução e reutiliza nas seguintes, mantendo `cert_sha256` e `spki_sha256` idênticos. |
| **Tokens de Dispositivo** | Guardados em texto claro na memória e com token fixo de CLI (`bf_cli_local_pc_token`). | Tokens aleatórios de 256 bits gerados por CSPRNG no `POST /pair`; o servidor persiste **exclusivamente o hash SHA-256** (`token_sha256`) no SQLite. |
| **Rotas REST** | `PUT/GET /position/{book_id}` e `GET /file/{name}` por nome de arquivo. | `GET /books`, `POST /library/rescan`, `GET /books/{id}/file` (por `book_id`, nunca caminho), `PUT/GET /books/{id}/position`, `GET /positions?since=<hlc>`, `GET/DELETE /devices[/{id}]`. |
| **Configuração** | Lida de variáveis de ambiente e caminhos absolutos fixos (`/home/gappy/.../spike`). | 100% via struct `Config` passada para `start(config: Config) -> Result<ServerHandle, ServerError>`, sem caminhos absolutos nem `std::process::exit`. |

---

## 9. Itens para as Próximas Fatias do MVP / v1

1. **Integração no App Desktop/Mobile Tauri v2 (Fatia 2/3)**: Embutir `bookferry_server::start(Config)` no processo principal do Tauri Desktop e integrar os leitores `foliate-js` (EPUB) e `pdf.js`/visualizador CBZ com a fila offline (outbox) no SQLite do cliente.
2. **Extração e Cache de Capas**: Extrair capa embutida de EPUB/CBZ/PDF na indexação da biblioteca e servir em rota dedicada (`GET /books/{id}/cover`).
3. **Lista "Ler a Seguir"**: Tabelas e rotas CRUD sincronizadas por HLC + `sort_key` fracionário e busca de capas na Open Library + upload manual.
4. **Adaptador KOSync (`/kosync/*`)**: Camada opcional de compatibilidade com clientes KOReader validada contra `kosync-conformance`.
5. **Relay Sem Estado (v1)**: Túnel cifrado ponta a ponta para sincronização fora da rede local quando os dispositivos não estão na mesma LAN.


## Conferência do orquestrador (13/11/2026)

Refeita depois do agente, com o código dele:

- **`cargo test --workspace`:** 8 testes no core + 6 de integração passando, 1 ignorado (mDNS real). `cargo fmt --check` e `cargo clippy --all-targets -D warnings` limpos. `cargo check --target aarch64-linux-android -p bookferry-server` compila. Código de requisição sem `unwrap`/`expect`; `#![forbid(unsafe_code)]` nos 3 crates.
- **Testes de mutação (quebrei o código de propósito):** LWW invertido e SSE ecoando ao próprio escritor foram pegos. **A remoção do limite do `Range` além do fim do arquivo NÃO foi pega** (buraco real de cobertura). Acrescentei o caso `bytes=5000-99999999` em `tests/integration.rs`, e agora a mutação é pega.
- **Cliente independente (`curl` contra o `bookferry-serverd` compilado, com dados em diretório temporário):** pin certo `200`, pin errado e HTTP puro recusados; sem token `401`; segredo errado `401`; segredo válido `200`; reuso `403`; `Range 100-199` `206` com bytes idênticos; fim além do tamanho limitado a `20000-20008/20009`; ids com `..` dão `400`/`404`; PUT mais novo aplica, mais antigo `applied:false`, HLC a +11 dias `422`. Reinício: fingerprint igual, segredo novo, token antigo ainda vale, posição persistiu; revogado dá `401`. O segredo aparece uma vez no stdout (a linha do QR, de propósito) e em nenhum log; token em claro não existe no SQLite.
- **Licenças:** 184 pacotes de registro, não 186. Há 1 crate com opção `LGPL-2.1-or-later` (escolhe-se MIT/Apache), 1 com `Unicode-3.0` (permissiva) e 1 `Unlicense OR MIT`; nenhum só-copyleft. `cargo audit`/`deny` continuam sem rodar.
- **Achados abertos:** (1) o arquivo SQLite sai com permissão `0644` (só guarda hashes, mas deveria ser `0600`); (2) o `/pair` exige `Content-Type: application/json` (correto, mas o cliente precisa mandar); (3) o código dos 6 testes de integração é grande e denso (cada um cobre vários cenários), então uma falha dá diagnóstico ruim; (4) o QR com segredo sai no stdout do daemon de desenvolvimento, e o app Tauri precisa exibi-lo só na tela de pareamento; (5) a flag `--announce-ip` liga o mDNS, e mDNS real não foi testado.

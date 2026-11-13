# `bookferry-server`

Servidor de sincronização local do **Bookferry**, projetado para ser embarcado dentro do aplicativo desktop Tauri (Linux/Windows/macOS) e compatível com compilação cruzada para Android (`aarch64-linux-android`).

## Características Principais

- **API Assíncrona Embarcável**: `bookferry_server::start(config: Config) -> Result<ServerHandle, ServerError>` com desligamento limpo (`ServerHandle::shutdown`), sem estado global, sem caminhos absolutos fixos e sem `std::process::exit`.
- **TLS 1.3/1.2 Persistente (`rustls` + `ring` + `rcgen`)**: gera chave privada ECDSA P-256 (`0600`) e certificado autoassinado na primeira execução em `data_dir`, preservando os fingerprints `cert_sha256` e `spki_sha256` entre reinicializações.
- **Pareamento Seguro**: segredo de uso único CSPRNG (256 bits) com expiração configurável, limite de tentativas falhas com bloqueio temporário (anti *brute-force*), comparação em tempo constante (`subtle`) e armazenamento exclusivo do hash SHA-256 do token de dispositivo no SQLite.
- **Biblioteca e Streaming HTTP Range**: indexação de `.epub`, `.pdf` e `.cbz` em `vault_dir` (somente leitura) com `book_id` estável (`{partial_md5}-{size}`), proteção contra *path traversal* e streaming `200`/`206`/`416` sem carregar o arquivo na RAM.
- **Posição de Leitura (HLC + LWW) e Tempo Real (SSE)**: resolução de conflitos determinística por HLC total, rejeição de relógios no futuro (`> 24h`) com `422`, histórico limitado por livro (20 entradas), `GET /positions?since=<hlc>` e `GET /events` com `Last-Event-ID`, keepalive e encerramento imediato ao revogar dispositivo.
- **Binário de Desenvolvimento**: `bookferry-serverd` para execução standalone durante desenvolvimento.

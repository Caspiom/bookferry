# `bookferry-client`

Biblioteca cliente de sincronização offline-first do **Bookferry** (`#![forbid(unsafe_code)]`), compartilhada entre o app Tauri v2 no Android e no Desktop.

## Responsabilidades
- **Pareamento e Pinning TLS**: Pareamento por payload de QR Code (`PairingQrPayload` ou string JSON), fixando `cert_sha256` e/ou `spki_sha256` via `rustls` (`ring`) sem depender de CAs do sistema operacional. Suporta um único servidor pareado por instalação.
- **Localização e Descoberta**: Tentativa em ordem determinística (último IP válido → demais IPs do QR Code → descoberta mDNS `_bookferry._tcp.local.` validada contra o registro TXT `fp`), atualizando o IP salvo quando o servidor muda de endereço via DHCP.
- **Biblioteca e Download Retomável**: Cache local em SQLite (`stale = true` quando offline) e download para `data_dir/books/<book_id>` via arquivo temporário `.part` com retomada HTTP `Range` (`206 Partial Content`), verificação de tamanho, `ETag` e Partial MD5, streaming sem carregar o arquivo na RAM e suporte a cancelamento.
- **Posição de Leitura Offline-First (HLC + Outbox + LWW)**: Relógio HLC monotônico persistido no SQLite (com `node = device_id`), gravação local imediata em `set_position`, fila `outbox` persistente com envio em background (1 `PUT` por livro com o estado mais recente), reconciliação LWW (`applied = false`), tratamento de `401` (`Unpaired`) e `422 hlc_too_far_in_future` (ajuste automático), `pull()` incremental (`GET /positions?since=`) em toda reconexão SSE e em `on_resume()`.
- **Tempo Real (SSE)**: Stream `GET /events` com `Last-Event-ID` persistido, timeout de leitura (~25 s) para detecção de conexão meio-aberta, backoff exponencial com jitter e filtro contra eco do próprio dispositivo.
- **Decisão de Retomada (`decide_resume`)**: Lógica pura para decidir entre `UseLocal`, `UseRemote`, `Ask { remote, local }` (diferença de progresso acima do limiar configurável, padrão 2 pontos percentuais) e `Nothing`.

Documentação detalhada: [`docs/mvp/02-cliente.md`](../../docs/mvp/02-cliente.md).

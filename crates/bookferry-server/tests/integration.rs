use axum::http::{HeaderMap, Method, Request, StatusCode};
use bookferry_core::{ManualClock, PositionUpdate, API_VERSION, DEFAULT_MAX_FUTURE_DRIFT_MS};
use bookferry_server::{
    build_pinned_client_config, start, BookMetaResponse, BooksListResponse, Config,
    DevicesListResponse, ErrorBody, GetPositionResponse, PairResponse, PositionsListResponse,
    PutPositionResponse, RescanResponse, RevokeDeviceResponse, SsePositionPayload, StatusResponse,
    CERT_DER_FILENAME, HEADER_BOOKFERRY_API, KEY_DER_FILENAME,
};
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper_util::rt::TokioIo;
use rusqlite::Connection;
use rustls::pki_types::ServerName;
use serde::de::DeserializeOwned;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

/// Gera um arquivo ZIP valido minimo em memoria (usado para `.epub` e `.cbz` sinteticos de poucos KB).
fn make_minimal_zip(entry_name: &str, payload: &[u8]) -> Vec<u8> {
    let name_bytes = entry_name.as_bytes();
    let name_len = name_bytes.len() as u16;
    let data_len = payload.len() as u32;
    let crc32: u32 = 0; // stored sem verificacao estrita nos testes de scan/range

    let mut out = Vec::with_capacity(128 + payload.len());
    // Local File Header (0x04034b50)
    out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
    out.extend_from_slice(&20u16.to_le_bytes()); // version needed
    out.extend_from_slice(&0u16.to_le_bytes()); // flags
    out.extend_from_slice(&0u16.to_le_bytes()); // compression = 0 (stored)
    out.extend_from_slice(&0u16.to_le_bytes()); // mod time
    out.extend_from_slice(&0u16.to_le_bytes()); // mod date
    out.extend_from_slice(&crc32.to_le_bytes());
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&name_len.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // extra len
    out.extend_from_slice(name_bytes);
    out.extend_from_slice(payload);

    let cd_offset = out.len() as u32;
    // Central Directory File Header (0x02014b50)
    out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
    out.extend_from_slice(&20u16.to_le_bytes()); // version made by
    out.extend_from_slice(&20u16.to_le_bytes()); // version needed
    out.extend_from_slice(&0u16.to_le_bytes()); // flags
    out.extend_from_slice(&0u16.to_le_bytes()); // compression
    out.extend_from_slice(&0u16.to_le_bytes()); // mod time
    out.extend_from_slice(&0u16.to_le_bytes()); // mod date
    out.extend_from_slice(&crc32.to_le_bytes());
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&name_len.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // extra len
    out.extend_from_slice(&0u16.to_le_bytes()); // comment len
    out.extend_from_slice(&0u16.to_le_bytes()); // disk number
    out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
    out.extend_from_slice(&0u32.to_le_bytes()); // external attrs
    out.extend_from_slice(&0u32.to_le_bytes()); // local header offset
    out.extend_from_slice(name_bytes);

    let cd_size = (out.len() as u32) - cd_offset;
    // End of Central Directory (0x06054b50)
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // disk number
    out.extend_from_slice(&0u16.to_le_bytes()); // cd start disk
    out.extend_from_slice(&1u16.to_le_bytes()); // entries on disk
    out.extend_from_slice(&1u16.to_le_bytes()); // total entries
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment len
    out
}

fn make_minimal_pdf(body_marker: &[u8]) -> Vec<u8> {
    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.4\n1 0 obj<< /Type /Catalog >>endobj\n%");
    pdf.extend_from_slice(body_marker);
    pdf.extend_from_slice(b"\n%%EOF\n");
    pdf
}

fn populate_sample_vault(vault_dir: &Path) {
    let epub_bytes = make_minimal_zip("OEBPS/ch1.xhtml", &[b'E'; 4096]);
    let pdf_bytes = make_minimal_pdf(&[b'P'; 6000]);
    let cbz_bytes = make_minimal_zip("page001.jpg", &[b'C'; 8192]);

    std::fs::write(vault_dir.join("Dom Casmurro.epub"), &epub_bytes).unwrap();
    std::fs::write(vault_dir.join("Manual Rust.pdf"), &pdf_bytes).unwrap();
    std::fs::write(vault_dir.join("Quadrinho Edicao 1.cbz"), &cbz_bytes).unwrap();
    // Arquivo nao suportado que deve ser ignorado
    std::fs::write(vault_dir.join("notas.txt"), b"ignorar este txt").unwrap();
}

struct PinnedTestClient {
    addr: SocketAddr,
    connector: TlsConnector,
}

impl PinnedTestClient {
    fn new(addr: SocketAddr, cert_sha256: &str, spki_sha256: Option<&str>) -> Self {
        let cfg = build_pinned_client_config(
            cert_sha256.to_string(),
            spki_sha256.map(ToString::to_string),
        )
        .expect("pinned client config");
        Self {
            addr,
            connector: TlsConnector::from(Arc::new(cfg)),
        }
    }

    async fn connect_tls(
        &self,
    ) -> Result<tokio_rustls::client::TlsStream<TcpStream>, std::io::Error> {
        let tcp = TcpStream::connect(self.addr).await?;
        tcp.set_nodelay(true)?;
        let domain = ServerName::try_from("bookferry.local")
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        self.connector.connect(domain, tcp).await
    }

    async fn request_raw(
        &self,
        method: Method,
        uri: &str,
        bearer_token: Option<&str>,
        extra_headers: &[(&str, &str)],
        body_bytes: Vec<u8>,
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let tls_stream = self.connect_tls().await.expect("TLS handshake");
        let io = TokioIo::new(tls_stream);
        let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
            .await
            .expect("HTTP/1.1 handshake");

        tokio::spawn(async move {
            let _ = conn.await;
        });

        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "bookferry.local");

        if let Some(tok) = bearer_token {
            builder = builder.header("authorization", format!("Bearer {tok}"));
        }
        if !body_bytes.is_empty() {
            builder = builder.header("content-type", "application/json");
        }
        for &(k, v) in extra_headers {
            builder = builder.header(k, v);
        }

        let req = builder
            .body(Full::new(Bytes::from(body_bytes)))
            .expect("valid request");
        let resp = sender.send_request(req).await.expect("HTTP response");
        let status = resp.status();
        let headers = resp.headers().clone();

        // Garante que X-Bookferry-Api: 1 esta presente em TODAS as respostas
        assert_eq!(
            headers
                .get(HEADER_BOOKFERRY_API)
                .and_then(|v| v.to_str().ok()),
            Some(API_VERSION),
            "cabecalho X-Bookferry-Api: 1 obrigatorio em {uri}"
        );

        let collected = resp
            .into_body()
            .collect()
            .await
            .expect("collect response body")
            .to_bytes()
            .to_vec();

        (status, headers, collected)
    }

    async fn request_json<T: DeserializeOwned>(
        &self,
        method: Method,
        uri: &str,
        bearer_token: Option<&str>,
        body_bytes: Vec<u8>,
    ) -> (StatusCode, T) {
        let (status, _, bytes) = self
            .request_raw(method, uri, bearer_token, &[], body_bytes)
            .await;
        let parsed: T = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "falha ao desserializar JSON de {uri} (status={status}): {e}; body={}",
                String::from_utf8_lossy(&bytes)
            )
        });
        (status, parsed)
    }

    async fn pair_device(&self, secret: &str, device_name: &str) -> (StatusCode, Vec<u8>) {
        let body = serde_json::to_vec(&serde_json::json!({
            "secret": secret,
            "device_name": device_name,
        }))
        .unwrap();
        let (status, _, bytes) = self
            .request_raw(Method::POST, "/pair", None, &[], body)
            .await;
        (status, bytes)
    }

    async fn open_sse_stream(
        &self,
        bearer_token: &str,
        last_event_id: Option<&str>,
    ) -> hyper::Response<Incoming> {
        let tls_stream = self.connect_tls().await.expect("TLS handshake for SSE");
        let io = TokioIo::new(tls_stream);
        let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
            .await
            .expect("HTTP/1.1 handshake for SSE");

        tokio::spawn(async move {
            let _ = conn.await;
        });

        let mut builder = Request::builder()
            .method(Method::GET)
            .uri("/events")
            .header("host", "bookferry.local")
            .header("authorization", format!("Bearer {bearer_token}"));

        if let Some(id) = last_event_id {
            builder = builder.header("last-event-id", id);
        }

        let req = builder.body(Full::new(Bytes::new())).unwrap();
        let resp = sender.send_request(req).await.expect("SSE response");
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(HEADER_BOOKFERRY_API)
                .and_then(|v| v.to_str().ok()),
            Some(API_VERSION)
        );
        resp
    }
}

#[derive(Debug, Clone)]
struct ParsedSseEvent {
    id: Option<u64>,
    event: String,
    data: String,
}

struct SseStreamReader {
    body: Incoming,
    buffer: String,
}

impl SseStreamReader {
    fn new(resp: hyper::Response<Incoming>) -> Self {
        Self {
            body: resp.into_body(),
            buffer: String::new(),
        }
    }

    async fn next_event(&mut self, timeout_dur: Duration) -> Option<ParsedSseEvent> {
        let deadline = Instant::now() + timeout_dur;
        loop {
            if let Some(pos) = self.buffer.find("\n\n") {
                let block = self.buffer[..pos].to_string();
                self.buffer.drain(..pos + 2);
                if block.trim().is_empty() || block.starts_with(':') {
                    continue;
                }
                let mut id = None;
                let mut event = "message".to_string();
                let mut data = String::new();
                for line in block.lines() {
                    if let Some(rest) = line.strip_prefix("id:") {
                        id = rest.trim().parse::<u64>().ok();
                    } else if let Some(rest) = line.strip_prefix("event:") {
                        event = rest.trim().to_string();
                    } else if let Some(rest) = line.strip_prefix("data:") {
                        data = rest.trim().to_string();
                    }
                }
                return Some(ParsedSseEvent { id, event, data });
            }

            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let remaining = deadline - now;
            match tokio::time::timeout(remaining, self.body.frame()).await {
                Ok(Some(Ok(frame))) => {
                    if let Ok(data) = frame.into_data() {
                        self.buffer.push_str(&String::from_utf8_lossy(&data));
                    }
                }
                Ok(Some(Err(_))) | Ok(None) | Err(_) => return None,
            }
        }
    }

    async fn wait_for_eof(&mut self, timeout_dur: Duration) -> bool {
        let deadline = Instant::now() + timeout_dur;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            match tokio::time::timeout(deadline - now, self.body.frame()).await {
                Ok(None) => return true,
                Ok(Some(Err(_))) => return true,
                Ok(Some(Ok(_))) => continue,
                Err(_) => return false,
            }
        }
    }
}

fn test_config(data_dir: &Path, vault_dir: &Path) -> Config {
    let mut cfg = Config::new(data_dir, vault_dir);
    cfg.bind_addr = IpAddr::V4(Ipv4Addr::LOCALHOST);
    cfg.port = 0;
    cfg.enable_mdns = false;
    cfg.failure_delay = Duration::from_millis(5);
    cfg
}

// ============================================================================
// 1. TLS Persistente, Pinning (certo/errado/HTTP puro) e Permissao 0600
// ============================================================================

#[tokio::test]
async fn tls_persistent_fingerprint_pinning_and_cleartext_rejected() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().join("data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    let handle1 = start(test_config(&data_dir, &vault_dir)).await.unwrap();
    let addr1 = handle1.local_addr();
    let cert_fp1 = handle1.cert_sha256().to_string();
    let spki_fp1 = handle1.spki_sha256().to_string();

    // Verifica que cert e chave foram gravados em data_dir e que a chave tem permissao 0600
    assert!(data_dir.join(CERT_DER_FILENAME).exists());
    let key_path = data_dir.join(KEY_DER_FILENAME);
    assert!(key_path.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "tls_key.der deve ter permissao 0600");
    }

    // (a) Pin certo (cert + spki) -> 200 OK em GET /status
    let good_client = PinnedTestClient::new(addr1, &cert_fp1, Some(&spki_fp1));
    let (status, status_body): (_, StatusResponse) = good_client
        .request_json(Method::GET, "/status", None, Vec::new())
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(status_body.status, "ok");
    // Sem token, /status nao revela nome do servidor, tamanho da biblioteca nem uptime.
    assert_eq!(status_body.book_count, None);
    assert_eq!(status_body.server_name, None);
    assert_eq!(status_body.uptime_ms, None);

    // (b) Pin errado (cert_sha256 diferente) -> Handshake TLS falha
    let bad_cert_client = PinnedTestClient::new(
        addr1,
        "000000000000000000000000000000000000000000000000000000000000dead",
        None,
    );
    assert!(
        bad_cert_client.connect_tls().await.is_err(),
        "pin de certificado errado deve falhar no handshake TLS"
    );

    // (b.2) Pin errado de SPKI -> Handshake TLS falha
    let bad_spki_client = PinnedTestClient::new(
        addr1,
        &cert_fp1,
        Some("000000000000000000000000000000000000000000000000000000000000beef"),
    );
    assert!(
        bad_spki_client.connect_tls().await.is_err(),
        "pin de SPKI errado deve falhar no handshake TLS"
    );

    // (c) HTTP puro sem TLS -> recusado (servidor envia alerta fatal TLS e fecha conexao)
    let mut raw_tcp = TcpStream::connect(addr1).await.unwrap();
    raw_tcp
        .write_all(b"GET /status HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut raw_resp = Vec::new();
    let _ = raw_tcp.read_to_end(&mut raw_resp).await;
    let resp_str = String::from_utf8_lossy(&raw_resp);
    assert!(
        !resp_str.starts_with("HTTP/1."),
        "conexao HTTP sem TLS jamais deve receber resposta HTTP em claro"
    );

    // Reinicia o servidor no mesmo data_dir e confirma que cert_sha256 e spki_sha256 permanecem IDENTICOS
    handle1.shutdown().await.unwrap();

    let handle2 = start(test_config(&data_dir, &vault_dir)).await.unwrap();
    assert_eq!(handle2.cert_sha256(), cert_fp1);
    assert_eq!(handle2.spki_sha256(), spki_fp1);

    let good_client2 = PinnedTestClient::new(handle2.local_addr(), &cert_fp1, Some(&spki_fp1));
    let (status2, _): (_, StatusResponse) = good_client2
        .request_json(Method::GET, "/status", None, Vec::new())
        .await;
    assert_eq!(status2, StatusCode::OK);

    handle2.shutdown().await.unwrap();
}

// ============================================================================
// 2. Pareamento: OK, Errado, Reuso, Expirado (relogio injetado), Brute Force,
//    Hash no SQLite e Revogacao de Dispositivo
// ============================================================================

#[tokio::test]
async fn pairing_lifecycle_token_hashing_expiration_bruteforce_and_revocation() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().join("data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    let clock = Arc::new(ManualClock::new(1_700_000_000_000));
    let mut cfg = test_config(&data_dir, &vault_dir);
    cfg.clock = clock.clone();
    cfg.pairing_ttl = Duration::from_secs(300); // 5 min = 300_000 ms
    cfg.max_failed_pair_attempts = 3;
    cfg.pair_lockout_duration = Duration::from_secs(60); // 60_000 ms

    let handle = start(cfg).await.unwrap();
    let qr = handle.pairing_payload();
    assert_eq!(qr.v, 1);
    assert_eq!(qr.port, handle.local_addr().port());
    assert_eq!(qr.cert_sha256, handle.cert_sha256());
    assert_eq!(qr.spki_sha256, handle.spki_sha256());

    let client = PinnedTestClient::new(
        handle.local_addr(),
        handle.cert_sha256(),
        Some(handle.spki_sha256()),
    );

    // 1. Segredo errado -> 401 invalid_pairing_secret
    let (st_wrong, bytes_wrong) = client.pair_device("segredo-errado", "Celular").await;
    assert_eq!(st_wrong, StatusCode::UNAUTHORIZED);
    let err_wrong: ErrorBody = serde_json::from_slice(&bytes_wrong).unwrap();
    assert_eq!(err_wrong.error, "invalid_pairing_secret");

    // 2. Segredo correto -> 200 OK + token >= 128 bits
    let (st_ok, bytes_ok) = client.pair_device(&qr.secret, "Galaxy S23").await;
    assert_eq!(st_ok, StatusCode::OK);
    let pair_ok: PairResponse = serde_json::from_slice(&bytes_ok).unwrap();
    assert!(pair_ok.token.len() >= 32);

    // 3. Reuso do mesmo segredo -> 403 pairing_secret_already_used
    let (st_reuse, bytes_reuse) = client.pair_device(&qr.secret, "Outro App").await;
    assert_eq!(st_reuse, StatusCode::FORBIDDEN);
    let err_reuse: ErrorBody = serde_json::from_slice(&bytes_reuse).unwrap();
    assert_eq!(err_reuse.error, "pairing_secret_already_used");

    // 4. Inspeciona SQLite diretamente: o token em claro NUNCA esta no banco, apenas seu SHA-256
    {
        let conn = Connection::open(handle.db_path()).unwrap();
        let stored_hashes: Vec<String> = conn
            .prepare("SELECT token_sha256 FROM devices")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(stored_hashes.len(), 1);
        assert_ne!(stored_hashes[0], pair_ok.token);
        assert_eq!(
            stored_hashes[0],
            bookferry_core::sha256_hex(pair_ok.token.as_bytes())
        );
    }

    // 5. Segredo expirado (avanca o relogio injetado alem de 5 min) -> 410 pairing_secret_expired
    let qr_exp = handle.rotate_pairing_secret().unwrap();
    clock.advance_ms(300_001);
    let (st_exp, bytes_exp) = client.pair_device(&qr_exp.secret, "Tablet").await;
    assert_eq!(st_exp, StatusCode::GONE);
    let err_exp: ErrorBody = serde_json::from_slice(&bytes_exp).unwrap();
    assert_eq!(err_exp.error, "pairing_secret_expired");

    // 6. Anti brute-force: 3 tentativas erradas bloqueiam novas tentativas com 429 Too Many Requests,
    //    mesmo que a 4a tentativa envie o segredo correto!
    let qr_bf = handle.rotate_pairing_secret().unwrap();
    for _ in 0..3 {
        let (st, _) = client.pair_device("chute-errado", "Attacker").await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }
    let (st_locked, bytes_locked) = client.pair_device(&qr_bf.secret, "Legitimo").await;
    assert_eq!(st_locked, StatusCode::TOO_MANY_REQUESTS);
    let err_locked: ErrorBody = serde_json::from_slice(&bytes_locked).unwrap();
    assert_eq!(err_locked.error, "pairing_rate_limited");

    // Apos passar o tempo de lockout (60 s), o pareamento volta a aceitar o segredo valido
    clock.advance_ms(60_001);
    let (st_after_lock, bytes_after_lock) = client.pair_device(&qr_bf.secret, "Notebook").await;
    assert_eq!(st_after_lock, StatusCode::OK);
    let pair_2: PairResponse = serde_json::from_slice(&bytes_after_lock).unwrap();

    // 7. GET /devices lista os 2 dispositivos pareados
    let (st_devs, devs_resp): (_, DevicesListResponse) = client
        .request_json(Method::GET, "/devices", Some(&pair_ok.token), Vec::new())
        .await;
    assert_eq!(st_devs, StatusCode::OK);
    assert_eq!(devs_resp.devices.len(), 2);

    // 8. DELETE /devices/{id} revoga o segundo dispositivo; o token dele passa a dar 401 imediatamente
    let (st_rev, rev_resp): (_, RevokeDeviceResponse) = client
        .request_json(
            Method::DELETE,
            &format!("/devices/{}", pair_2.device_id),
            Some(&pair_ok.token),
            Vec::new(),
        )
        .await;
    assert_eq!(st_rev, StatusCode::OK);
    assert!(rev_resp.revoked);

    let (st_unauth, err_unauth): (_, ErrorBody) = client
        .request_json(Method::GET, "/books", Some(&pair_2.token), Vec::new())
        .await;
    assert_eq!(st_unauth, StatusCode::UNAUTHORIZED);
    assert_eq!(err_unauth.error, "unauthorized");

    handle.shutdown().await.unwrap();
}

// ============================================================================
// 3. Biblioteca: Scan, Duplicados, Rescan, HTTP Range (inicio/meio/fim/invalido/multiplos)
//    e Protecao contra Path Traversal
// ============================================================================

#[tokio::test]
async fn library_scan_duplicates_http_range_and_path_traversal() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().join("data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    let handle = start(test_config(&data_dir, &vault_dir)).await.unwrap();
    let qr = handle.pairing_payload();
    let client = PinnedTestClient::new(
        handle.local_addr(),
        handle.cert_sha256(),
        Some(handle.spki_sha256()),
    );

    let (_, pair_bytes) = client.pair_device(&qr.secret, "Leitor PC").await;
    let pair: PairResponse = serde_json::from_slice(&pair_bytes).unwrap();
    let token = &pair.token;

    // Lista os 3 livros iniciais (EPUB, PDF, CBZ; o .txt foi ignorado)
    let (st_books, list1): (_, BooksListResponse) = client
        .request_json(Method::GET, "/books", Some(token), Vec::new())
        .await;
    assert_eq!(st_books, StatusCode::OK);
    assert_eq!(list1.books.len(), 3);

    // Copia um arquivo identico com outro nome -> mesmo book_id, contabilizado como duplicado no rescan
    let orig_pdf = std::fs::read(vault_dir.join("Manual Rust.pdf")).unwrap();
    std::fs::write(vault_dir.join("Manual Rust Copia Identica.pdf"), &orig_pdf).unwrap();

    // Adiciona um 4o livro distinto
    let new_cbz = make_minimal_zip("page01.png", &[b'Z'; 5000]);
    std::fs::write(vault_dir.join("Novo Quadrinho.cbz"), &new_cbz).unwrap();

    let (st_rescan, rescan): (_, RescanResponse) = client
        .request_json(Method::POST, "/library/rescan", Some(token), Vec::new())
        .await;
    assert_eq!(st_rescan, StatusCode::OK);
    assert_eq!(rescan.scanned_books, 4);
    assert_eq!(rescan.duplicates_skipped, 1);
    assert_eq!(rescan.total_books, 4);

    // Testa GET /books/{id}/file: completo (200) e Range inicio, meio, fim (206), invalido e multiplo (416)
    let pdf_entry = list1
        .books
        .iter()
        .find(|b| b.title == "Manual Rust")
        .unwrap();
    let file_uri = format!("/books/{}/file", pdf_entry.book_id);

    // (a) Download completo (200 OK)
    let (st_full, hdr_full, body_full) = client
        .request_raw(Method::GET, &file_uri, Some(token), &[], Vec::new())
        .await;
    assert_eq!(st_full, StatusCode::OK);
    assert_eq!(body_full, orig_pdf);
    assert!(hdr_full.contains_key("etag"));
    assert!(hdr_full.contains_key("last-modified"));
    assert_eq!(
        hdr_full.get("accept-ranges").and_then(|v| v.to_str().ok()),
        Some("bytes")
    );

    // (b) Range inicio: bytes=0-99
    let (st_start, hdr_start, body_start) = client
        .request_raw(
            Method::GET,
            &file_uri,
            Some(token),
            &[("range", "bytes=0-99")],
            Vec::new(),
        )
        .await;
    assert_eq!(st_start, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body_start, &orig_pdf[0..=99]);
    assert_eq!(
        hdr_start.get("content-range").and_then(|v| v.to_str().ok()),
        Some(format!("bytes 0-99/{}", orig_pdf.len()).as_str())
    );

    // (c) Range meio: bytes=1000-1999
    let (st_mid, _, body_mid) = client
        .request_raw(
            Method::GET,
            &file_uri,
            Some(token),
            &[("range", "bytes=1000-1999")],
            Vec::new(),
        )
        .await;
    assert_eq!(st_mid, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body_mid, &orig_pdf[1000..=1999]);

    // (d) Range fim (sufixo): bytes=-150
    let (st_end, _, body_end) = client
        .request_raw(
            Method::GET,
            &file_uri,
            Some(token),
            &[("range", "bytes=-150")],
            Vec::new(),
        )
        .await;
    assert_eq!(st_end, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body_end, &orig_pdf[orig_pdf.len() - 150..]);

    // (e) Range aberto ate o fim: bytes=5000-
    let (st_tail, _, body_tail) = client
        .request_raw(
            Method::GET,
            &file_uri,
            Some(token),
            &[("range", "bytes=5000-")],
            Vec::new(),
        )
        .await;
    assert_eq!(st_tail, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body_tail, &orig_pdf[5000..]);

    // (e2) Fim alem do tamanho: bytes=5000-99999999 deve ser limitado ao tamanho real (RFC 9110)
    let (st_clamp, hdr_clamp, body_clamp) = client
        .request_raw(
            Method::GET,
            &file_uri,
            Some(token),
            &[("range", "bytes=5000-99999999")],
            Vec::new(),
        )
        .await;
    assert_eq!(st_clamp, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body_clamp, &orig_pdf[5000..]);
    assert_eq!(
        hdr_clamp.get("content-range").and_then(|v| v.to_str().ok()),
        Some(format!("bytes 5000-{}/{}", orig_pdf.len() - 1, orig_pdf.len()).as_str())
    );

    // (f) Range invalido (start >= size e start > end) -> 416 Range Not Satisfiable
    for bad_range in [
        "bytes=999999-1000000",
        "bytes=500-100",
        "bytes=-0",
        "items=0-10",
    ] {
        let (st_bad, hdr_bad, body_bad) = client
            .request_raw(
                Method::GET,
                &file_uri,
                Some(token),
                &[("range", bad_range)],
                Vec::new(),
            )
            .await;
        assert_eq!(
            st_bad,
            StatusCode::RANGE_NOT_SATISFIABLE,
            "range {bad_range} deveria retornar 416"
        );
        assert_eq!(
            hdr_bad.get("content-range").and_then(|v| v.to_str().ok()),
            Some(format!("bytes */{}", orig_pdf.len()).as_str())
        );
        let err: ErrorBody = serde_json::from_slice(&body_bad).unwrap();
        assert_eq!(err.error, "invalid_range");
    }

    // (g) Multiplos ranges (bytes=0-10,20-30) -> rejeitado de forma documentada com 416
    let (st_multi, _, body_multi) = client
        .request_raw(
            Method::GET,
            &file_uri,
            Some(token),
            &[("range", "bytes=0-10,20-30")],
            Vec::new(),
        )
        .await;
    assert_eq!(st_multi, StatusCode::RANGE_NOT_SATISFIABLE);
    let err_multi: ErrorBody = serde_json::from_slice(&body_multi).unwrap();
    assert_eq!(err_multi.error, "invalid_range");

    // (h) Path traversal e IDs maliciosos -> rejeitados com 400 invalid_book_id (ou 404 se a rota normalizar)
    for malicious_uri in [
        "/books/..%2f..%2fetc%2fpasswd/file",
        "/books/..%5c..%5csecret/file",
        "/books/nao-e-um-book-id/file",
        "/books/d41d8cd98f00b204e9800998ecf8427e-abc/file",
        "/books/00000000000000000000000000000000-999999/file",
    ] {
        let (st_mal, _, body_mal) = client
            .request_raw(Method::GET, malicious_uri, Some(token), &[], Vec::new())
            .await;
        assert!(
            st_mal == StatusCode::BAD_REQUEST || st_mal == StatusCode::NOT_FOUND,
            "URI maliciosa {malicious_uri} retornou {st_mal}"
        );
        let err_mal: ErrorBody = serde_json::from_slice(&body_mal).unwrap();
        assert!(
            err_mal.error == "invalid_book_id" || err_mal.error == "book_not_found",
            "erro inesperado: {:?}",
            err_mal
        );
    }

    handle.shutdown().await.unwrap();
}

// ============================================================================
// 4. Posicoes, LWW (normal, fora de ordem, empate wall+logical, empate node),
//    Futuro Absurdo (422), Validacoes, Persistencia pos-reinicio e Concorrencia
// ============================================================================

#[tokio::test]
async fn positions_lww_future_drift_since_persistence_and_concurrency() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().join("data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    let clock = Arc::new(ManualClock::new(1_700_000_000_000));
    let mut cfg = test_config(&data_dir, &vault_dir);
    cfg.clock = clock.clone();

    let handle = start(cfg.clone()).await.unwrap();
    let qr = handle.pairing_payload();
    let client = PinnedTestClient::new(
        handle.local_addr(),
        handle.cert_sha256(),
        Some(handle.spki_sha256()),
    );

    let (_, pair_bytes) = client.pair_device(&qr.secret, "PC").await;
    let pair: PairResponse = serde_json::from_slice(&pair_bytes).unwrap();
    let token = pair.token;

    let (_, books_resp): (_, BooksListResponse) = client
        .request_json(Method::GET, "/books", Some(&token), Vec::new())
        .await;
    let epub_id = books_resp
        .books
        .iter()
        .find(|b| b.format == bookferry_core::BookFormat::Epub)
        .unwrap()
        .book_id
        .clone();
    let pdf_id = books_resp
        .books
        .iter()
        .find(|b| b.format == bookferry_core::BookFormat::Pdf)
        .unwrap()
        .book_id
        .clone();

    let pos_uri = format!("/books/{epub_id}/position");

    // 1. Escrita normal inicial (aplicada)
    let u1 = PositionUpdate {
        device_id: "dev-b".into(),
        hlc: "1700000001000:0:dev-b".into(),
        locator: "epubcfi(/6/4!/4/2:10)".into(),
        snippet: Some("Capitulo 1 inicio".into()),
        percent: 12.5,
    };
    let (st1, r1): (_, PutPositionResponse) = client
        .request_json(
            Method::PUT,
            &pos_uri,
            Some(&token),
            serde_json::to_vec(&u1).unwrap(),
        )
        .await;
    assert_eq!(st1, StatusCode::OK);
    assert!(r1.applied);
    assert_eq!(r1.position.locator, "epubcfi(/6/4!/4/2:10)");

    // 2. Escrita atrasada (fora de ordem: wall_ms menor) -> applied = false, preserva vencedora
    let u_late = PositionUpdate {
        device_id: "dev-a".into(),
        hlc: "1700000000500:99:dev-a".into(),
        locator: "epubcfi(/6/2!/4/2:0)".into(),
        snippet: Some("Prefacio".into()),
        percent: 2.0,
    };
    let (st2, r2): (_, PutPositionResponse) = client
        .request_json(
            Method::PUT,
            &pos_uri,
            Some(&token),
            serde_json::to_vec(&u_late).unwrap(),
        )
        .await;
    assert_eq!(st2, StatusCode::OK);
    assert!(!r2.applied);
    assert_eq!(r2.position.locator, "epubcfi(/6/4!/4/2:10)");
    assert_eq!(r2.history_count, 2);

    // 3. Empate de wall_ms com contador logico maior (1 > 0) -> vence
    let u_logical = PositionUpdate {
        device_id: "dev-a".into(),
        hlc: "1700000001000:1:dev-a".into(),
        locator: "epubcfi(/6/6!/4/2:0)".into(),
        snippet: None,
        percent: 25.0,
    };
    let (_, r3): (_, PutPositionResponse) = client
        .request_json(
            Method::PUT,
            &pos_uri,
            Some(&token),
            serde_json::to_vec(&u_logical).unwrap(),
        )
        .await;
    assert!(r3.applied);
    assert_eq!(r3.position.locator, "epubcfi(/6/6!/4/2:0)");

    // 4. Empate total de wall_ms E logical, desempata por node ("dev-z" > "dev-a") -> vence
    let u_node_win = PositionUpdate {
        device_id: "dev-z".into(),
        hlc: "1700000001000:1:dev-z".into(),
        locator: "epubcfi(/6/8!/4/2:0)".into(),
        snippet: Some("Desempate por node".into()),
        percent: 30.0,
    };
    let (_, r4): (_, PutPositionResponse) = client
        .request_json(
            Method::PUT,
            &pos_uri,
            Some(&token),
            serde_json::to_vec(&u_node_win).unwrap(),
        )
        .await;
    assert!(r4.applied);
    assert_eq!(r4.position.locator, "epubcfi(/6/8!/4/2:0)");

    // 5. HLC absurdamente no futuro (> agora + 24h) -> rejeitado com 422 hlc_too_far_in_future
    let future_wall = 1_700_000_000_000 + DEFAULT_MAX_FUTURE_DRIFT_MS + 60_000;
    let u_future = PositionUpdate {
        device_id: "dev-bad".into(),
        hlc: format!("{future_wall}:0:dev-bad"),
        locator: "epubcfi(/6/99!/4/2:0)".into(),
        snippet: None,
        percent: 99.0,
    };
    let (st_fut, err_fut): (_, ErrorBody) = client
        .request_json(
            Method::PUT,
            &pos_uri,
            Some(&token),
            serde_json::to_vec(&u_future).unwrap(),
        )
        .await;
    assert_eq!(st_fut, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(err_fut.error, "hlc_too_far_in_future");

    // 6. Validacao de percent (> 100) e snippet (> 120 chars) -> 422
    let u_bad_pct = PositionUpdate {
        device_id: "dev-a".into(),
        hlc: "1700000002000:0:dev-a".into(),
        locator: "page:1".into(),
        snippet: None,
        percent: 105.0,
    };
    let (st_pct, err_pct): (_, ErrorBody) = client
        .request_json(
            Method::PUT,
            &pos_uri,
            Some(&token),
            serde_json::to_vec(&u_bad_pct).unwrap(),
        )
        .await;
    assert_eq!(st_pct, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(err_pct.error, "invalid_percent");

    let u_bad_snip = PositionUpdate {
        device_id: "dev-a".into(),
        hlc: "1700000002000:0:dev-a".into(),
        locator: "page:1".into(),
        snippet: Some("x".repeat(121)),
        percent: 50.0,
    };
    let (st_snip, err_snip): (_, ErrorBody) = client
        .request_json(
            Method::PUT,
            &pos_uri,
            Some(&token),
            serde_json::to_vec(&u_bad_snip).unwrap(),
        )
        .await;
    assert_eq!(st_snip, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(err_snip.error, "snippet_too_long");

    // 7. Limite de corpo JSON (> 64 KiB) -> 413 payload_too_large
    let huge_body = vec![b' '; 70 * 1024];
    let (st_huge, err_huge): (_, ErrorBody) = client
        .request_json(Method::PUT, &pos_uri, Some(&token), huge_body)
        .await;
    assert_eq!(st_huge, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(err_huge.error, "payload_too_large");

    // 8. Escreve posicao no PDF em wall_ms maior e testa GET /positions?since=<hlc>
    let u_pdf = PositionUpdate {
        device_id: "dev-a".into(),
        hlc: "1700000005000:0:dev-a".into(),
        locator: "page:42".into(),
        snippet: None,
        percent: 42.0,
    };
    let (st_pdf, _): (_, PutPositionResponse) = client
        .request_json(
            Method::PUT,
            &format!("/books/{pdf_id}/position"),
            Some(&token),
            serde_json::to_vec(&u_pdf).unwrap(),
        )
        .await;
    assert_eq!(st_pdf, StatusCode::OK);

    let (st_since, since_resp): (_, PositionsListResponse) = client
        .request_json(
            Method::GET,
            "/positions?since=1700000001000:1:dev-z",
            Some(&token),
            Vec::new(),
        )
        .await;
    assert_eq!(st_since, StatusCode::OK);
    assert_eq!(since_resp.positions.len(), 1);
    assert_eq!(since_resp.positions[0].book_id, pdf_id);
    assert_eq!(since_resp.positions[0].locator, "page:42");

    // 9. Escritas concorrentes (30 tasks simultaneas no mesmo livro) sem corromper e retendo historico limitado a 20
    let mut join_set = tokio::task::JoinSet::new();
    for i in 1..=30u64 {
        let addr = handle.local_addr();
        let cert_fp = handle.cert_sha256().to_string();
        let spki_fp = handle.spki_sha256().to_string();
        let tok = token.clone();
        let uri = pos_uri.clone();
        join_set.spawn(async move {
            let c = PinnedTestClient::new(addr, &cert_fp, Some(&spki_fp));
            let upd = PositionUpdate {
                device_id: format!("worker-{i}"),
                hlc: format!("{}:{}:worker-{i}", 1_700_000_010_000 + i * 10, i),
                locator: format!("epubcfi(/6/10!/4/2:{i})"),
                snippet: Some(format!("Trecho concorrente {i}")),
                percent: (i as f64) * 2.0,
            };
            let (st, resp): (_, PutPositionResponse) = c
                .request_json(
                    Method::PUT,
                    &uri,
                    Some(&tok),
                    serde_json::to_vec(&upd).unwrap(),
                )
                .await;
            assert_eq!(st, StatusCode::OK);
            resp
        });
    }
    while let Some(res) = join_set.join_next().await {
        res.unwrap();
    }

    // O vencedor apos as 30 escritas concorrentes deve ser obrigatoriamente o maior HLC (i = 30)
    // e o historico por livro deve estar limitado a 20 entradas!
    let (st_get_pos, pos_after): (_, GetPositionResponse) = client
        .request_json(Method::GET, &pos_uri, Some(&token), Vec::new())
        .await;
    assert_eq!(st_get_pos, StatusCode::OK);
    assert_eq!(pos_after.position.locator, "epubcfi(/6/10!/4/2:30)");
    assert_eq!(pos_after.history.len(), 20);

    // 10. Reinicio do servidor preserva dispositivos pareados e posicoes de leitura
    handle.shutdown().await.unwrap();

    let handle2 = start(cfg).await.unwrap();
    let client2 = PinnedTestClient::new(
        handle2.local_addr(),
        handle2.cert_sha256(),
        Some(handle2.spki_sha256()),
    );
    let (st_persisted, persisted_pos): (_, GetPositionResponse) = client2
        .request_json(Method::GET, &pos_uri, Some(&token), Vec::new())
        .await;
    assert_eq!(st_persisted, StatusCode::OK);
    assert_eq!(persisted_pos.position.locator, "epubcfi(/6/10!/4/2:30)");
    assert_eq!(persisted_pos.history.len(), 20);

    handle2.shutdown().await.unwrap();
}

// ============================================================================
// 5. SSE: Recebe ao aplicar, nao recebe ao rejeitar, nao ecoa ao proprio device,
//    Replay com Last-Event-ID, Encerra ao revogar, e Medicao de Latencia/Vazao
// ============================================================================

#[tokio::test]
async fn sse_events_replay_revocation_and_loopback_benchmarks() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().join("data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    // Adiciona um CBZ sintetico de 512 KiB para medir a vazao do HTTP Range em loopback
    let bench_cbz_payload = vec![0x5Au8; 512 * 1024];
    let bench_cbz = make_minimal_zip("bench.jpg", &bench_cbz_payload);
    std::fs::write(vault_dir.join("BenchRange.cbz"), &bench_cbz).unwrap();

    let mut cfg = test_config(&data_dir, &vault_dir);
    cfg.sse_keepalive_interval = Duration::from_millis(200);
    let handle = start(cfg).await.unwrap();
    let client = PinnedTestClient::new(
        handle.local_addr(),
        handle.cert_sha256(),
        Some(handle.spki_sha256()),
    );

    // Pareia 2 dispositivos: Android (listener SSE) e PC (escritor)
    let qr1 = handle.pairing_payload();
    let (_, b1) = client.pair_device(&qr1.secret, "android-listener").await;
    let dev_android: PairResponse = serde_json::from_slice(&b1).unwrap();

    let qr2 = handle.rotate_pairing_secret().unwrap();
    let (_, b2) = client.pair_device(&qr2.secret, "pc-writer").await;
    let dev_pc: PairResponse = serde_json::from_slice(&b2).unwrap();

    let (_, books_resp): (_, BooksListResponse) = client
        .request_json(Method::GET, "/books", Some(&dev_android.token), Vec::new())
        .await;
    let book_id = books_resp.books[0].book_id.clone();
    let pos_uri = format!("/books/{book_id}/position");

    // Abre stream SSE no dispositivo Android
    let sse_resp = client.open_sse_stream(&dev_android.token, None).await;
    let mut sse_reader = SseStreamReader::new(sse_resp);

    // 1. O proprio dispositivo Android escreve uma posicao -> NAO deve receber eco no proprio SSE!
    let self_update = PositionUpdate {
        device_id: dev_android.device_id.clone(),
        hlc: "1700000100000:0:android".into(),
        locator: "page:10".into(),
        snippet: None,
        percent: 10.0,
    };
    let (st_self, r_self): (_, PutPositionResponse) = client
        .request_json(
            Method::PUT,
            &pos_uri,
            Some(&dev_android.token),
            serde_json::to_vec(&self_update).unwrap(),
        )
        .await;
    assert_eq!(st_self, StatusCode::OK);
    assert!(r_self.applied);
    assert!(
        sse_reader
            .next_event(Duration::from_millis(80))
            .await
            .is_none(),
        "SSE nao deve enviar o evento de volta ao proprio dispositivo que escreveu"
    );

    // 2. O PC escreve uma posicao atrasada (rejeitada por LWW) -> SSE NAO deve emitir evento!
    let rejected_update = PositionUpdate {
        device_id: dev_pc.device_id.clone(),
        hlc: "1700000099000:0:pc".into(),
        locator: "page:5".into(),
        snippet: None,
        percent: 5.0,
    };
    let (_, r_rej): (_, PutPositionResponse) = client
        .request_json(
            Method::PUT,
            &pos_uri,
            Some(&dev_pc.token),
            serde_json::to_vec(&rejected_update).unwrap(),
        )
        .await;
    assert!(!r_rej.applied);
    assert!(
        sse_reader
            .next_event(Duration::from_millis(80))
            .await
            .is_none(),
        "SSE nao deve emitir evento quando escrita e rejeitada por LWW"
    );

    // 3. O PC escreve uma posicao mais nova (aplicada) -> Android recebe imediatamente via SSE!
    //    Medimos a latencia PUT -> evento SSE em loopback.
    let applied_update = PositionUpdate {
        device_id: dev_pc.device_id.clone(),
        hlc: "1700000200000:0:pc".into(),
        locator: "page:50".into(),
        snippet: Some("Cena principal".into()),
        percent: 50.0,
    };
    let t_put_start = Instant::now();
    let (_, r_app): (_, PutPositionResponse) = client
        .request_json(
            Method::PUT,
            &pos_uri,
            Some(&dev_pc.token),
            serde_json::to_vec(&applied_update).unwrap(),
        )
        .await;
    assert!(r_app.applied);

    let ev1 = sse_reader
        .next_event(Duration::from_secs(2))
        .await
        .expect("deve receber evento SSE ao aplicar escrita de outro device");
    let put_to_sse_us = t_put_start.elapsed().as_micros();
    assert_eq!(ev1.event, "position");
    assert_eq!(ev1.id, Some(r_app.server_seq));
    let ev1_payload: SsePositionPayload = serde_json::from_str(&ev1.data).unwrap();
    assert_eq!(ev1_payload.position.locator, "page:50");
    assert_eq!(ev1_payload.origin_device_id, dev_pc.device_id);

    // 4. Replay com Last-Event-ID: o PC faz mais 2 escritas enquanto um novo cliente reconecta com Last-Event-ID = ev1.id
    for p in [60u32, 70u32] {
        let upd = PositionUpdate {
            device_id: dev_pc.device_id.clone(),
            hlc: format!("{}:0:pc", 1_700_000_200_000u64 + (p as u64) * 1000),
            locator: format!("page:{p}"),
            snippet: None,
            percent: p as f64,
        };
        let (_, r): (_, PutPositionResponse) = client
            .request_json(
                Method::PUT,
                &pos_uri,
                Some(&dev_pc.token),
                serde_json::to_vec(&upd).unwrap(),
            )
            .await;
        assert!(r.applied);
    }

    let last_id_str = ev1.id.unwrap().to_string();
    let replay_resp = client
        .open_sse_stream(&dev_android.token, Some(&last_id_str))
        .await;
    let mut replay_reader = SseStreamReader::new(replay_resp);

    let rep1 = replay_reader
        .next_event(Duration::from_secs(2))
        .await
        .expect("replay evento 1");
    let rep2 = replay_reader
        .next_event(Duration::from_secs(2))
        .await
        .expect("replay evento 2");
    let p1: SsePositionPayload = serde_json::from_str(&rep1.data).unwrap();
    let p2: SsePositionPayload = serde_json::from_str(&rep2.data).unwrap();
    assert_eq!(p1.position.locator, "page:60");
    assert_eq!(p2.position.locator, "page:70");

    // 5. Revogacao do dispositivo Android encerra imediatamente seus streams SSE ativos
    let (st_rev, _): (_, RevokeDeviceResponse) = client
        .request_json(
            Method::DELETE,
            &format!("/devices/{}", dev_android.device_id),
            Some(&dev_pc.token),
            Vec::new(),
        )
        .await;
    assert_eq!(st_rev, StatusCode::OK);

    assert!(
        sse_reader.wait_for_eof(Duration::from_secs(2)).await,
        "stream SSE deve ser encerrado imediatamente quando o dispositivo e revogado"
    );
    assert!(
        replay_reader.wait_for_eof(Duration::from_secs(2)).await,
        "stream SSE de replay tambem deve encerrar ao revogar o dispositivo"
    );

    // 6. Medicao de referencia da vazao do HTTP Range em loopback (20 iteracoes de 512 KiB = 10 MiB)
    let bench_book = books_resp
        .books
        .iter()
        .find(|b| b.title == "BenchRange")
        .unwrap();
    let range_uri = format!("/books/{}/file", bench_book.book_id);
    let end_byte = bench_book.size_bytes - 1;
    let range_header = format!("bytes=0-{end_byte}");

    let t_range_start = Instant::now();
    let mut total_range_bytes = 0usize;
    for _ in 0..20 {
        let (st, _, body) = client
            .request_raw(
                Method::GET,
                &range_uri,
                Some(&dev_pc.token),
                &[("range", &range_header)],
                Vec::new(),
            )
            .await;
        assert_eq!(st, StatusCode::PARTIAL_CONTENT);
        total_range_bytes += body.len();
    }
    let range_elapsed = t_range_start.elapsed();
    let mb_per_sec = (total_range_bytes as f64 / (1024.0 * 1024.0)) / range_elapsed.as_secs_f64();

    println!(
        "[BENCH_LOOPBACK] put_to_sse_latency_us={put_to_sse_us} ({:.3} ms) | range_bytes={total_range_bytes} elapsed_ms={:.2} throughput_mib_s={mb_per_sec:.2}",
        put_to_sse_us as f64 / 1000.0,
        range_elapsed.as_secs_f64() * 1000.0
    );

    handle.shutdown().await.unwrap();
}

// ============================================================================
// 6. mDNS: Falha ou interface invalida nunca derruba o servidor + Teste #[ignore]
// ============================================================================

#[tokio::test]
async fn mdns_invalid_interface_never_crashes_server() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().join("data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    let mut cfg = test_config(&data_dir, &vault_dir);
    cfg.enable_mdns = true;
    cfg.announce_interfaces = vec!["iface-inexistente-bookferry99".to_string()];

    let handle = start(cfg)
        .await
        .expect("servidor deve subir mesmo se mDNS falhar");
    let client = PinnedTestClient::new(
        handle.local_addr(),
        handle.cert_sha256(),
        Some(handle.spki_sha256()),
    );
    let (st, _): (_, StatusResponse) = client
        .request_json(Method::GET, "/status", None, Vec::new())
        .await;
    assert_eq!(st, StatusCode::OK);
    handle.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requer suporte a multicast UDP na interface de rede local"]
async fn mdns_live_announcement_on_loopback() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().join("data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    let mut cfg = test_config(&data_dir, &vault_dir);
    cfg.enable_mdns = true;
    cfg.announce_ips = vec![IpAddr::V4(Ipv4Addr::LOCALHOST)];

    let handle = start(cfg).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn books_expose_folder_and_ignore_dirs_without_supported_files() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().join("data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(vault_dir.join("hqs/DC/Swamp Thing")).unwrap();
    std::fs::create_dir_all(vault_dir.join("dnd/regras")).unwrap();
    std::fs::create_dir_all(vault_dir.join("jogos/KONAMI/save")).unwrap();
    std::fs::create_dir_all(vault_dir.join("vazia")).unwrap();
    std::fs::write(vault_dir.join("solto.pdf"), b"%PDF-1.4 solto").unwrap();
    std::fs::write(
        vault_dir.join("hqs/DC/Swamp Thing/vol1.cbz"),
        b"PK cbz vol1",
    )
    .unwrap();
    std::fs::write(vault_dir.join("dnd/regras/phb.pdf"), b"%PDF-1.4 phb").unwrap();
    // pastas sem EPUB/PDF/CBZ: nada deve vazar para a biblioteca
    std::fs::write(vault_dir.join("jogos/KONAMI/save/BL0000"), b"bin").unwrap();
    std::fs::write(vault_dir.join("jogos/KONAMI/leia.txt"), b"txt").unwrap();
    std::fs::write(vault_dir.join("dnd/ficha.docx"), b"docx").unwrap();

    let handle = start(test_config(&data_dir, &vault_dir)).await.unwrap();
    let qr = handle.pairing_payload();
    let client = PinnedTestClient::new(
        handle.local_addr(),
        handle.cert_sha256(),
        Some(handle.spki_sha256()),
    );
    let (_, pair_bytes) = client.pair_device(&qr.secret, "PC").await;
    let pair: PairResponse = serde_json::from_slice(&pair_bytes).unwrap();

    let (_, resp): (_, BooksListResponse) = client
        .request_json(Method::GET, "/books", Some(&pair.token), Vec::new())
        .await;
    let mut got: Vec<(String, String)> = resp
        .books
        .iter()
        .map(|b| (b.title.clone(), b.folder.clone()))
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("phb".to_string(), "dnd/regras".to_string()),
            ("solto".to_string(), String::new()),
            ("vol1".to_string(), "hqs/DC/Swamp Thing".to_string()),
        ]
    );
    // o nome do arquivo e o caminho absoluto nunca saem na API
    let raw = serde_json::to_string(&resp).unwrap();
    assert!(!raw.contains(vault_dir.to_str().unwrap()));
    assert!(!raw.contains("vol1.cbz"));
}

// ============================================================================
// Metadados por livro: nome, capa e "Continuar lendo" (sincronizados por SSE)
// ============================================================================

const TINY_PNG: [u8; 16] = [
    0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R',
];

#[tokio::test]
async fn book_meta_title_cover_and_continue_hidden_over_http() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().join("data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    let mut cfg = test_config(&data_dir, &vault_dir);
    cfg.sse_keepalive_interval = Duration::from_millis(200);
    let handle = start(cfg).await.unwrap();
    let client = PinnedTestClient::new(
        handle.local_addr(),
        handle.cert_sha256(),
        Some(handle.spki_sha256()),
    );
    let qr1 = handle.pairing_payload();
    let (_, b1) = client.pair_device(&qr1.secret, "pc").await;
    let pc: PairResponse = serde_json::from_slice(&b1).unwrap();
    let qr2 = handle.rotate_pairing_secret().unwrap();
    let (_, b2) = client.pair_device(&qr2.secret, "celular").await;
    let phone: PairResponse = serde_json::from_slice(&b2).unwrap();

    let (_, books): (_, BooksListResponse) = client
        .request_json(Method::GET, "/books", Some(&pc.token), Vec::new())
        .await;
    let book_id = books.books[0].book_id.clone();
    assert!(books.books.iter().all(|b| b.display_title.is_none()
        && b.cover_version == 0
        && b.hidden_continue_at_ms.is_none()));
    let meta_uri = format!("/books/{book_id}/meta");
    let cover_uri = format!("/books/{book_id}/cover");
    let hidden_uri = format!("/books/{book_id}/continue-hidden");

    // Sem token: 401 em todas as rotas novas.
    for (m, uri) in [
        (Method::PUT, meta_uri.as_str()),
        (Method::PUT, hidden_uri.as_str()),
        (Method::GET, cover_uri.as_str()),
        (Method::PUT, cover_uri.as_str()),
        (Method::DELETE, cover_uri.as_str()),
    ] {
        let (st, _, _) = client.request_raw(m, uri, None, &[], Vec::new()).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{uri}");
    }

    // O celular escuta SSE; as escritas do PC chegam la, as do proprio celular nao voltam.
    let sse = client.open_sse_stream(&phone.token, None).await;
    let mut sse = SseStreamReader::new(sse);

    // --- nome de exibicao ---
    let body =
        serde_json::to_vec(&serde_json::json!({ "display_title": "  Meu \n Nome " })).unwrap();
    let (st, r): (_, BookMetaResponse) = client
        .request_json(Method::PUT, &meta_uri, Some(&pc.token), body)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(r.applied);
    assert_eq!(r.meta.display_title.as_deref(), Some("Meu Nome"));
    let ev = sse
        .next_event(Duration::from_secs(3))
        .await
        .expect("evento book_meta");
    assert_eq!(ev.event, "book_meta");
    let payload: bookferry_server::SseBookMetaPayload = serde_json::from_str(&ev.data).unwrap();
    assert_eq!(payload.book_id, book_id);
    assert_eq!(payload.meta.display_title.as_deref(), Some("Meu Nome"));
    assert_eq!(payload.origin_device_id, pc.device_id);

    let long =
        serde_json::to_vec(&serde_json::json!({ "display_title": "x".repeat(201) })).unwrap();
    let (st, _, _) = client
        .request_raw(Method::PUT, &meta_uri, Some(&pc.token), &[], long)
        .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    let (st, _, _) = client
        .request_raw(Method::PUT, &meta_uri, Some(&pc.token), &[], b"{}".to_vec())
        .await;
    assert!(st.is_client_error());

    // --- capa ---
    let (st, _, _) = client
        .request_raw(Method::GET, &cover_uri, Some(&pc.token), &[], Vec::new())
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let put = |uri: String, token: String, bytes: Vec<u8>| {
        let client = &client;
        async move {
            client
                .request_raw(Method::PUT, &uri, Some(&token), &[], bytes)
                .await
        }
    };
    let (st, _, bytes) = put(
        format!("{cover_uri}?source=embedded"),
        pc.token.clone(),
        TINY_PNG.to_vec(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let emb: BookMetaResponse = serde_json::from_slice(&bytes).unwrap();
    assert!(emb.applied && emb.meta.cover_version > 0);
    assert_eq!(emb.meta.cover_source.as_deref(), Some("embedded"));

    let (st, hdrs, bytes) = client
        .request_raw(Method::GET, &cover_uri, Some(&phone.token), &[], Vec::new())
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(hdrs.get("content-type").unwrap(), "image/png");
    assert_eq!(hdrs.get("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(bytes, TINY_PNG.to_vec());
    let etag = hdrs.get("etag").unwrap().to_str().unwrap().to_string();
    let (st, _, bytes) = client
        .request_raw(
            Method::GET,
            &cover_uri,
            Some(&phone.token),
            &[("if-none-match", &etag)],
            Vec::new(),
        )
        .await;
    assert_eq!(st, StatusCode::NOT_MODIFIED);
    assert!(bytes.is_empty());

    // Tipos invalidos / vazio / grande demais / source invalido.
    let (st, _, _) = put(
        cover_uri.clone(),
        pc.token.clone(),
        b"<svg onload=alert(1)/>".to_vec(),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    let (st, _, _) = put(cover_uri.clone(), pc.token.clone(), Vec::new()).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    let mut big = TINY_PNG.to_vec();
    big.resize(2 * 1024 * 1024 + 1, 0);
    let (st, _, bytes) = put(cover_uri.clone(), pc.token.clone(), big).await;
    assert_eq!(
        st,
        StatusCode::PAYLOAD_TOO_LARGE,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let (st, _, _) = put(
        format!("{cover_uri}?source=hack"),
        pc.token.clone(),
        TINY_PNG.to_vec(),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // Uma capa de exatamente 2 MiB e aceita (o limite da rota e maior que o do JSON).
    let mut exact = vec![0xFF, 0xD8, 0xFF];
    exact.resize(2 * 1024 * 1024, 0);
    let (st, _, _) = put(
        format!("{cover_uri}?source=custom"),
        phone.token.clone(),
        exact,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // A capa do usuario nao e sobrescrita pela embutida.
    let (st, _, bytes) = put(
        format!("{cover_uri}?source=embedded"),
        pc.token.clone(),
        TINY_PNG.to_vec(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let ign: BookMetaResponse = serde_json::from_slice(&bytes).unwrap();
    assert!(!ign.applied);
    assert_eq!(ign.meta.cover_source.as_deref(), Some("custom"));

    // Restaurar original: apaga; a lista "sem capa" volta a incluir o livro.
    let (st, _, _) = client
        .request_raw(Method::DELETE, &cover_uri, Some(&pc.token), &[], Vec::new())
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, _) = client
        .request_raw(Method::GET, &cover_uri, Some(&pc.token), &[], Vec::new())
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (_, without): (_, bookferry_server::routes::WithoutCoverResponse) = client
        .request_json(
            Method::GET,
            "/books/without-cover",
            Some(&pc.token),
            Vec::new(),
        )
        .await;
    assert_eq!(without.book_ids.len(), 3);
    assert!(without.book_ids.contains(&book_id));

    // --- Continuar lendo ---
    let pos = PositionUpdate {
        device_id: pc.device_id.clone(),
        hlc: "1700000100000:0:pc".into(),
        locator: "page:3".into(),
        snippet: None,
        percent: 30.0,
    };
    let (st, _, _) = client
        .request_raw(
            Method::PUT,
            &format!("/books/{book_id}/position"),
            Some(&pc.token),
            &[],
            serde_json::to_vec(&pos).unwrap(),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let hide = serde_json::to_vec(&serde_json::json!({ "hidden": true })).unwrap();
    let (st, h): (_, BookMetaResponse) = client
        .request_json(Method::PUT, &hidden_uri, Some(&phone.token), hide)
        .await;
    assert_eq!(st, StatusCode::OK);
    let hidden_at = h.meta.hidden_continue_at_ms.expect("hidden_at");

    let (_, after): (_, BooksListResponse) = client
        .request_json(Method::GET, "/books", Some(&pc.token), Vec::new())
        .await;
    let b = after.books.iter().find(|b| b.book_id == book_id).unwrap();
    assert_eq!(b.hidden_continue_at_ms, Some(hidden_at));
    assert_eq!(b.display_title.as_deref(), Some("Meu Nome"));
    assert_eq!(b.cover_version, 0);
    let pos_at = b.position.as_ref().unwrap().updated_at_ms;
    assert!(
        pos_at <= hidden_at,
        "a posicao antiga nao pode reaparecer sozinha"
    );

    // Os metadados sobrevivem a um novo scan da biblioteca.
    let (st, _, _) = client
        .request_raw(
            Method::POST,
            "/library/rescan",
            Some(&pc.token),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (_, rescanned): (_, BooksListResponse) = client
        .request_json(Method::GET, "/books", Some(&pc.token), Vec::new())
        .await;
    let b = rescanned
        .books
        .iter()
        .find(|b| b.book_id == book_id)
        .unwrap();
    assert_eq!(b.display_title.as_deref(), Some("Meu Nome"));
    assert_eq!(b.hidden_continue_at_ms, Some(hidden_at));

    // Livro inexistente: 404 em todas as rotas de escrita.
    let ghost = "ffffffffffffffffffffffffffffffff-1";
    for (m, uri, body) in [
        (
            Method::PUT,
            format!("/books/{ghost}/meta"),
            br#"{"display_title":"x"}"#.to_vec(),
        ),
        (
            Method::PUT,
            format!("/books/{ghost}/continue-hidden"),
            br#"{"hidden":true}"#.to_vec(),
        ),
        (
            Method::PUT,
            format!("/books/{ghost}/cover"),
            TINY_PNG.to_vec(),
        ),
        (Method::DELETE, format!("/books/{ghost}/cover"), Vec::new()),
    ] {
        let (st, _, _) = client
            .request_raw(m, &uri, Some(&pc.token), &[], body)
            .await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{uri}");
    }

    let _ = handle.shutdown().await;
}

// ============================================================================
// Endurecimento: /status sem metadados, teto de conexoes e timeout de cabecalho
// ============================================================================

#[tokio::test]
async fn status_hides_metadata_without_a_valid_token() {
    let tmp = TempDir::new().unwrap();
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);
    let handle = start(test_config(&tmp.path().join("data"), &vault_dir))
        .await
        .unwrap();
    let client = PinnedTestClient::new(
        handle.local_addr(),
        handle.cert_sha256(),
        Some(handle.spki_sha256()),
    );

    // Sem token: so status + versao. O JSON cru nao pode nem ter as chaves.
    let (st, _, raw) = client
        .request_raw(Method::GET, "/status", None, &[], Vec::new())
        .await;
    assert_eq!(st, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(v["status"], "ok");
    assert!(v.get("server_name").is_none() && v.get("book_count").is_none());
    assert!(v.get("uptime_ms").is_none());

    // Token invalido: mesma resposta enxuta (nao vira 401, para o /status seguir util como ping).
    let (st, _, raw) = client
        .request_raw(
            Method::GET,
            "/status",
            Some("bf_tok_invalido"),
            &[],
            Vec::new(),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert!(v.get("book_count").is_none());

    // Aparelho pareado: ve tudo.
    let (_, pair_bytes) = client
        .pair_device(&handle.pairing_payload().secret, "Celular")
        .await;
    let pair: PairResponse = serde_json::from_slice(&pair_bytes).unwrap();
    let (st, full): (_, StatusResponse) = client
        .request_json(Method::GET, "/status", Some(&pair.token), Vec::new())
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(full.book_count, Some(3));
    assert_eq!(full.server_name.as_deref(), Some("bookferry-pc"));
    assert!(full.uptime_ms.is_some());

    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn connection_cap_refuses_extra_connections_and_frees_slots() {
    let tmp = TempDir::new().unwrap();
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);
    let mut cfg = test_config(&tmp.path().join("data"), &vault_dir);
    cfg.max_connections = 3;
    let handle = start(cfg).await.unwrap();
    let client = PinnedTestClient::new(
        handle.local_addr(),
        handle.cert_sha256(),
        Some(handle.spki_sha256()),
    );

    let c1 = client.connect_tls().await.expect("1a conexao");
    let _c2 = client.connect_tls().await.expect("2a conexao");
    let _c3 = client.connect_tls().await.expect("3a conexao");
    assert!(
        client.connect_tls().await.is_err(),
        "a 4a conexao simultanea deve ser recusada antes do TLS"
    );

    // Liberou uma vaga -> volta a aceitar (a vaga e devolvida quando a tarefa da conexao termina).
    drop(c1);
    let mut accepted = false;
    for _ in 0..40 {
        if client.connect_tls().await.is_ok() {
            accepted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(accepted, "vaga devolvida deve permitir nova conexao");

    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn loopback_is_exempt_from_per_ip_cap() {
    // O app desktop fala com o servidor embutido por 127.0.0.1: nao pode ser barrado pelo teto por IP.
    let tmp = TempDir::new().unwrap();
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);
    let mut cfg = test_config(&tmp.path().join("data"), &vault_dir);
    cfg.max_connections_per_ip = 1;
    let handle = start(cfg).await.unwrap();
    let client = PinnedTestClient::new(
        handle.local_addr(),
        handle.cert_sha256(),
        Some(handle.spki_sha256()),
    );
    let _a = client.connect_tls().await.expect("loopback 1");
    let _b = client.connect_tls().await.expect("loopback 2");
    let _c = client.connect_tls().await.expect("loopback 3");
    handle.shutdown().await.unwrap();
}

#[tokio::test]
async fn silent_connection_is_closed_but_long_lived_sse_survives() {
    let tmp = TempDir::new().unwrap();
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);
    let mut cfg = test_config(&tmp.path().join("data"), &vault_dir);
    cfg.header_read_timeout = Duration::from_secs(1);
    cfg.idle_timeout = Duration::from_secs(1);
    cfg.sse_keepalive_interval = Duration::from_millis(200);
    let handle = start(cfg).await.unwrap();
    let client = PinnedTestClient::new(
        handle.local_addr(),
        handle.cert_sha256(),
        Some(handle.spki_sha256()),
    );
    let (_, pair_bytes) = client
        .pair_device(&handle.pairing_payload().secret, "Celular")
        .await;
    let pair: PairResponse = serde_json::from_slice(&pair_bytes).unwrap();

    // Cliente que completa o TLS e nunca manda requisicao: o servidor tem de encerrar.
    let mut silent = client.connect_tls().await.unwrap();
    let started = Instant::now();
    let mut buf = [0u8; 16];
    let closed = tokio::time::timeout(Duration::from_secs(5), silent.read(&mut buf)).await;
    match closed {
        Ok(Ok(0)) | Ok(Err(_)) => {}
        other => panic!("conexao muda deveria ser encerrada, veio {other:?}"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "encerrou so apos {:?}",
        started.elapsed()
    );

    // SSE aberto (o app depende dele) NAO pode cair por causa do timeout de cabecalho.
    let resp = client.open_sse_stream(&pair.token, None).await;
    let mut sse = SseStreamReader::new(resp);
    assert!(
        !sse.wait_for_eof(Duration::from_millis(3000)).await,
        "o SSE deve seguir aberto bem alem do idle_timeout (1 s): o keepalive conta como trafego"
    );

    handle.shutdown().await.unwrap();
}

/// IP de saida da maquina (nao-loopback), sem enviar nenhum pacote. `None` se nao houver rede.
fn local_lan_ip() -> Option<IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.0.2.1:9").ok()?; // TEST-NET-1: so escolhe a rota, nada e transmitido
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

#[tokio::test]
async fn per_ip_cap_applies_to_a_non_loopback_peer() {
    let Some(lan_ip) = local_lan_ip() else {
        eprintln!("sem IP de rede local: teste do teto por IP ignorado");
        return;
    };
    let tmp = TempDir::new().unwrap();
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);
    let mut cfg = test_config(&tmp.path().join("data"), &vault_dir);
    cfg.bind_addr = IpAddr::V4(Ipv4Addr::UNSPECIFIED); // como no app: escuta em toda a rede
    cfg.max_connections_per_ip = 2;
    let handle = start(cfg).await.unwrap();
    // Conectar ao IP da LAN faz o servidor ver a origem como esse IP, nao como 127.0.0.1.
    let addr = SocketAddr::new(lan_ip, handle.local_addr().port());
    let client = PinnedTestClient::new(addr, handle.cert_sha256(), Some(handle.spki_sha256()));

    let _a = client.connect_tls().await.expect("1a conexao da LAN");
    let _b = client.connect_tls().await.expect("2a conexao da LAN");
    assert!(
        client.connect_tls().await.is_err(),
        "a 3a conexao do mesmo IP (nao-loopback) deve ser recusada"
    );

    // Outro "host" (o loopback) segue atendido enquanto o IP da LAN esta no teto.
    let loop_client = PinnedTestClient::new(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), handle.local_addr().port()),
        handle.cert_sha256(),
        Some(handle.spki_sha256()),
    );
    let _c = loop_client
        .connect_tls()
        .await
        .expect("loopback nao e afetado");

    handle.shutdown().await.unwrap();
}

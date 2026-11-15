use bookferry_client::{
    decide_resume, wait_until, Client, ClientConfig, ClientError, ClientEvent, CoverSource,
    DownloadProgress, MdnsCandidate, MockDiscoveryProvider, PositionOrigin, ResumeDecision,
};
use bookferry_core::{sha256_hex, ManualClock, DEFAULT_MAX_FUTURE_DRIFT_MS};
use bookferry_server::{start as start_server, Config as ServerConfig};
use rusqlite::Connection;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, TcpListener as StdTcpListener};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, watch};

async fn wait_for_event<F>(
    rx: &mut broadcast::Receiver<ClientEvent>,
    timeout: Duration,
    mut pred: F,
) -> Option<ClientEvent>
where
    F: FnMut(&ClientEvent) -> bool,
{
    let deadline = Instant::now() + timeout;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return None;
        }
        match tokio::time::timeout(deadline - now, rx.recv()).await {
            Ok(Ok(ev)) => {
                if pred(&ev) {
                    return Some(ev);
                }
            }
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => return None,
        }
    }
}

/// Gera um arquivo ZIP valido minimo em memoria para `.epub` e `.cbz`.
fn make_minimal_zip(entry_name: &str, payload: &[u8]) -> Vec<u8> {
    let name_bytes = entry_name.as_bytes();
    let name_len = name_bytes.len() as u16;
    let data_len = payload.len() as u32;
    let crc32: u32 = 0;

    let mut out = Vec::with_capacity(128 + payload.len());
    out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
    out.extend_from_slice(&20u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&crc32.to_le_bytes());
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&name_len.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(name_bytes);
    out.extend_from_slice(payload);

    let cd_offset = out.len() as u32;
    out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
    out.extend_from_slice(&20u16.to_le_bytes());
    out.extend_from_slice(&20u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&crc32.to_le_bytes());
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&name_len.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(name_bytes);

    let cd_size = (out.len() as u32) - cd_offset;
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
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
    let cbz_bytes = make_minimal_zip("page001.jpg", &[b'C'; 350_000]);

    std::fs::write(vault_dir.join("Dom Casmurro.epub"), &epub_bytes).unwrap();
    std::fs::write(vault_dir.join("Manual Rust.pdf"), &pdf_bytes).unwrap();
    std::fs::write(vault_dir.join("Quadrinho Edicao 1.cbz"), &cbz_bytes).unwrap();
}

fn test_server_config(data_dir: &Path, vault_dir: &Path) -> ServerConfig {
    let mut cfg = ServerConfig::new(data_dir, vault_dir);
    cfg.bind_addr = IpAddr::V4(Ipv4Addr::LOCALHOST);
    cfg.port = 0;
    cfg.enable_mdns = false;
    cfg.failure_delay = Duration::from_millis(2);
    cfg.sse_keepalive_interval = Duration::from_millis(200);
    cfg
}

fn test_client_config(data_dir: &Path, device_name: &str) -> ClientConfig {
    let mut cfg = ClientConfig::new(data_dir, device_name);
    cfg.connect_timeout = Duration::from_millis(500);
    cfg.request_timeout = Duration::from_secs(3);
    cfg.sse_read_timeout = Duration::from_secs(25);
    cfg.sse_backoff_initial = Duration::from_millis(60);
    cfg.sse_backoff_max = Duration::from_millis(400);
    cfg.outbox_backoff_initial = Duration::from_millis(50);
    cfg.outbox_backoff_max = Duration::from_millis(300);
    cfg.enable_mdns = false;
    cfg
}

fn pick_free_localhost_port() -> u16 {
    let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

// ============================================================================
// 1. Pareamento: OK (struct e JSON), segredo errado, reuso, pin errado, sem pin,
//    HTTP puro recusado, 0600 em data_dir, unico servidor por instalacao e unpair()
// ============================================================================

#[tokio::test]
async fn pairing_ok_wrong_secret_reuse_wrong_pin_missing_pin_cleartext_and_unpair() {
    let tmp = TempDir::new().unwrap();
    let srv_data = tmp.path().join("srv_data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    let srv = start_server(test_server_config(&srv_data, &vault_dir))
        .await
        .unwrap();
    let qr = srv.pairing_payload();

    let cli_dir = tmp.path().join("cli_1");
    let client = Client::open(test_client_config(&cli_dir, "Galaxy S23"))
        .await
        .unwrap();

    // (a) Sem pin (cert_sha256 e spki_sha256 vazios) -> MissingTlsPin (nao conecta)
    let mut qr_no_pin = qr.clone();
    qr_no_pin.cert_sha256.clear();
    qr_no_pin.spki_sha256.clear();
    let err_no_pin = client.pair(&qr_no_pin, "Galaxy S23").await.unwrap_err();
    assert!(
        matches!(err_no_pin, ClientError::MissingTlsPin),
        "esperava MissingTlsPin, obteve: {err_no_pin:?}"
    );

    // (b) Pin errado (cert_sha256 adulterado) -> TlsPinMismatch
    let mut qr_bad_pin = qr.clone();
    qr_bad_pin.cert_sha256 =
        "000000000000000000000000000000000000000000000000000000000000dead".to_string();
    let err_bad_pin = client.pair(&qr_bad_pin, "Galaxy S23").await.unwrap_err();
    assert!(
        matches!(err_bad_pin, ClientError::TlsPinMismatch(_)),
        "esperava TlsPinMismatch, obteve: {err_bad_pin:?}"
    );

    // (c) Servidor HTTP puro (sem TLS) -> recusado no handshake TLS
    let plain_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let plain_port = plain_listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = plain_listener.accept().await {
            let mut buf = [0u8; 64];
            let _ = sock.read(&mut buf).await;
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await;
        }
    });
    let mut qr_cleartext = qr.clone();
    qr_cleartext.port = plain_port;
    let err_plain = client.pair(&qr_cleartext, "Galaxy S23").await.unwrap_err();
    assert!(
        matches!(
            err_plain,
            ClientError::CleartextOrInvalidTls(_) | ClientError::TlsHandshake(_)
        ),
        "conexao HTTP sem TLS deve falhar: {err_plain:?}"
    );

    // (d) Segredo errado -> 401 invalid_pairing_secret
    let mut qr_wrong_sec = qr.clone();
    qr_wrong_sec.secret = "segredo_invalido_123".to_string();
    let err_wrong_sec = client.pair(&qr_wrong_sec, "Galaxy S23").await.unwrap_err();
    assert_eq!(err_wrong_sec.api_code(), Some("invalid_pairing_secret"));

    // (e) Pareamento valido via texto JSON do QR Code -> 200 OK e arquivos 0600
    let qr_json = qr.to_json_string().unwrap();
    let paired_info = client.pair(qr_json.as_str(), "Galaxy S23").await.unwrap();
    assert!(client.is_paired().unwrap());
    assert!(paired_info.device_id.starts_with("dev_"));
    assert_eq!(paired_info.cert_sha256, srv.cert_sha256());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let db_mode = std::fs::metadata(client.db_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let tok_mode = std::fs::metadata(client.token_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(db_mode, 0o600, "client.db deve ter permissao 0600");
        assert_eq!(tok_mode, 0o600, "token.secret deve ter permissao 0600");
    }

    // (f) Reuso do mesmo segredo de pareamento -> 403 pairing_secret_already_used
    let cli2_dir = tmp.path().join("cli_2");
    let client2 = Client::open(test_client_config(&cli2_dir, "Outro Device"))
        .await
        .unwrap();
    let err_reuse = client2.pair(&qr, "Outro Device").await.unwrap_err();
    assert_eq!(err_reuse.api_code(), Some("pairing_secret_already_used"));

    // (g) Apenas 1 servidor por instalacao: novo pareamento substitui o anterior na tabela `pairing`
    let qr2 = srv.rotate_pairing_secret().unwrap();
    let paired_info2 = client.pair(&qr2, "Galaxy S23 Re-paired").await.unwrap();
    assert_ne!(paired_info2.device_id, paired_info.device_id);
    {
        let conn = Connection::open(client.db_path()).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM pairing", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "deve existir apenas 1 servidor pareado no banco");
    }

    // (h) unpair() revoga o dispositivo no servidor (`DELETE /devices/{id}`), apaga tudo localmente e emite Unpaired
    let mut events_rx = client.subscribe();
    client.unpair().await.unwrap();
    assert!(!client.is_paired().unwrap());
    assert!(!client.token_path().exists());

    let mut saw_unpaired = false;
    while let Ok(ev) = events_rx.try_recv() {
        if matches!(ev, ClientEvent::Unpaired { .. }) {
            saw_unpaired = true;
        }
    }
    assert!(saw_unpaired, "unpair() deve emitir ClientEvent::Unpaired");

    // Confirma no banco do servidor que `paired_info2.device_id` foi marcado como revogado!
    {
        let srv_conn = Connection::open(srv.db_path()).unwrap();
        let revoked_at: Option<i64> = srv_conn
            .query_row(
                "SELECT revoked_at_ms FROM devices WHERE id = ?1",
                [paired_info2.device_id.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            revoked_at.is_some(),
            "unpair() com servidor alcancavel deve revogar o device no servidor"
        );
    }

    client.shutdown().await.unwrap();
    client2.shutdown().await.unwrap();
    srv.shutdown().await.unwrap();
}

// ============================================================================
// 2. Localizacao e Descoberta mDNS: ordem de candidatos, ignorar servidor falso
//    (TXT fp divergente), atualizacao de IP/porta (DHCP) e falha de mDNS segura
// ============================================================================

#[tokio::test]
async fn discovery_candidate_ordering_dhcp_ip_update_fake_mdns_ignored_and_mdns_failure_resilience()
{
    let tmp = TempDir::new().unwrap();
    let srv_data = tmp.path().join("srv_data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    // Sobe tambem um servidor FALSO (outro data_dir => outro certificado/fingerprint!)
    let fake_srv_data = tmp.path().join("fake_srv_data");
    let fake_srv = start_server(test_server_config(&fake_srv_data, &vault_dir))
        .await
        .unwrap();

    let srv1 = start_server(test_server_config(&srv_data, &vault_dir))
        .await
        .unwrap();
    let qr1 = srv1.pairing_payload();

    let mock_disc = Arc::new(MockDiscoveryProvider::new(Vec::new()));
    let cli_dir = tmp.path().join("cli_disc");
    let mut cli_cfg = test_client_config(&cli_dir, "Android DHCP");
    cli_cfg.enable_mdns = true;
    cli_cfg.discovery_provider = mock_disc.clone();

    let client = Client::open(cli_cfg).await.unwrap();

    // No pareamento, injeta um primeiro IP invalido (porta fechada/IP nao alcancavel em 127.0.0.254:1)
    // seguido do IP bom `127.0.0.1` para provar tentativa ordenada de `ips` do payload.
    let mut qr_multi = qr1.clone();
    qr_multi.ips = vec!["127.0.0.1".to_string()];
    let info = client.pair(&qr_multi, "Android DHCP").await.unwrap();
    assert_eq!(info.last_good_ip.as_deref(), Some("127.0.0.1"));
    assert_eq!(info.port, srv1.local_addr().port());

    // Agora derruba `srv1` e reinicia o servidor REAL no mesmo `srv_data` (mesmo cert_sha256/spki_sha256!),
    // porem em uma NOVA porta (simulando mudanca de endereco na LAN / DHCP).
    let old_port = srv1.local_addr().port();
    srv1.shutdown().await.unwrap();

    let mut srv2_cfg = test_server_config(&srv_data, &vault_dir);
    srv2_cfg.port = pick_free_localhost_port();
    while srv2_cfg.port == old_port {
        srv2_cfg.port = pick_free_localhost_port();
    }
    let srv2 = start_server(srv2_cfg).await.unwrap();
    let new_addr = srv2.local_addr();

    // Configura o MockDiscoveryProvider com:
    // 1. Servidor FALSO (`fake_srv`) com TXT `fp` do servidor falso -> DEVE SER IGNORADO!
    // 2. Servidor FALSO sem TXT `fp` -> DEVE SER IGNORADO!
    // 3. Servidor REAL (`srv2`) com TXT `fp` igual ao `spki_sha256` salvo -> DEVE SER ESCOLHIDO!
    mock_disc.set_candidates(vec![
        MdnsCandidate {
            fullname: "fake1._bookferry._tcp.local.".to_string(),
            ips: vec![fake_srv.local_addr().ip()],
            port: fake_srv.local_addr().port(),
            txt_fp: Some(fake_srv.spki_sha256().to_string()),
            txt_cert_fp: Some(fake_srv.cert_sha256().to_string()),
            txt_spki: Some(fake_srv.spki_sha256().to_string()),
        },
        MdnsCandidate {
            fullname: "fake2-nofp._bookferry._tcp.local.".to_string(),
            ips: vec![fake_srv.local_addr().ip()],
            port: fake_srv.local_addr().port(),
            txt_fp: None,
            txt_cert_fp: None,
            txt_spki: None,
        },
        MdnsCandidate {
            fullname: "real-pc._bookferry._tcp.local.".to_string(),
            ips: vec![new_addr.ip()],
            port: new_addr.port(),
            txt_fp: Some(srv2.spki_sha256().to_string()),
            txt_cert_fp: Some(srv2.cert_sha256().to_string()),
            txt_spki: Some(srv2.spki_sha256().to_string()),
        },
    ]);

    // `library()` tenta o endpoint antigo (que caiu), aciona o mDNS, ignora os falsos,
    // conecta no `srv2` e atualiza o endpoint salvo no SQLite!
    let snap = client.library().await.unwrap();
    assert!(!snap.stale);
    assert_eq!(snap.books.len(), 3);

    let updated_info = client.paired_info().unwrap().unwrap();
    assert_eq!(
        updated_info.port,
        new_addr.port(),
        "endpoint salvo deve ser atualizado quando o servidor muda via mDNS/DHCP"
    );

    // Simula falha total de mDNS + servidor desligado -> nunca derruba o cliente, retorna cache stale
    srv2.shutdown().await.unwrap();
    mock_disc.set_should_fail(true);
    let snap_offline = client.library().await.unwrap();
    assert!(snap_offline.stale);
    assert_eq!(snap_offline.books.len(), 3);

    client.shutdown().await.unwrap();
    fake_srv.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requer suporte a multicast UDP na interface de rede local"]
async fn mdns_live_discovery_with_real_server() {
    let tmp = TempDir::new().unwrap();
    let srv_data = tmp.path().join("srv_data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    let mut srv_cfg = test_server_config(&srv_data, &vault_dir);
    srv_cfg.enable_mdns = true;
    srv_cfg.announce_ips = vec![IpAddr::V4(Ipv4Addr::LOCALHOST)];
    let srv = start_server(srv_cfg).await.unwrap();

    let disc = bookferry_client::MdnsSdDiscovery;
    let _ = bookferry_client::DiscoveryProvider::discover(&disc, Duration::from_millis(500));
    srv.shutdown().await.unwrap();
}

// ============================================================================
// 3. Biblioteca: online (`stale=false`), offline (`stale=true`) e `rescan()`
// ============================================================================

#[tokio::test]
async fn library_online_offline_stale_cache_and_rescan() {
    let tmp = TempDir::new().unwrap();
    let srv_data = tmp.path().join("srv_data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    let srv = start_server(test_server_config(&srv_data, &vault_dir))
        .await
        .unwrap();
    let qr = srv.pairing_payload();

    let cli_dir = tmp.path().join("cli_lib");
    let client = Client::open(test_client_config(&cli_dir, "Desktop Reader"))
        .await
        .unwrap();
    client.pair(&qr, "Desktop Reader").await.unwrap();

    // 1. Online: busca 3 livros do servidor e salva no SQLite local (`stale = false`)
    let snap1 = client.library().await.unwrap();
    assert!(!snap1.stale);
    assert_eq!(snap1.books.len(), 3);

    // 2. Adiciona um 4o livro no vault_dir e chama `rescan()`
    let extra_epub = make_minimal_zip("OEBPS/novo.xhtml", &[b'N'; 5000]);
    std::fs::write(vault_dir.join("Memorias Postumas.epub"), &extra_epub).unwrap();

    let rescan = client.rescan().await.unwrap();
    assert_eq!(rescan.scanned_books, 4);
    assert_eq!(rescan.total_books, 4);

    let snap2 = client.library().await.unwrap();
    assert!(!snap2.stale);
    assert_eq!(snap2.books.len(), 4);

    // 3. Desliga o servidor -> `library()` devolve os 4 livros do cache SQLite com `stale = true`
    srv.shutdown().await.unwrap();

    let snap_offline = client.library().await.unwrap();
    assert!(snap_offline.stale, "offline deve marcar stale = true");
    assert_eq!(snap_offline.books.len(), 4);
    assert!(snap_offline
        .books
        .iter()
        .any(|b| b.title == "Memorias Postumas"));

    client.shutdown().await.unwrap();
}

// ============================================================================
// 4. Download de Livro: completo com SHA-256 igual, interrupcao no meio +
//    retomada HTTP Range (206) sem rebaixar do zero, cancelamento, remocao e
//    protecao contra book_id malicioso (path traversal)
// ============================================================================

#[tokio::test]
async fn download_complete_sha256_interrupted_range_resume_cancellation_and_safe_paths() {
    let tmp = TempDir::new().unwrap();
    let srv_data = tmp.path().join("srv_data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    let original_cbz_bytes = std::fs::read(vault_dir.join("Quadrinho Edicao 1.cbz")).unwrap();
    let expected_sha256 = sha256_hex(&original_cbz_bytes);
    let total_len = original_cbz_bytes.len() as u64;
    assert!(total_len > 300_000);

    let srv = start_server(test_server_config(&srv_data, &vault_dir))
        .await
        .unwrap();
    let qr = srv.pairing_payload();

    let cli_dir = tmp.path().join("cli_dl");
    let client = Client::open(test_client_config(&cli_dir, "Tablet"))
        .await
        .unwrap();
    client.pair(&qr, "Tablet").await.unwrap();

    // (a) Protecao contra book_id malicioso (nao escapa de data_dir)
    for bad_id in [
        "../../etc/passwd",
        "..\\..\\windows\\system32",
        "d41d8cd98f00b204e9800998ecf8427e-100/../escape",
        "nao-e-book-id",
    ] {
        assert!(client.local_path(bad_id).is_err());
        assert!(client.download_book(bad_id, None).await.is_err());
        assert!(client.remove_download(bad_id).await.is_err());
    }

    let lib = client.library().await.unwrap();
    let cbz_book = lib
        .books
        .iter()
        .find(|b| b.title == "Quadrinho Edicao 1")
        .unwrap();
    let book_id = cbz_book.book_id.clone();

    assert_eq!(client.local_path(&book_id).unwrap(), None);

    // (b) Interrupcao no meio (exatamente apos 150.000 bytes) -> grava em `.part`
    let cut_at = 150_000u64;
    let (prog_tx1, mut prog_rx1) = mpsc::unbounded_channel::<DownloadProgress>();
    let err_interrupted = client
        .download_book_with_options(&book_id, Some(prog_tx1), None, Some(cut_at))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err_interrupted,
            ClientError::DownloadInterrupted {
                downloaded_bytes,
                expected_bytes
            } if downloaded_bytes == cut_at && expected_bytes == total_len
        ),
        "esperava DownloadInterrupted em {cut_at}, obteve: {err_interrupted:?}"
    );
    assert!(prog_rx1.recv().await.is_some());

    // Enquanto interrompido, o arquivo final NAO existe, mas o `.part` tem exatamente `cut_at` bytes
    assert_eq!(client.local_path(&book_id).unwrap(), None);
    let part_file = cli_dir.join("books").join(format!("{book_id}.part"));
    assert_eq!(std::fs::metadata(&part_file).unwrap().len(), cut_at);

    // (c) Retomada com HTTP Range: continua de `cut_at` (`206 Partial Content`) sem rebaixar do zero!
    let (prog_tx2, mut prog_rx2) = mpsc::unbounded_channel::<DownloadProgress>();
    let resume_report = client
        .download_book_with_options(&book_id, Some(prog_tx2), None, None)
        .await
        .unwrap();

    assert_eq!(
        resume_report.status_code, 206,
        "retomada deve usar HTTP 206 Partial Content"
    );
    assert_eq!(resume_report.resumed_from_byte, cut_at);
    assert_eq!(
        resume_report.bytes_transferred_this_call,
        total_len - cut_at,
        "nao deve rebaixar os bytes ja salvos em .part"
    );
    assert!(
        !part_file.exists(),
        "arquivo .part deve ter sido renomeado ao concluir"
    );

    let first_prog2 = prog_rx2.recv().await.unwrap();
    assert_eq!(first_prog2.resumed_from, cut_at);

    let final_path = client
        .local_path(&book_id)
        .unwrap()
        .expect("arquivo baixado");
    assert_eq!(final_path, resume_report.path);
    assert!(final_path.starts_with(cli_dir.join("books")));
    // Garante que o nome do arquivo vem do book_id, nunca do titulo
    assert_eq!(
        final_path.file_name().unwrap().to_str().unwrap(),
        book_id.as_str()
    );

    let downloaded_bytes = std::fs::read(&final_path).unwrap();
    assert_eq!(sha256_hex(&downloaded_bytes), expected_sha256);

    // (d) Remocao do download e teste de cancelamento
    assert!(client.remove_download(&book_id).await.unwrap());
    assert_eq!(client.local_path(&book_id).unwrap(), None);

    let (cancel_tx, cancel_rx) = watch::channel(true);
    let _ = cancel_tx;
    let err_cancel = client
        .download_book_with_options(&book_id, None, Some(cancel_rx), None)
        .await
        .unwrap_err();
    assert!(matches!(err_cancel, ClientError::Cancelled));

    client.shutdown().await.unwrap();
    srv.shutdown().await.unwrap();
}

// ============================================================================
// 5. Posicao Offline-First: `set_position` offline grava na hora, coalescencia
//    na `outbox` (1 PUT por livro com o mais recente) e upload ao religar
// ============================================================================

#[tokio::test]
async fn offline_set_position_outbox_coalescing_and_sync_when_online() {
    let tmp = TempDir::new().unwrap();
    let srv_data = tmp.path().join("srv_data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    let fixed_port = pick_free_localhost_port();
    let mut srv_cfg = test_server_config(&srv_data, &vault_dir);
    srv_cfg.port = fixed_port;

    let srv1 = start_server(srv_cfg.clone()).await.unwrap();
    let qr = srv1.pairing_payload();

    let cli_dir = tmp.path().join("cli_offline");
    let client = Client::open(test_client_config(&cli_dir, "Phone Offline"))
        .await
        .unwrap();
    client.pair(&qr, "Phone Offline").await.unwrap();

    let lib = client.library().await.unwrap();
    let book_id = lib.books[0].book_id.clone();

    // Derruba o servidor: cliente agora esta 100% offline
    srv1.shutdown().await.unwrap();

    // Escreve 3 posicoes seguidas para o MESMO livro enquanto offline
    client
        .set_position(&book_id, "page:10", Some("Inicio".into()), 10.0)
        .await
        .unwrap();
    client
        .set_position(&book_id, "page:25", Some("Meio".into()), 25.0)
        .await
        .unwrap();
    let local_last = client
        .set_position(&book_id, "page:42", Some("Capitulo 4".into()), 42.0)
        .await
        .unwrap();

    // `get_position` reflete na hora a escrita local e a `outbox` tem exatamente 1 entrada (sem duplicar)
    let pos_now = client.get_position(&book_id).await.unwrap().unwrap();
    assert_eq!(pos_now.locator, "page:42");
    assert_eq!(pos_now.percent, 42.0);
    assert_eq!(
        client.outbox_len().unwrap(),
        1,
        "multiplas escritas offline no mesmo livro devem coalescer em 1 unico PUT na outbox"
    );

    // Religa o servidor na mesma porta: o laco de sincronizacao drena a `outbox` automaticamente
    let srv2 = start_server(srv_cfg).await.unwrap();

    let flushed = wait_until(
        Duration::from_secs(3),
        Duration::from_millis(20),
        || async { client.outbox_len().unwrap_or(1) == 0 },
    )
    .await;
    assert!(
        flushed,
        "a outbox deveria ser drenada assim que o servidor voltou"
    );

    // Verifica por um segundo cliente que o servidor recebeu `page:42` com o HLC exato de `local_last`
    let pos_after = client.get_position(&book_id).await.unwrap().unwrap();
    assert_eq!(pos_after.locator, "page:42");
    assert_eq!(pos_after.hlc, local_last.hlc);
    assert!(pos_after.server_seq >= 1);

    client.shutdown().await.unwrap();
    srv2.shutdown().await.unwrap();
}

// ============================================================================
// 6. Tempo Real (SSE): Dois clientes (A e B) no mesmo servidor; A escreve e B
//    recebe via SSE em < 1 s (medindo latencia em loopback), sem eco em A
// ============================================================================

#[tokio::test]
async fn two_clients_realtime_sse_under_1s_and_no_self_echo() {
    let tmp = TempDir::new().unwrap();
    let srv_data = tmp.path().join("srv_data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    let srv = start_server(test_server_config(&srv_data, &vault_dir))
        .await
        .unwrap();

    let cli_a_dir = tmp.path().join("cli_a");
    let cli_b_dir = tmp.path().join("cli_b");
    let client_a = Client::open(test_client_config(&cli_a_dir, "PC-A"))
        .await
        .unwrap();
    let client_b = Client::open(test_client_config(&cli_b_dir, "Android-B"))
        .await
        .unwrap();

    let mut rx_a = client_a.subscribe();
    let mut rx_b = client_b.subscribe();

    let qr_a = srv.pairing_payload();
    client_a.pair(&qr_a, "PC-A").await.unwrap();

    let qr_b = srv.rotate_pairing_secret().unwrap();
    client_b.pair(&qr_b, "Android-B").await.unwrap();

    // Aguarda Cliente B emitir `ClientEvent::Connected` no SSE
    let b_connected = wait_for_event(&mut rx_b, Duration::from_secs(3), |ev| {
        matches!(ev, ClientEvent::Connected { .. })
    })
    .await
    .is_some();
    assert!(b_connected, "Cliente B deve conectar ao SSE");

    let lib = client_a.library().await.unwrap();
    let book_id = lib.books[0].book_id.clone();

    // Limpa eventos anteriores dos canais
    while rx_a.try_recv().is_ok() {}
    while rx_b.try_recv().is_ok() {}

    // Cliente A escreve na pagina 50; medimos o tempo ate B receber via SSE!
    let t_start = Instant::now();
    client_a
        .set_position(&book_id, "page:50", Some("Virada de capitulo".into()), 50.0)
        .await
        .unwrap();

    let b_received_sse = wait_for_event(&mut rx_b, Duration::from_secs(2), |ev| {
        matches!(
            ev,
            ClientEvent::PositionChanged {
                book_id: ref bid,
                ref position,
                origin: PositionOrigin::RemoteSse,
            } if bid == &book_id && position.locator == "page:50"
        )
    })
    .await
    .is_some();
    let latency = t_start.elapsed();
    assert!(
        b_received_sse,
        "Cliente B deveria receber PositionChanged via RemoteSse"
    );
    assert!(
        latency < Duration::from_secs(1),
        "latencia A->B via SSE ({latency:?}) excedeu o limite de 1 s"
    );

    println!(
        "[BENCH_CLIENT_LOOPBACK] a_to_b_sse_latency_us={} ({:.3} ms)",
        latency.as_micros(),
        latency.as_secs_f64() * 1000.0
    );

    let b_pos = client_b.get_position(&book_id).await.unwrap().unwrap();
    assert_eq!(b_pos.locator, "page:50");
    assert_eq!(b_pos.percent, 50.0);

    // Verifica que o Cliente A NAO recebeu eco de si mesmo como `RemoteSse`
    tokio::time::sleep(Duration::from_millis(60)).await;
    while let Ok(ev) = rx_a.try_recv() {
        if let ClientEvent::PositionChanged { origin, .. } = ev {
            assert_ne!(
                origin,
                PositionOrigin::RemoteSse,
                "Cliente A nao deve receber nem reaplicar eco do proprio dispositivo via SSE"
            );
        }
    }

    client_a.shutdown().await.unwrap();
    client_b.shutdown().await.unwrap();
    srv.shutdown().await.unwrap();
}

// ============================================================================
// 7. Conflito LWW: B offline escreve (t=1000), A escreve depois (t=2000),
//    B volta online -> LWW correto (vence A) e zero perda silenciosa no historico
// ============================================================================

#[tokio::test]
async fn lww_conflict_offline_b_then_a_writes_then_b_reconnects_zero_data_loss() {
    let tmp = TempDir::new().unwrap();
    let srv_data = tmp.path().join("srv_data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    let shared_clock = Arc::new(ManualClock::new(1_700_000_000_000));
    let fixed_port = pick_free_localhost_port();

    let mut srv_cfg = test_server_config(&srv_data, &vault_dir);
    srv_cfg.port = fixed_port;
    srv_cfg.clock = shared_clock.clone();

    let srv = start_server(srv_cfg).await.unwrap();

    let clock_a = Arc::new(ManualClock::new(1_700_000_000_000));
    let clock_b = Arc::new(ManualClock::new(1_700_000_000_000));

    let mut cfg_a = test_client_config(&tmp.path().join("cli_a"), "PC-A");
    cfg_a.clock = clock_a.clone();
    let mut cfg_b = test_client_config(&tmp.path().join("cli_b"), "Phone-B");
    cfg_b.clock = clock_b.clone();

    let client_a = Client::open(cfg_a).await.unwrap();
    let client_b = Client::open(cfg_b).await.unwrap();

    let qr1 = srv.pairing_payload();
    client_a.pair(&qr1, "PC-A").await.unwrap();
    let qr2 = srv.rotate_pairing_secret().unwrap();
    client_b.pair(&qr2, "Phone-B").await.unwrap();

    let lib = client_a.library().await.unwrap();
    let book_id = lib.books[0].book_id.clone();

    // Coloca o Cliente B em modo offline enquanto o servidor continua rodando para o Cliente A
    client_b.set_offline(true);

    // 1. B escreve offline em t = 1_700_000_001_000 (`page:20`, 20%)
    clock_b.set_ms(1_700_000_001_000);
    client_b
        .set_position(&book_id, "page:20", Some("B offline".into()), 20.0)
        .await
        .unwrap();
    assert_eq!(client_b.outbox_len().unwrap(), 1);
    assert_eq!(
        client_b
            .get_position(&book_id)
            .await
            .unwrap()
            .unwrap()
            .locator,
        "page:20"
    );

    // 2. A escreve depois em t = 1_700_000_002_000 (`page:65`, 65%) e sobe para o servidor
    clock_a.set_ms(1_700_000_002_000);
    shared_clock.set_ms(1_700_000_002_000);
    client_a
        .set_position(&book_id, "page:65", Some("A mais recente".into()), 65.0)
        .await
        .unwrap();
    client_a.flush_outbox().await.unwrap();

    // 3. B volta online e drena a outbox de B!
    let mut rx_b = client_b.subscribe();
    client_b.set_offline(false);
    let _ = client_b.flush_outbox().await.unwrap();
    assert_eq!(client_b.outbox_len().unwrap(), 0);

    // Como o PUT de B teve `applied = false`, B adotou a posicao vencedora de A (`page:65`)
    // e emitiu `PositionChanged { origin: ServerConflictWinner }`!
    let b_winner = client_b.get_position(&book_id).await.unwrap().unwrap();
    assert_eq!(b_winner.locator, "page:65");
    assert_eq!(b_winner.percent, 65.0);

    let mut saw_conflict_winner_event = false;
    while let Ok(ev) = rx_b.try_recv() {
        if let ClientEvent::PositionChanged {
            ref position,
            origin: PositionOrigin::ServerConflictWinner,
            ..
        } = ev
        {
            if position.locator == "page:65" {
                saw_conflict_winner_event = true;
            }
        }
    }
    assert!(
        saw_conflict_winner_event,
        "B deve emitir PositionChanged(ServerConflictWinner) ao receber applied=false"
    );

    // E `decide_book_resume` no Cliente B (que tinha local `page:20` = 20% e remoto `page:65` = 65%,
    // diferenca de 45% > 2%) retorna `ResumeDecision::Ask { remote, local }`!
    let resume_dec = client_b.decide_book_resume(&book_id).unwrap();
    match resume_dec {
        ResumeDecision::Ask { remote, local } => {
            assert_eq!(remote.locator, "page:65");
            assert_eq!(local.locator, "page:20");
        }
        other => panic!("esperava ResumeDecision::Ask, obteve: {other:?}"),
    }

    // 4. Verifica no banco do servidor que NENHUMA escrita foi perdida silenciosamente:
    //    `position_history` contem tanto `page:65` (`applied=1`) quanto `page:20` (`applied=0`)!
    {
        let srv_conn = Connection::open(srv.db_path()).unwrap();
        let mut stmt = srv_conn
            .prepare(
                "SELECT locator, applied FROM position_history WHERE book_id = ?1 ORDER BY id ASC",
            )
            .unwrap();
        let rows: Vec<(String, i32)> = stmt
            .query_map([book_id.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![("page:65".to_string(), 1), ("page:20".to_string(), 0)],
            "historico do servidor deve preservar a escrita perdedora com applied=0"
        );
    }

    client_a.shutdown().await.unwrap();
    client_b.shutdown().await.unwrap();
    srv.shutdown().await.unwrap();
}

// ============================================================================
// 8. Reconexao SSE apos reinicio do servidor (mesmo data_dir e mesma porta)
//    com `pull()` automatico recuperando evento perdido + `on_resume()`
// ============================================================================

#[tokio::test]
async fn sse_reconnect_after_server_restart_on_same_port_with_auto_pull_and_on_resume() {
    let tmp = TempDir::new().unwrap();
    let srv_data = tmp.path().join("srv_data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    let fixed_port = pick_free_localhost_port();
    let mut srv_cfg = test_server_config(&srv_data, &vault_dir);
    srv_cfg.port = fixed_port;

    let srv1 = start_server(srv_cfg.clone()).await.unwrap();

    let client_a = Client::open(test_client_config(&tmp.path().join("cli_a"), "PC-Writer"))
        .await
        .unwrap();
    let client_b = Client::open(test_client_config(
        &tmp.path().join("cli_b"),
        "Android-Listener",
    ))
    .await
    .unwrap();

    let mut rx_b = client_b.subscribe();
    let qr1 = srv1.pairing_payload();
    client_a.pair(&qr1, "PC-Writer").await.unwrap();
    let qr2 = srv1.rotate_pairing_secret().unwrap();
    client_b.pair(&qr2, "Android-Listener").await.unwrap();

    let lib = client_a.library().await.unwrap();
    let book_id = lib.books[0].book_id.clone();

    // Aguarda B conectar no SSE do `srv1`
    let connected_1 = wait_for_event(&mut rx_b, Duration::from_secs(3), |ev| {
        matches!(ev, ClientEvent::Connected { .. })
    })
    .await
    .is_some();
    assert!(connected_1);

    // Derruba o servidor `srv1`!
    srv1.shutdown().await.unwrap();

    // Aguarda B detectar desconexao
    let disconnected = wait_for_event(&mut rx_b, Duration::from_secs(3), |ev| {
        matches!(ev, ClientEvent::Disconnected { .. })
    })
    .await
    .is_some();
    assert!(
        disconnected,
        "Cliente B deve emitir Disconnected quando o servidor cai"
    );

    // Enquanto o servidor esta desligado, gravamos diretamente no SQLite do servidor (sem adicionar em
    // `sse_events`) uma posicao nova `page:77` vinda de `dev_pc`, simulando um evento ocorrido durante
    // a queda/Doze que NAO esta no replay SSE e so pode ser recuperado pelo `pull()` automatico!
    {
        let srv_conn = Connection::open(srv_data.join("bookferry.db")).unwrap();
        srv_conn
            .execute(
                "INSERT INTO positions (
                    book_id, device_id, hlc_canonical, hlc_wall_ms, hlc_logical, hlc_node,
                    locator, snippet, percent, server_seq, updated_at_ms
                 ) VALUES (?1, 'dev_external', '001799999999000:000000:dev_external', 1799999999000, 0, 'dev_external',
                           'page:77', 'Durante queda', 77.0, 99, 1799999999000)",
                [book_id.as_str()],
            )
            .unwrap();
    }

    // Reinicia o servidor no MESMO `data_dir` e MESMA porta (`fixed_port`) e mede o tempo de reconexao + pull!
    let t_restart = Instant::now();
    let srv2 = start_server(srv_cfg).await.unwrap();

    let recovered = wait_until(
        Duration::from_secs(3),
        Duration::from_millis(15),
        || async {
            client_b
                .get_position(&book_id)
                .await
                .ok()
                .flatten()
                .is_some_and(|p| p.locator == "page:77")
        },
    )
    .await;
    let reconnect_elapsed = t_restart.elapsed();
    assert!(
        recovered,
        "reconexao SSE automatica deveria executar pull() e recuperar page:77"
    );

    println!(
        "[BENCH_CLIENT_LOOPBACK] sse_reconnect_and_pull_ms={:.3}",
        reconnect_elapsed.as_secs_f64() * 1000.0
    );

    // Testa tambem `on_resume()` explicitamente: grava `page:88` no banco do servidor e chama `on_resume()`
    {
        let srv_conn = Connection::open(srv_data.join("bookferry.db")).unwrap();
        srv_conn
            .execute(
                "UPDATE positions
                 SET locator = 'page:88', percent = 88.0,
                     hlc_canonical = '001799999999500:000000:dev_external',
                     hlc_wall_ms = 1799999999500
                 WHERE book_id = ?1",
                [book_id.as_str()],
            )
            .unwrap();
    }

    let pulled_on_resume = client_b.on_resume().await.unwrap();
    assert_eq!(pulled_on_resume.len(), 1);
    assert_eq!(pulled_on_resume[0].locator, "page:88");
    assert_eq!(
        client_b
            .get_position(&book_id)
            .await
            .unwrap()
            .unwrap()
            .locator,
        "page:88"
    );

    client_a.shutdown().await.unwrap();
    client_b.shutdown().await.unwrap();
    srv2.shutdown().await.unwrap();
}

// ============================================================================
// 9. HLC Monotonico: persiste no SQLite apos reabrir o Client, continua
//    monotonico quando o relogio volta no tempo, e ajusta em 422 hlc_too_far_in_future
// ============================================================================

#[tokio::test]
async fn hlc_monotonic_persistence_across_reopen_clock_regression_and_422_future_adjustment() {
    let tmp = TempDir::new().unwrap();
    let srv_data = tmp.path().join("srv_data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    let srv_clock = Arc::new(ManualClock::new(1_700_000_000_000));
    let mut srv_cfg = test_server_config(&srv_data, &vault_dir);
    srv_cfg.clock = srv_clock.clone();
    let srv = start_server(srv_cfg).await.unwrap();
    let qr = srv.pairing_payload();

    let cli_dir = tmp.path().join("cli_hlc");
    let cli_clock = Arc::new(ManualClock::new(1_700_000_050_000));

    // 1. Primeira abertura em t = 1_700_000_050_000
    let mut cfg1 = test_client_config(&cli_dir, "Phone HLC");
    cfg1.clock = cli_clock.clone();
    let client1 = Client::open(cfg1).await.unwrap();
    client1.pair(&qr, "Phone HLC").await.unwrap();

    let lib = client1.library().await.unwrap();
    let book_id = lib.books[0].book_id.clone();

    let p1 = client1
        .set_position(&book_id, "page:1", None, 1.0)
        .await
        .unwrap();
    let p2 = client1
        .set_position(&book_id, "page:2", None, 2.0)
        .await
        .unwrap();
    assert!(p2.hlc > p1.hlc);
    client1.shutdown().await.unwrap();

    // 2. Relogio do celular VOLTA NO TEMPO para 1_600_000_000_000 e o Client e reaberto no mesmo `data_dir`
    cli_clock.set_ms(1_600_000_000_000);
    let mut cfg2 = test_client_config(&cli_dir, "Phone HLC");
    cfg2.clock = cli_clock.clone();
    let client2 = Client::open(cfg2).await.unwrap();

    let p3 = client2
        .set_position(&book_id, "page:3", None, 3.0)
        .await
        .unwrap();
    assert!(
        p3.hlc > p2.hlc,
        "HLC deve continuar estritamente monotonico apos reabrir mesmo com relogio voltando no tempo: p3={} vs p2={}",
        p3.hlc,
        p2.hlc
    );
    assert_eq!(p3.hlc_wall_ms, p2.hlc_wall_ms);
    assert!(p3.hlc_logical > p2.hlc_logical);
    client2.flush_outbox().await.unwrap();

    // 3. Relogio do celular salta absurdamente para o futuro (+10 dias > 24h max_drift),
    //    causando `422 hlc_too_far_in_future` no servidor -> o cliente ajusta o HLC e aplica!
    cli_clock.set_ms(1_700_000_000_000 + DEFAULT_MAX_FUTURE_DRIFT_MS * 10);
    let p_fut = client2
        .set_position(&book_id, "page:99", Some("Relogio adiantado".into()), 99.0)
        .await
        .unwrap();
    assert!(p_fut.hlc_wall_ms > 1_700_000_000_000 + DEFAULT_MAX_FUTURE_DRIFT_MS);

    let _ = client2.flush_outbox().await.unwrap();
    let drained = wait_until(
        Duration::from_secs(3),
        Duration::from_millis(15),
        || async { client2.outbox_len().unwrap_or(1) == 0 },
    )
    .await;
    assert!(
        drained,
        "outbox com HLC no futuro deve ser ajustada e drenada"
    );

    let pos_adjusted = client2.get_position(&book_id).await.unwrap().unwrap();
    assert_eq!(pos_adjusted.locator, "page:99");
    assert!(
        pos_adjusted.hlc_wall_ms <= 1_700_000_000_000 + DEFAULT_MAX_FUTURE_DRIFT_MS,
        "HLC ajustado ({}) deve respeitar a janela do servidor",
        pos_adjusted.hlc
    );

    client2.shutdown().await.unwrap();
    srv.shutdown().await.unwrap();
}

// ============================================================================
// 10. Revogacao de Token (`401 Unauthorized`): emite `ClientEvent::Unpaired`
//     e encerra o laco de sincronizacao
// ============================================================================

#[tokio::test]
async fn token_revocation_emits_unpaired_and_stops_sync_loops() {
    let tmp = TempDir::new().unwrap();
    let srv_data = tmp.path().join("srv_data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    let srv = start_server(test_server_config(&srv_data, &vault_dir))
        .await
        .unwrap();

    let client_a = Client::open(test_client_config(&tmp.path().join("cli_a"), "Admin-PC"))
        .await
        .unwrap();
    let client_b = Client::open(test_client_config(
        &tmp.path().join("cli_b"),
        "Revoked-Phone",
    ))
    .await
    .unwrap();

    let mut rx_b = client_b.subscribe();

    let qr1 = srv.pairing_payload();
    client_a.pair(&qr1, "Admin-PC").await.unwrap();

    let qr2 = srv.rotate_pairing_secret().unwrap();
    let info_b = client_b.pair(&qr2, "Revoked-Phone").await.unwrap();

    // Aguarda B conectar no SSE
    let b_connected = wait_for_event(&mut rx_b, Duration::from_secs(3), |ev| {
        matches!(ev, ClientEvent::Connected { .. })
    })
    .await
    .is_some();
    assert!(b_connected);

    let lib = client_b.library().await.unwrap();
    let book_id = lib.books[0].book_id.clone();

    // Revoga o dispositivo B no banco do servidor
    {
        let srv_conn = Connection::open(srv.db_path()).unwrap();
        srv_conn
            .execute(
                "UPDATE devices SET revoked_at_ms = 1700000000000 WHERE id = ?1",
                [info_b.device_id.as_str()],
            )
            .unwrap();
    }

    // Quando B escreve uma posicao (entrando na outbox) ou faz pull(), o laco recebe 401,
    // emite `ClientEvent::Unpaired`, apaga `token.secret` e para!
    client_b
        .set_position(&book_id, "page:11", None, 11.0)
        .await
        .unwrap();

    let unpaired_detected = wait_until(
        Duration::from_secs(3),
        Duration::from_millis(15),
        || async { !client_b.is_paired().unwrap_or(true) },
    )
    .await;
    assert!(
        unpaired_detected,
        "laco de sincronizacao deve detectar 401 e desparear automaticamente"
    );
    assert!(!client_b.token_path().exists());

    let mut saw_unpaired = false;
    while let Ok(ev) = rx_b.try_recv() {
        if matches!(ev, ClientEvent::Unpaired { .. }) {
            saw_unpaired = true;
        }
    }
    assert!(
        saw_unpaired,
        "401 Unauthorized deve emitir ClientEvent::Unpaired"
    );

    client_a.shutdown().await.unwrap();
    client_b.shutdown().await.unwrap();
    srv.shutdown().await.unwrap();
}

// ============================================================================
// 11. Seguranca: Token e segredo de pareamento NUNCA aparecem em logs (`tracing`)
//     nem nas implementacoes de `fmt::Debug`
// ============================================================================

#[derive(Clone, Default)]
struct BufferWriter {
    buf: Arc<Mutex<Vec<u8>>>,
}

impl Write for BufferWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut guard) = self.buf.lock() {
            guard.extend_from_slice(data);
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn security_redaction_in_debug_and_tracing_logs() {
    let log_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let buf_clone = log_buf.clone();

    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || BufferWriter {
            buf: buf_clone.clone(),
        })
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let tmp = TempDir::new().unwrap();
    let srv_data = tmp.path().join("srv_data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    let srv = start_server(test_server_config(&srv_data, &vault_dir))
        .await
        .unwrap();
    let qr = srv.pairing_payload();
    let raw_secret = qr.secret.clone();
    assert!(raw_secret.len() > 16);

    let cli_dir = tmp.path().join("cli_sec");
    let cfg = test_client_config(&cli_dir, "Audit-Device");
    let client = Client::open(cfg.clone()).await.unwrap();

    let _info = client.pair(&qr, "Audit-Device").await.unwrap();
    let raw_token = std::fs::read_to_string(client.token_path())
        .unwrap()
        .trim()
        .to_string();
    assert!(raw_token.starts_with("bf_tok_"));

    let lib = client.library().await.unwrap();
    let book_id = lib.books[0].book_id.clone();
    client
        .set_position(&book_id, "page:15", Some("Teste de log".into()), 15.0)
        .await
        .unwrap();
    client.flush_outbox().await.unwrap();
    client.pull().await.unwrap();

    // Verifica que Debug de todas as estruturas publicas/internas redige segredos
    let debug_combined = format!("{qr:?} | {cfg:?} | {client:?}");
    assert!(
        !debug_combined.contains(&raw_secret),
        "Debug jamais deve conter o segredo de pareamento"
    );
    assert!(
        !debug_combined.contains(&raw_token),
        "Debug jamais deve conter o token Bearer"
    );

    client.unpair().await.unwrap();
    client.shutdown().await.unwrap();
    srv.shutdown().await.unwrap();

    // Verifica a saida capturada do tracing em nivel TRACE
    let captured_logs = String::from_utf8_lossy(&log_buf.lock().unwrap()).into_owned();
    assert!(
        !captured_logs.contains(&raw_secret),
        "Logs tracing jamais devem conter o segredo de pareamento"
    );
    assert!(
        !captured_logs.contains(&raw_token),
        "Logs tracing jamais devem conter o token Bearer"
    );

    // E confirma que `decide_resume` tambem esta acessivel na raiz do crate
    assert_eq!(decide_resume(None, None, 1000), ResumeDecision::Nothing);
}

// ============================================================================
// 12. Deteccao de Conexao SSE Meio-Aberta (`sse_read_timeout`) e Reconexao
// ============================================================================

#[tokio::test]
async fn sse_half_open_read_timeout_detects_stalled_server_and_reconnects() {
    let tmp = TempDir::new().unwrap();
    let srv_data = tmp.path().join("srv_data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);

    // Servidor configurado com keepalive muito longo (60 s), enquanto o cliente
    // exige batimento dentro de 120 ms (`sse_read_timeout = 120 ms`).
    let mut srv_cfg = test_server_config(&srv_data, &vault_dir);
    srv_cfg.sse_keepalive_interval = Duration::from_secs(60);
    let srv = start_server(srv_cfg).await.unwrap();
    let qr = srv.pairing_payload();

    let mut cli_cfg = test_client_config(&tmp.path().join("cli_half_open"), "Android-Doze");
    cli_cfg.sse_read_timeout = Duration::from_millis(120);
    cli_cfg.sse_backoff_initial = Duration::from_millis(40);

    let client = Client::open(cli_cfg).await.unwrap();
    let mut rx = client.subscribe();
    client.pair(&qr, "Android-Doze").await.unwrap();

    // 1. Conecta inicialmente
    let connected_1 = wait_for_event(&mut rx, Duration::from_secs(2), |ev| {
        matches!(ev, ClientEvent::Connected { .. })
    })
    .await
    .is_some();
    assert!(connected_1);

    // 2. Detecta conexao meio-aberta apos ~120 ms sem keepalive
    let half_open_disc = wait_for_event(&mut rx, Duration::from_secs(2), |ev| {
        matches!(
            ev,
            ClientEvent::Disconnected { ref reason } if reason == "read_timeout_half_open"
        )
    })
    .await
    .is_some();
    assert!(
        half_open_disc,
        "deve detectar conexao meio-aberta quando o timeout de leitura SSE expira"
    );

    // 3. Reconecta automaticamente apos o backoff curto
    let reconnected = wait_for_event(&mut rx, Duration::from_secs(2), |ev| {
        matches!(ev, ClientEvent::Connected { .. })
    })
    .await
    .is_some();
    assert!(
        reconnected,
        "deve reconectar automaticamente apos desconexao por read_timeout_half_open"
    );

    client.shutdown().await.unwrap();
    srv.shutdown().await.unwrap();
}

// ============================================================================
// Metadados por livro (nome, capa, "Continuar lendo") entre dois aparelhos
// ============================================================================

#[tokio::test]
async fn book_meta_syncs_between_devices_and_cover_is_cached_on_disk() {
    let tmp = TempDir::new().unwrap();
    let srv_data = tmp.path().join("srv_data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);
    let srv = start_server(test_server_config(&srv_data, &vault_dir))
        .await
        .unwrap();

    let pc_dir = tmp.path().join("pc");
    let phone_dir = tmp.path().join("phone");
    let pc = Client::open(test_client_config(&pc_dir, "PC"))
        .await
        .unwrap();
    let phone = Client::open(test_client_config(&phone_dir, "Celular"))
        .await
        .unwrap();
    pc.pair(&srv.pairing_payload(), "PC").await.unwrap();
    phone
        .pair(&srv.rotate_pairing_secret().unwrap(), "Celular")
        .await
        .unwrap();
    let mut rx_phone = phone.subscribe();
    assert!(
        wait_for_event(&mut rx_phone, Duration::from_secs(3), |ev| matches!(
            ev,
            ClientEvent::Connected { .. }
        ))
        .await
        .is_some()
    );

    let book_id = pc.library().await.unwrap().books[0].book_id.clone();
    phone.library().await.unwrap();

    // O PC gera a capa embutida; o celular recebe o evento e baixa o arquivo.
    let png = {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        v.extend_from_slice(b"imagem-fake-da-capa");
        v
    };
    assert_eq!(pc.books_without_cover().await.unwrap().len(), 3);
    let (applied, meta) = pc
        .set_cover(&book_id, CoverSource::Embedded, png.clone())
        .await
        .unwrap();
    assert!(applied && meta.cover_version > 0);
    assert_eq!(pc.books_without_cover().await.unwrap().len(), 2);

    let ev = wait_for_event(
        &mut rx_phone,
        Duration::from_secs(3),
        |ev| matches!(ev, ClientEvent::BookMetaChanged { book_id: b, .. } if b == &book_id),
    )
    .await
    .expect("celular recebe book_meta por SSE");
    let ClientEvent::BookMetaChanged { meta: seen, .. } = ev else {
        unreachable!()
    };
    assert_eq!(seen.cover_version, meta.cover_version);

    let path = phone
        .cover_path(&book_id)
        .await
        .unwrap()
        .expect("capa baixada");
    assert_eq!(std::fs::read(&path).unwrap(), png);
    assert!(path.starts_with(phone_dir.join("covers")));
    // Segunda chamada serve do disco (funciona ate com o servidor fora do ar).
    srv.shutdown().await.unwrap();
    assert_eq!(phone.cover_path(&book_id).await.unwrap().unwrap(), path);
    assert!(phone.set_display_title(&book_id, Some("x")).await.is_err());

    pc.shutdown().await.unwrap();
    phone.shutdown().await.unwrap();
}

#[tokio::test]
async fn book_meta_title_hide_user_cover_precedence_and_old_versions_cleanup() {
    let tmp = TempDir::new().unwrap();
    let srv_data = tmp.path().join("srv_data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);
    let srv = start_server(test_server_config(&srv_data, &vault_dir))
        .await
        .unwrap();
    let dir = tmp.path().join("pc");
    let pc = Client::open(test_client_config(&dir, "PC")).await.unwrap();
    pc.pair(&srv.pairing_payload(), "PC").await.unwrap();
    let book_id = pc.library().await.unwrap().books[0].book_id.clone();

    let meta = pc
        .set_display_title(&book_id, Some("  Meu   livro "))
        .await
        .unwrap();
    assert_eq!(meta.display_title.as_deref(), Some("Meu livro"));
    let lib = pc.library().await.unwrap();
    let b = lib.books.iter().find(|b| b.book_id == book_id).unwrap();
    assert_eq!(b.display_title.as_deref(), Some("Meu livro"));
    assert_ne!(b.title, "Meu livro", "o titulo original continua intacto");
    assert!(pc
        .set_display_title(&book_id, Some(&"x".repeat(201)))
        .await
        .is_err());
    let cleared = pc.set_display_title(&book_id, Some("   ")).await.unwrap();
    assert_eq!(cleared.display_title, None);

    let hid = pc.set_continue_hidden(&book_id, true).await.unwrap();
    assert!(hid.hidden_continue_at_ms.is_some());
    let lib = pc.library().await.unwrap();
    assert!(lib
        .books
        .iter()
        .find(|b| b.book_id == book_id)
        .unwrap()
        .hidden_continue_at_ms
        .is_some());
    assert_eq!(
        pc.set_continue_hidden(&book_id, false)
            .await
            .unwrap()
            .hidden_continue_at_ms,
        None
    );

    // Capa do usuario vence a embutida; capas invalidas sao recusadas.
    let jpg = |tag: &[u8]| {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0];
        v.extend_from_slice(tag);
        v
    };
    let (_, m1) = pc
        .set_cover(&book_id, CoverSource::Custom, jpg(b"um"))
        .await
        .unwrap();
    let p1 = pc.cover_path(&book_id).await.unwrap().unwrap();
    let (applied, m_ign) = pc
        .set_cover(&book_id, CoverSource::Embedded, jpg(b"auto"))
        .await
        .unwrap();
    assert!(!applied);
    assert_eq!(m_ign.cover_version, m1.cover_version);
    assert!(pc
        .set_cover(&book_id, CoverSource::Custom, b"<svg/>".to_vec())
        .await
        .is_err());
    let (_, m2) = pc
        .set_cover(&book_id, CoverSource::Web, jpg(b"dois"))
        .await
        .unwrap();
    assert!(m2.cover_version > m1.cover_version);
    let p2 = pc.cover_path(&book_id).await.unwrap().unwrap();
    assert_ne!(p1, p2);
    assert!(!p1.exists(), "versao antiga da capa e apagada do disco");
    assert_eq!(std::fs::read(&p2).unwrap(), jpg(b"dois"));

    // Restaurar original: some a capa e o arquivo local deixa de ser servido.
    let gone = pc.remove_cover(&book_id).await.unwrap();
    assert_eq!(gone.cover_version, 0);
    assert!(pc.cover_path(&book_id).await.unwrap().is_none());
    // Entrada invalida nao vira caminho.
    assert!(pc.cover_path("../../etc/passwd").await.is_err());

    pc.shutdown().await.unwrap();
    srv.shutdown().await.unwrap();
}

// ============================================================================
// Varios aparelhos num unico servidor (e um aparelho so fala com um servidor)
// ============================================================================

fn active_device_names(server_data_dir: &Path) -> Vec<String> {
    let conn = Connection::open_with_flags(
        server_data_dir.join("bookferry.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut stmt = conn
        .prepare("SELECT name FROM devices WHERE revoked_at_ms IS NULL ORDER BY name")
        .unwrap();
    let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
    rows.map(|r| r.unwrap()).collect()
}

#[tokio::test]
async fn one_server_serves_one_pc_and_two_phones_sharing_library_and_position() {
    let tmp = TempDir::new().unwrap();
    let srv_data = tmp.path().join("srv_data");
    let vault_dir = tmp.path().join("vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    populate_sample_vault(&vault_dir);
    let srv = start_server(test_server_config(&srv_data, &vault_dir))
        .await
        .unwrap();

    // Um PC "so cliente" e dois celulares, cada um com seu proprio banco local.
    let pc = Client::open(test_client_config(&tmp.path().join("pc"), "PC da sala"))
        .await
        .unwrap();
    let phone_1 = Client::open(test_client_config(&tmp.path().join("p1"), "Celular 1"))
        .await
        .unwrap();
    let phone_2 = Client::open(test_client_config(&tmp.path().join("p2"), "Celular 2"))
        .await
        .unwrap();
    let mut rx_pc = pc.subscribe();
    let mut rx_p2 = phone_2.subscribe();

    // Cada aparelho usa o QR (segredo de uso unico) gerado para ele.
    pc.pair(&srv.pairing_payload(), "PC da sala").await.unwrap();
    phone_1
        .pair(&srv.rotate_pairing_secret().unwrap(), "Celular 1")
        .await
        .unwrap();
    phone_2
        .pair(&srv.rotate_pairing_secret().unwrap(), "Celular 2")
        .await
        .unwrap();

    assert_eq!(
        active_device_names(&srv_data),
        vec!["Celular 1", "Celular 2", "PC da sala"]
    );

    // Mesma biblioteca para todos.
    let ids = |snap: bookferry_client::LibrarySnapshot| {
        let mut v: Vec<String> = snap.books.iter().map(|b| b.book_id.clone()).collect();
        v.sort();
        v
    };
    let lib_pc = ids(pc.library().await.unwrap());
    assert_eq!(lib_pc.len(), 3);
    assert_eq!(lib_pc, ids(phone_1.library().await.unwrap()));
    assert_eq!(lib_pc, ids(phone_2.library().await.unwrap()));

    for rx in [&mut rx_pc, &mut rx_p2] {
        let ok = wait_for_event(rx, Duration::from_secs(3), |ev| {
            matches!(ev, ClientEvent::Connected { .. })
        })
        .await
        .is_some();
        assert!(ok, "todo aparelho deve conectar ao SSE");
        while rx.try_recv().is_ok() {}
    }

    // O celular 1 le ate a pagina 70: PC e celular 2 recebem na hora, pela mesma posicao.
    phone_1
        .set_position(&lib_pc[0], "page:70", None, 70.0)
        .await
        .unwrap();
    for (rx, who) in [(&mut rx_pc, "PC"), (&mut rx_p2, "celular 2")] {
        let got = wait_for_event(rx, Duration::from_secs(3), |ev| {
            matches!(
                ev,
                ClientEvent::PositionChanged { position, origin: PositionOrigin::RemoteSse, .. }
                    if position.locator == "page:70"
            )
        })
        .await
        .is_some();
        assert!(got, "{who} deveria receber a posicao do celular 1");
    }
    assert_eq!(
        pc.get_position(&lib_pc[0]).await.unwrap().unwrap().locator,
        "page:70"
    );

    // A posicao e uma por livro para o servidor inteiro (nao ha usuarios separados).
    for c in [&pc, &phone_1, &phone_2] {
        c.shutdown().await.unwrap();
    }
    srv.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_device_talks_to_one_server_at_a_time() {
    let tmp = TempDir::new().unwrap();
    let vault_a = tmp.path().join("vault_a");
    let vault_b = tmp.path().join("vault_b");
    std::fs::create_dir_all(&vault_a).unwrap();
    std::fs::create_dir_all(&vault_b).unwrap();
    populate_sample_vault(&vault_a); // 3 livros
    std::fs::write(
        vault_b.join("Unico.epub"),
        make_minimal_zip("OEBPS/c.xhtml", &[b'U'; 2048]),
    )
    .unwrap(); // 1 livro
    let data_a = tmp.path().join("srv_a");
    let data_b = tmp.path().join("srv_b");
    let srv_a = start_server(test_server_config(&data_a, &vault_a))
        .await
        .unwrap();
    let srv_b = start_server(test_server_config(&data_b, &vault_b))
        .await
        .unwrap();

    let phone = Client::open(test_client_config(&tmp.path().join("phone"), "Celular"))
        .await
        .unwrap();
    phone
        .pair(&srv_a.pairing_payload(), "Celular")
        .await
        .unwrap();
    assert_eq!(phone.library().await.unwrap().books.len(), 3);

    // Parear com outro servidor SUBSTITUI o primeiro (ha um unico pareamento por aparelho).
    phone
        .pair(&srv_b.pairing_payload(), "Celular")
        .await
        .unwrap();
    assert_eq!(phone.library().await.unwrap().books.len(), 1);

    // O servidor A nao e avisado: o aparelho continua na lista dele, ate alguem revogar la.
    assert_eq!(active_device_names(&data_a), vec!["Celular"]);

    phone.shutdown().await.unwrap();
    srv_a.shutdown().await.unwrap();
    srv_b.shutdown().await.unwrap();
}

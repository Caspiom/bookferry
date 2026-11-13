use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use bookferry_server::{
    build_pinned_client_config, client_tls_request, discover_mdns_once,
    download_cbz_with_interruption_and_range_resume, probe_plain_http_rejected,
    probe_tls_pinning_case, read_proc_mem, sha256_hex, unix_ms_now, MdnsDiscoveryAttempt,
    PairingQrPayload, PositionPayload, ProcMemStatus, RangeTransferReport, StoredPosition,
    TlsProbeResult,
};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, Error as TlsError, SignatureScheme,
    StreamOwned,
};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::FileExt;
use std::os::unix::io::FromRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

static REGISTERED_FILES: OnceLock<Mutex<HashMap<String, Arc<File>>>> = OnceLock::new();
static SERVER_PORT: OnceLock<u16> = OnceLock::new();
static ANDROID_SSE_STATE: OnceLock<Arc<AndroidSseTracker>> = OnceLock::new();

pub struct AndroidSseTracker {
    pub running: AtomicBool,
    pub connected: AtomicBool,
    pub connection_epoch: AtomicU64,
    pub logs: Mutex<Vec<serde_json::Value>>,
    pub received_events: Mutex<Vec<serde_json::Value>>,
    pub saved_token: Mutex<Option<String>>,
    pub saved_qr: Mutex<Option<PairingQrPayload>>,
}

fn sse_tracker() -> &'static Arc<AndroidSseTracker> {
    ANDROID_SSE_STATE.get_or_init(|| {
        Arc::new(AndroidSseTracker {
            running: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            connection_epoch: AtomicU64::new(0),
            logs: Mutex::new(Vec::new()),
            received_events: Mutex::new(Vec::new()),
            saved_token: Mutex::new(None),
            saved_qr: Mutex::new(None),
        })
    })
}

fn files_map() -> &'static Mutex<HashMap<String, Arc<File>>> {
    REGISTERED_FILES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ia = a.chars().peekable();
    let mut ib = b.chars().peekable();

    loop {
        match (ia.peek(), ib.peek()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(&ca), Some(&cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let mut sa = String::new();
                    let mut sb = String::new();
                    while let Some(&c) = ia.peek() {
                        if c.is_ascii_digit() {
                            sa.push(c);
                            ia.next();
                        } else {
                            break;
                        }
                    }
                    while let Some(&c) = ib.peek() {
                        if c.is_ascii_digit() {
                            sb.push(c);
                            ib.next();
                        } else {
                            break;
                        }
                    }
                    let sa_trim = sa.trim_start_matches('0');
                    let sb_trim = sb.trim_start_matches('0');
                    match sa_trim.len().cmp(&sb_trim.len()) {
                        Ordering::Equal => match sa_trim.cmp(sb_trim) {
                            Ordering::Equal => match sa.len().cmp(&sb.len()) {
                                Ordering::Equal => continue,
                                other => return other,
                            },
                            other => return other,
                        },
                        other => return other,
                    }
                } else {
                    let la = ca.to_ascii_lowercase();
                    let lb = cb.to_ascii_lowercase();
                    if la != lb {
                        return la.cmp(&lb);
                    }
                    ia.next();
                    ib.next();
                }
            }
        }
    }
}

fn is_valid_comic_page(entry_name: &str) -> bool {
    if entry_name.ends_with('/') {
        return false;
    }
    if entry_name.starts_with("__MACOSX/") || entry_name.contains("/__MACOSX/") {
        return false;
    }
    let base = entry_name.rsplit('/').next().unwrap_or(entry_name);
    if base.starts_with('.') {
        return false;
    }
    let lower = base.to_ascii_lowercase();
    lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".png")
        || lower.ends_with(".webp")
        || lower.ends_with(".gif")
}

#[derive(Serialize)]
pub struct SafCbzResult {
    pub file_name: String,
    pub file_size_bytes: u64,
    pub total_zip_entries: usize,
    pub ignored_macosx_entries: usize,
    pub valid_page_count: usize,
    pub first_5_pages: Vec<String>,
    pub last_5_pages: Vec<String>,
    pub target_page_index: usize,
    pub target_page_name: String,
    pub target_page_bytes: usize,
    pub zip_open_ms: f64,
    pub list_natural_sort_ms: f64,
    pub read_single_page_ms: f64,
    pub base64_encode_ms: f64,
    pub total_rust_ms: f64,
    pub copied_to_cache: bool,
    pub mem_before: ProcMemStatus,
    pub mem_after: ProcMemStatus,
    pub data_url: String,
}

#[tauri::command]
fn ping(name: String) -> String {
    format!(
        "pong, {} (rust {} / {})",
        name,
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    )
}

#[tauri::command]
fn saf_inspect_and_read_cbz(
    fd: i32,
    name: String,
    page_index: Option<usize>,
    target_name: Option<String>,
) -> Result<SafCbzResult, String> {
    let mem_before = read_proc_mem();
    let t_total = Instant::now();

    let file = unsafe { File::from_raw_fd(fd) };
    let file_size_bytes = file.metadata().map(|m| m.len()).unwrap_or(0);

    let t_zip = Instant::now();
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("ZipArchive::new erro: {e}"))?;
    let zip_open_ms = t_zip.elapsed().as_secs_f64() * 1000.0;

    let total_zip_entries = archive.len();
    let mut pages: Vec<String> = Vec::with_capacity(total_zip_entries);
    let mut ignored_macosx_entries = 0usize;

    for raw_name in archive.file_names() {
        if is_valid_comic_page(raw_name) {
            pages.push(raw_name.to_string());
        } else {
            ignored_macosx_entries += 1;
        }
    }

    pages.sort_by(|a, b| natural_cmp(a, b));
    let list_natural_sort_ms = t_total.elapsed().as_secs_f64() * 1000.0;

    if pages.is_empty() {
        return Err("Nenhuma pagina valida encontrada no CBZ".to_string());
    }

    let chosen_idx = if let Some(ref req_name) = target_name {
        pages
            .iter()
            .position(|p| p == req_name || p.ends_with(req_name))
            .unwrap_or(0)
    } else {
        page_index.unwrap_or(0).min(pages.len() - 1)
    };
    let chosen_name = pages[chosen_idx].clone();

    let t_read = Instant::now();
    let mut buf = Vec::new();
    {
        let mut entry = archive
            .by_name(&chosen_name)
            .map_err(|e| format!("by_name({chosen_name}) erro: {e}"))?;
        buf.reserve_exact(entry.size() as usize);
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("read_to_end erro: {e}"))?;
    }
    let read_single_page_ms = t_read.elapsed().as_secs_f64() * 1000.0;

    let t_b64 = Instant::now();
    let b64 = BASE64.encode(&buf);
    let data_url = format!("data:image/jpeg;base64,{b64}");
    let base64_encode_ms = t_b64.elapsed().as_secs_f64() * 1000.0;

    let total_rust_ms = t_total.elapsed().as_secs_f64() * 1000.0;

    let inner_file = archive.into_inner();
    files_map()
        .lock()
        .unwrap()
        .insert(name.clone(), Arc::new(inner_file));

    let mem_after = read_proc_mem();

    let first_5_pages = pages.iter().take(5).cloned().collect();
    let last_5_pages = pages
        .iter()
        .rev()
        .take(5)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    Ok(SafCbzResult {
        file_name: name,
        file_size_bytes,
        total_zip_entries,
        ignored_macosx_entries,
        valid_page_count: pages.len(),
        first_5_pages,
        last_5_pages,
        target_page_index: chosen_idx,
        target_page_name: chosen_name,
        target_page_bytes: buf.len(),
        zip_open_ms,
        list_natural_sort_ms,
        read_single_page_ms,
        base64_encode_ms,
        total_rust_ms,
        copied_to_cache: false,
        mem_before,
        mem_after,
        data_url,
    })
}

#[derive(Serialize)]
pub struct RegisteredFileInfo {
    pub name: String,
    pub size: u64,
    pub http_url: String,
}

#[tauri::command]
fn register_saf_file(fd: i32, name: String) -> Result<RegisteredFileInfo, String> {
    let file = unsafe { File::from_raw_fd(fd) };
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    files_map()
        .lock()
        .unwrap()
        .insert(name.clone(), Arc::new(file));
    let port = *SERVER_PORT.get().unwrap_or(&0);
    Ok(RegisteredFileInfo {
        name: name.clone(),
        size,
        http_url: format!("http://127.0.0.1:{port}/file/{name}"),
    })
}

#[tauri::command]
fn try_open_direct_path(
    path: String,
    register_as: Option<String>,
) -> Result<RegisteredFileInfo, String> {
    let file = File::open(&path).map_err(|e| {
        format!(
            "File::open({path}) falhou: {e} (os error {:?})",
            e.raw_os_error()
        )
    })?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    let name =
        register_as.unwrap_or_else(|| path.rsplit('/').next().unwrap_or("file").to_string());
    files_map()
        .lock()
        .unwrap()
        .insert(name.clone(), Arc::new(file));
    let port = *SERVER_PORT.get().unwrap_or(&0);
    Ok(RegisteredFileInfo {
        name: name.clone(),
        size,
        http_url: format!("http://127.0.0.1:{port}/file/{name}"),
    })
}

#[derive(Serialize)]
pub struct EnvInfo {
    pub os: String,
    pub arch: String,
    pub server_port: u16,
    pub autorun: Option<String>,
    pub out_file: Option<String>,
}

#[tauri::command]
fn get_env_info() -> EnvInfo {
    EnvInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        server_port: *SERVER_PORT.get().unwrap_or(&0),
        autorun: std::env::var("BOOKFERRY_AUTORUN").ok(),
        out_file: std::env::var("BOOKFERRY_OUT").ok(),
    }
}

#[tauri::command]
fn log_result(tag: String, payload: String) {
    println!("[BOOKFERRY_RESULT][{}] {}", tag, payload);
    if let Ok(out_path) = std::env::var("BOOKFERRY_OUT") {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(out_path)
        {
            let _ = writeln!(f, "{}", payload);
        }
    }
    if tag == "AUTORUN_DONE" && std::env::var("BOOKFERRY_AUTORUN").is_ok() {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(300));
            std::process::exit(0);
        });
    }
}

// ============================================================================
// FASE 3: Comandos Tauri de Rede Local (mDNS, TLS Pinning, Pareamento, Sync HLC+SSE, Range)
// ============================================================================

/// Simula a câmera lendo o QR impresso pelo servidor no PC (busca `/qr-info` na LAN apenas para obter
/// o segredo e o hash SHA-256 gerados em runtime pelo servidor atual).
#[derive(Debug)]
struct BootstrapQrFetcherVerifier;
impl ServerCertVerifier for BootstrapQrFetcherVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[tauri::command]
fn lan_read_simulated_qr(host: String, port: u16) -> Result<PairingQrPayload, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cfg = Arc::new(
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(BootstrapQrFetcherVerifier))
            .with_no_client_auth(),
    );
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|e| format!("Addr err: {e}"))?;
    let mut tcp =
        TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|e| e.to_string())?;
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(5)));
    let sn = ServerName::try_from("bookferry.local".to_string()).unwrap();
    let mut conn = ClientConnection::new(cfg, sn).map_err(|e| e.to_string())?;
    while conn.is_handshaking() {
        conn.complete_io(&mut tcp).map_err(|e| e.to_string())?;
    }
    let mut stream = StreamOwned::new(conn, tcp);
    let req = format!("GET /qr-info HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).map_err(|e| e.to_string())?;
    let mut content_len = 0usize;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_len = v.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body_bytes = vec![0u8; content_len];
    reader.read_exact(&mut body_bytes).map_err(|e| e.to_string())?;
    let qr: PairingQrPayload =
        serde_json::from_slice(&body_bytes).map_err(|e| format!("JSON /qr-info err: {e}"))?;
    *sse_tracker().saved_qr.lock().unwrap() = Some(qr.clone());
    Ok(qr)
}

#[tauri::command]
fn lan_discover_mdns(rep: usize, mode: String, timeout_ms: u64) -> MdnsDiscoveryAttempt {
    discover_mdns_once(rep, &mode, timeout_ms)
}

#[tauri::command]
fn lan_test_tls_pinning(
    host: String,
    port: u16,
    correct_sha256: String,
    repetitions: usize,
) -> serde_json::Value {
    let mut valid_runs: Vec<TlsProbeResult> = Vec::new();
    for i in 0..repetitions.max(1) {
        valid_runs.push(probe_tls_pinning_case(
            &format!("valid_hash_rep_{}", i + 1),
            &host,
            port,
            &correct_sha256,
        ));
    }

    let wrong_hash = "000000000000000000000000000000000000000000000000000000000000dead";
    let wrong_hash_probe = probe_tls_pinning_case("wrong_hash_rejected", &host, port, wrong_hash);
    let plain_http_probe = probe_plain_http_rejected(&host, port);

    serde_json::json!({
        "host": host,
        "port": port,
        "correct_sha256": correct_sha256,
        "case_a_correct_hash_runs": valid_runs,
        "case_b_wrong_hash_rejected": wrong_hash_probe,
        "case_c_plain_http_rejected": plain_http_probe,
    })
}

#[tauri::command]
fn lan_test_pairing(
    host: String,
    port: u16,
    cert_sha256: String,
    one_time_secret: String,
) -> Result<serde_json::Value, String> {
    // 1) Token inválido antes ou sem parear -> 401
    let (status_invalid_tok, _, body_invalid_tok, _, _, _) = client_tls_request(
        &host,
        port,
        &cert_sha256,
        "GET",
        "/position/little-nemo",
        Some("bf_tok_invalido_000000"),
        None,
    )?;

    // 2) Segredo de pareamento errado -> 401
    let wrong_pair_body =
        serde_json::to_vec(&serde_json::json!({"secret": "pair_errado_123", "device_name": "SM-S918B"})).unwrap();
    let (status_wrong_secret, _, body_wrong_secret, _, _, _) = client_tls_request(
        &host,
        port,
        &cert_sha256,
        "POST",
        "/pair",
        None,
        Some(&wrong_pair_body),
    )?;

    // 3) Segredo de uso único correto -> 200 OK + recebe token
    let ok_pair_body =
        serde_json::to_vec(&serde_json::json!({"secret": one_time_secret, "device_name": "SM-S918B"})).unwrap();
    let t_pair = Instant::now();
    let (status_pair_ok, _, body_pair_ok, tcp_ms, hs_ms, req_ms) = client_tls_request(
        &host,
        port,
        &cert_sha256,
        "POST",
        "/pair",
        None,
        Some(&ok_pair_body),
    )?;
    let pair_total_ms = t_pair.elapsed().as_secs_f64() * 1000.0;
    let pair_json: serde_json::Value =
        serde_json::from_slice(&body_pair_ok).unwrap_or(serde_json::json!({}));
    let token = pair_json
        .get("token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if !token.is_empty() {
        *sse_tracker().saved_token.lock().unwrap() = Some(token.clone());
    }

    // 4) Reuso do mesmo segredo de uso único -> 403 Forbidden (recusado!)
    let (status_reused_secret, _, body_reused_secret, _, _, _) = client_tls_request(
        &host,
        port,
        &cert_sha256,
        "POST",
        "/pair",
        None,
        Some(&ok_pair_body),
    )?;

    // 5) Chamada autenticada com o token válido recebido -> 200 OK
    let (status_auth_ok, _, body_auth_ok, _, _, _) = client_tls_request(
        &host,
        port,
        &cert_sha256,
        "GET",
        "/status",
        Some(&token),
        None,
    )?;

    Ok(serde_json::json!({
        "step1_invalid_token": {
            "status": status_invalid_tok,
            "body": String::from_utf8_lossy(&body_invalid_tok),
        },
        "step2_wrong_secret": {
            "status": status_wrong_secret,
            "body": String::from_utf8_lossy(&body_wrong_secret),
        },
        "step3_valid_one_time_secret": {
            "status": status_pair_ok,
            "tcp_ms": tcp_ms,
            "tls_hs_ms": hs_ms,
            "req_ms": req_ms,
            "total_ms": pair_total_ms,
            "body": pair_json,
        },
        "step4_reused_secret_refused": {
            "status": status_reused_secret,
            "body": String::from_utf8_lossy(&body_reused_secret),
        },
        "step5_authenticated_status_check": {
            "status": status_auth_ok,
            "ok": status_auth_ok == 200,
            "body_preview": String::from_utf8_lossy(&body_auth_ok).chars().take(200).collect::<String>(),
        },
        "issued_token": token,
    }))
}

#[tauri::command]
fn lan_put_position(
    host: String,
    port: u16,
    cert_sha256: String,
    token: String,
    book_id: String,
    device: String,
    hlc: String,
    locator: String,
    percent: f64,
    page: Option<u32>,
) -> Result<serde_json::Value, String> {
    let payload = PositionPayload {
        device,
        hlc,
        locator,
        percent,
        page,
    };
    let body_bytes = serde_json::to_vec(&payload).map_err(|e| e.to_string())?;
    let (status, _, resp, tcp_ms, hs_ms, req_ms) = client_tls_request(
        &host,
        port,
        &cert_sha256,
        "PUT",
        &format!("/position/{book_id}"),
        Some(&token),
        Some(&body_bytes),
    )?;
    let parsed: serde_json::Value =
        serde_json::from_slice(&resp).unwrap_or_else(|_| serde_json::json!({"raw": String::from_utf8_lossy(&resp)}));
    Ok(serde_json::json!({
        "status": status,
        "tcp_ms": tcp_ms,
        "tls_hs_ms": hs_ms,
        "req_ms": req_ms,
        "response": parsed,
    }))
}

#[tauri::command]
fn lan_get_position(
    host: String,
    port: u16,
    cert_sha256: String,
    token: String,
    book_id: String,
) -> Result<serde_json::Value, String> {
    let t0 = Instant::now();
    let (status, _, resp, tcp_ms, hs_ms, req_ms) = client_tls_request(
        &host,
        port,
        &cert_sha256,
        "GET",
        &format!("/position/{book_id}"),
        Some(&token),
        None,
    )?;
    let total_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let parsed: serde_json::Value =
        serde_json::from_slice(&resp).unwrap_or_else(|_| serde_json::json!({"raw": String::from_utf8_lossy(&resp)}));
    Ok(serde_json::json!({
        "status": status,
        "tcp_ms": tcp_ms,
        "tls_hs_ms": hs_ms,
        "req_ms": req_ms,
        "total_ms": total_ms,
        "response": parsed,
    }))
}

#[tauri::command]
fn lan_start_sse_listener(
    window: tauri::WebviewWindow,
    host: String,
    port: u16,
    cert_sha256: String,
    token: String,
) -> Result<serde_json::Value, String> {
    let tracker = sse_tracker().clone();
    let epoch = tracker.connection_epoch.fetch_add(1, AtomicOrdering::SeqCst) + 1;
    tracker.running.store(true, AtomicOrdering::SeqCst);

    std::thread::spawn(move || {
        let mut attempt = 0u64;
        let mut last_disconnect_instant: Option<Instant> = None;

        while tracker.running.load(AtomicOrdering::SeqCst)
            && tracker.connection_epoch.load(AtomicOrdering::SeqCst) == epoch
        {
            attempt += 1;
            let t_conn_start = Instant::now();
            let addr: SocketAddr = match format!("{host}:{port}").parse() {
                Ok(a) => a,
                Err(_) => break,
            };

            let tcp_res = TcpStream::connect_timeout(&addr, Duration::from_secs(5));
            let mut tcp = match tcp_res {
                Ok(s) => s,
                Err(e) => {
                    let ts = unix_ms_now();
                    let log_item = serde_json::json!({
                        "ts_ms": ts,
                        "attempt": attempt,
                        "event": "sse_connect_failed",
                        "error": e.to_string(),
                    });
                    println!("[BOOKFERRY_RESULT][ANDROID_SSE_LOG] {log_item}");
                    tracker.logs.lock().unwrap().push(log_item);
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
            };

            let _ = tcp.set_nodelay(true);
            // Servidor manda `: hb <ts>` a cada 2s; timeout de leitura de 6s detecta queda/congelamento rapidamente
            let _ = tcp.set_read_timeout(Some(Duration::from_secs(6)));
            let _ = tcp.set_write_timeout(Some(Duration::from_secs(5)));

            let cfg = match build_pinned_client_config(&cert_sha256) {
                Ok(c) => c,
                Err(_) => break,
            };
            let sn = ServerName::try_from("bookferry.local".to_string()).unwrap();
            let mut conn = match ClientConnection::new(cfg, sn) {
                Ok(c) => c,
                Err(_) => break,
            };

            let mut hs_ok = true;
            while conn.is_handshaking() {
                if let Err(e) = conn.complete_io(&mut tcp) {
                    let ts = unix_ms_now();
                    let log_item = serde_json::json!({
                        "ts_ms": ts,
                        "attempt": attempt,
                        "event": "sse_tls_hs_failed",
                        "error": e.to_string(),
                    });
                    println!("[BOOKFERRY_RESULT][ANDROID_SSE_LOG] {log_item}");
                    tracker.logs.lock().unwrap().push(log_item);
                    hs_ok = false;
                    break;
                }
            }
            if !hs_ok {
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }

            let mut tls_stream = StreamOwned::new(conn, tcp);
            let req = format!(
                "GET /events HTTP/1.1\r\nHost: {host}:{port}\r\nAuthorization: Bearer {token}\r\nX-Device-Name: SM-S918B\r\nAccept: text/event-stream\r\nConnection: keep-alive\r\n\r\n"
            );
            if tls_stream.write_all(req.as_bytes()).is_err() || tls_stream.flush().is_err() {
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }

            let mut reader = BufReader::new(tls_stream);
            let mut status_line = String::new();
            if reader.read_line(&mut status_line).is_err() || !status_line.contains("200") {
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
            loop {
                let mut hdr = String::new();
                if reader.read_line(&mut hdr).is_err() || hdr.trim().is_empty() {
                    break;
                }
            }

            tracker.connected.store(true, AtomicOrdering::SeqCst);
            let connect_elapsed_ms = t_conn_start.elapsed().as_secs_f64() * 1000.0;
            let downtime_ms = last_disconnect_instant.map(|d| d.elapsed().as_secs_f64() * 1000.0);
            let ts_conn = unix_ms_now();
            let conn_log = serde_json::json!({
                "ts_ms": ts_conn,
                "attempt": attempt,
                "event": if attempt == 1 { "sse_connected_initial" } else { "sse_reconnected" },
                "connect_elapsed_ms": connect_elapsed_ms,
                "downtime_since_disconnect_ms": downtime_ms,
            });
            println!("[BOOKFERRY_RESULT][ANDROID_SSE_LOG] {conn_log}");
            tracker.logs.lock().unwrap().push(conn_log.clone());
            let _ = window.eval(&format!(
                "window.__onLanSseStatus && window.__onLanSseStatus({})",
                conn_log
            ));

            let mut current_event = String::from("message");
            loop {
                if !tracker.running.load(AtomicOrdering::SeqCst)
                    || tracker.connection_epoch.load(AtomicOrdering::SeqCst) != epoch
                {
                    break;
                }
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        let ts = unix_ms_now();
                        tracker.connected.store(false, AtomicOrdering::SeqCst);
                        last_disconnect_instant = Some(Instant::now());
                        let dlog = serde_json::json!({
                            "ts_ms": ts,
                            "attempt": attempt,
                            "event": "sse_eof_disconnected",
                        });
                        println!("[BOOKFERRY_RESULT][ANDROID_SSE_LOG] {dlog}");
                        tracker.logs.lock().unwrap().push(dlog);
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        if let Some(ev) = trimmed.strip_prefix("event:") {
                            current_event = ev.trim().to_string();
                        } else if let Some(data_str) = trimmed.strip_prefix("data:") {
                            let data_trimmed = data_str.trim();
                            let recv_ms = unix_ms_now();
                            if let Ok(pos) = serde_json::from_str::<StoredPosition>(data_trimmed) {
                                // Envia ACK imediato ao servidor PC para medir latência exata no relógio monotônico do PC!
                                let h_clone = host.clone();
                                let c_clone = cert_sha256.clone();
                                let t_clone = token.clone();
                                let pos_clone = pos.clone();
                                std::thread::spawn(move || {
                                    let ack_body = serde_json::to_vec(&serde_json::json!({
                                        "book_id": pos_clone.book_id,
                                        "page": pos_clone.page,
                                        "hlc": pos_clone.hlc_canonical,
                                        "device": "android-rust-sse",
                                        "server_received_unix_ms": pos_clone.server_received_unix_ms,
                                        "client_reported_render_ms": null,
                                    }))
                                    .unwrap_or_default();
                                    let _ = client_tls_request(
                                        &h_clone,
                                        port,
                                        &c_clone,
                                        "POST",
                                        "/position-ack",
                                        Some(&t_clone),
                                        Some(&ack_body),
                                    );
                                });

                                let ev_obj = serde_json::json!({
                                    "event": current_event,
                                    "android_rust_recv_unix_ms": recv_ms,
                                    "position": pos,
                                });
                                println!("[BOOKFERRY_RESULT][ANDROID_SSE_EVENT] {ev_obj}");
                                tracker.received_events.lock().unwrap().push(ev_obj.clone());
                                let _ = window.eval(&format!(
                                    "window.__onLanSseEvent && window.__onLanSseEvent({})",
                                    ev_obj
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        let ts = unix_ms_now();
                        tracker.connected.store(false, AtomicOrdering::SeqCst);
                        last_disconnect_instant = Some(Instant::now());
                        let dlog = serde_json::json!({
                            "ts_ms": ts,
                            "attempt": attempt,
                            "event": "sse_read_error_disconnected",
                            "error": e.to_string(),
                        });
                        println!("[BOOKFERRY_RESULT][ANDROID_SSE_LOG] {dlog}");
                        tracker.logs.lock().unwrap().push(dlog);
                        break;
                    }
                }
            }

            std::thread::sleep(Duration::from_millis(400));
        }
    });

    Ok(serde_json::json!({"ok": true, "epoch": epoch}))
}

#[tauri::command]
fn lan_ack_ui_rendered(
    host: String,
    port: u16,
    cert_sha256: String,
    token: String,
    book_id: String,
    page: Option<u32>,
    hlc: String,
    server_received_unix_ms: u64,
    ui_render_delta_ms: f64,
) -> Result<serde_json::Value, String> {
    let ack_body = serde_json::to_vec(&serde_json::json!({
        "book_id": book_id,
        "page": page,
        "hlc": hlc,
        "device": "android-webview-ui",
        "server_received_unix_ms": server_received_unix_ms,
        "client_reported_render_ms": ui_render_delta_ms,
    }))
    .unwrap_or_default();
    let (status, _, resp, _, _, _) = client_tls_request(
        &host,
        port,
        &cert_sha256,
        "POST",
        "/position-ack",
        Some(&token),
        Some(&ack_body),
    )?;
    let parsed: serde_json::Value =
        serde_json::from_slice(&resp).unwrap_or_else(|_| serde_json::json!({}));
    Ok(serde_json::json!({"status": status, "ack": parsed}))
}

#[tauri::command]
fn lan_get_sse_state() -> serde_json::Value {
    let tr = sse_tracker();
    let logs = tr.logs.lock().unwrap().clone();
    let events = tr.received_events.lock().unwrap().clone();
    let saved_token = tr.saved_token.lock().unwrap().clone();
    let saved_qr = tr.saved_qr.lock().unwrap().clone();
    serde_json::json!({
        "running": tr.running.load(AtomicOrdering::SeqCst),
        "connected": tr.connected.load(AtomicOrdering::SeqCst),
        "logs": logs,
        "received_events": events,
        "saved_token": saved_token,
        "saved_qr": saved_qr,
    })
}

#[tauri::command]
fn lan_download_cbz_range(
    host: String,
    port: u16,
    cert_sha256: String,
    token: String,
    file_name: String,
    fd: i32,
    dest_path: String,
    interrupt_after_bytes: u64,
) -> Result<RangeTransferReport, String> {
    let file = unsafe { File::from_raw_fd(fd) };
    download_cbz_with_interruption_and_range_resume(
        &host,
        port,
        &cert_sha256,
        &token,
        &file_name,
        file,
        &dest_path,
        interrupt_after_bytes,
    )
}

fn resolve_file(name: &str) -> Option<Arc<File>> {
    if let Some(f) = files_map().lock().unwrap().get(name).cloned() {
        return Some(f);
    }
    let candidates = [
        PathBuf::from(format!("../../fixtures/{name}")),
        PathBuf::from(format!("../fixtures/{name}")),
        PathBuf::from(format!("fixtures/{name}")),
        PathBuf::from(format!(
            "/home/gappy/Documents/projetos/bookshelf/spike/fixtures/{name}"
        )),
        PathBuf::from(format!("/sdcard/Download/bookferry-fixtures/{name}")),
    ];
    for p in candidates {
        if let Ok(f) = File::open(&p) {
            let arc = Arc::new(f);
            files_map()
                .lock()
                .unwrap()
                .insert(name.to_string(), arc.clone());
            return Some(arc);
        }
    }
    None
}

fn guess_mime(name: &str) -> &'static str {
    if name.ends_with(".pdf") {
        "application/pdf"
    } else if name.ends_with(".epub") {
        "application/epub+zip"
    } else if name.ends_with(".cbz") {
        "application/vnd.comicbook+zip"
    } else if name.ends_with(".jpeg") || name.ends_with(".jpg") {
        "image/jpeg"
    } else if name.ends_with(".json") {
        "application/json"
    } else {
        "application/octet-stream"
    }
}

fn handle_http_client(mut stream: TcpStream) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return;
    }
    let method = parts[0];
    let raw_path = parts[1];

    let mut range_header: Option<String> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if trimmed.len() > 6 && trimmed[..6].eq_ignore_ascii_case("range:") {
            range_header = Some(trimmed[6..].trim().to_string());
        }
    }

    if method == "OPTIONS" {
        let resp = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, HEAD, OPTIONS\r\nAccess-Control-Allow-Headers: *\r\nAccess-Control-Expose-Headers: Content-Range, Content-Length, Accept-Ranges\r\n\r\n";
        let _ = stream.write_all(resp.as_bytes());
        return;
    }

    let path = raw_path.split('?').next().unwrap_or(raw_path);
    if let Some(name) = path.strip_prefix("/file/") {
        if let Some(file) = resolve_file(name) {
            let total_len = match file.metadata() {
                Ok(m) => m.len(),
                Err(_) => return,
            };
            let mime = guess_mime(name);

            if method == "HEAD" {
                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {total_len}\r\nAccept-Ranges: bytes\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Expose-Headers: Content-Range, Content-Length, Accept-Ranges\r\n\r\n"
                );
                let _ = stream.write_all(hdr.as_bytes());
                return;
            }

            if let Some(r_str) =
                range_header.and_then(|s| s.strip_prefix("bytes=").map(|x| x.to_string()))
            {
                let mut bounds = r_str.split('-');
                let start: u64 = bounds.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                let end: u64 = bounds
                    .next()
                    .and_then(|s| if s.is_empty() { None } else { s.parse().ok() })
                    .unwrap_or_else(|| total_len.saturating_sub(1))
                    .min(total_len.saturating_sub(1));
                if start <= end && start < total_len {
                    let chunk_len = (end - start + 1) as usize;
                    let mut buf = vec![0u8; chunk_len];
                    let _ = file.read_at(&mut buf, start);
                    let hdr = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Type: {mime}\r\nContent-Length: {chunk_len}\r\nContent-Range: bytes {start}-{end}/{total_len}\r\nAccept-Ranges: bytes\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Expose-Headers: Content-Range, Content-Length, Accept-Ranges\r\n\r\n"
                    );
                    let _ = stream.write_all(hdr.as_bytes());
                    let _ = stream.write_all(&buf);
                    return;
                }
            }

            let hdr = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {total_len}\r\nAccept-Ranges: bytes\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Expose-Headers: Content-Range, Content-Length, Accept-Ranges\r\n\r\n"
            );
            if stream.write_all(hdr.as_bytes()).is_err() {
                return;
            }
            let mut offset = 0u64;
            let mut buf = vec![0u8; 256 * 1024];
            while offset < total_len {
                let to_read = ((total_len - offset) as usize).min(buf.len());
                match file.read_at(&mut buf[..to_read], offset) {
                    Ok(0) => break,
                    Ok(n) => {
                        if stream.write_all(&buf[..n]).is_err() {
                            break;
                        }
                        offset += n as u64;
                    }
                    Err(_) => break,
                }
            }
            return;
        }
    } else if let Some(rest) = path.strip_prefix("/cbz-page/") {
        if let Some((cbz_name, page_name)) = rest.split_once('/') {
            if let Some(file) = resolve_file(cbz_name) {
                if let Ok(dup_file) = file.try_clone() {
                    let mut dup_file = dup_file;
                    let _ = dup_file.seek(SeekFrom::Start(0));
                    if let Ok(mut archive) = zip::ZipArchive::new(dup_file) {
                        if let Ok(mut entry) = archive.by_name(page_name) {
                            let mut buf = Vec::with_capacity(entry.size() as usize);
                            if entry.read_to_end(&mut buf).is_ok() {
                                let hdr = format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
                                    buf.len()
                                );
                                let _ = stream.write_all(hdr.as_bytes());
                                let _ = stream.write_all(&buf);
                                return;
                            }
                        }
                    }
                }
            }
        }
    }

    let not_found = "HTTP/1.1 404 Not Found\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 9\r\n\r\nNot Found";
    let _ = stream.write_all(not_found.as_bytes());
}

fn start_local_http_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:18421")
        .or_else(|_| TcpListener::bind("127.0.0.1:0"))
        .expect("Falha ao abrir TcpListener local");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || handle_http_client(stream));
        }
    });
    port
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let port = start_local_http_server();
    let _ = SERVER_PORT.set(port);

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            ping,
            saf_inspect_and_read_cbz,
            register_saf_file,
            try_open_direct_path,
            get_env_info,
            log_result,
            lan_read_simulated_qr,
            lan_discover_mdns,
            lan_test_tls_pinning,
            lan_test_pairing,
            lan_put_position,
            lan_get_position,
            lan_start_sse_listener,
            lan_ack_ui_rendered,
            lan_get_sse_state,
            lan_download_cbz_range,
        ])
        .run(tauri::generate_context!())
        .expect("erro ao iniciar o app Tauri");
}

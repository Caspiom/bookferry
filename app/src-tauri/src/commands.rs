use crate::protocol::ProtocolHandler;
use crate::reader_prefs::{self, ReaderPrefsView};
#[cfg(bookferry_e2e)]
use bookferry_client::PositionOrigin;
use bookferry_client::{Client, DownloadReport, PairedServerInfo, RescanSummary, ResumeDecision};
use bookferry_core::{BookFormat, PositionRecord};
#[cfg(not(target_os = "android"))]
use bookferry_core::{Clock, SystemClock};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::watch;

/// Porta fixa do servidor embutido no desktop (a que o firewall do PC libera).
#[cfg(not(target_os = "android"))]
const DEFAULT_SERVER_PORT: u16 = 47600;
#[cfg(any(not(target_os = "android"), bookferry_e2e))]
use tokio::sync::Mutex as AsyncMutex;

#[cfg(not(target_os = "android"))]
use bookferry_client::tls::{PinnedHttpClient, TlsPin};
#[cfg(not(target_os = "android"))]
use bookferry_core::PairingQrPayload;
#[cfg(not(target_os = "android"))]
use bookferry_server::{
    Config as ServerConfig, DeviceSummary, DevicesListResponse, RevokeDeviceResponse, ServerHandle,
    DEFAULT_PAIRING_TTL,
};
#[cfg(not(target_os = "android"))]
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
#[cfg(not(target_os = "android"))]
use std::time::Duration;

const SETTINGS_FILENAME: &str = "settings.json";

/// Configuracoes persistidas do usuario em `<data_dir>/settings.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppSettings {
    /// Tamanho de fonte padrao do leitor EPUB em pixels (12..=36).
    pub default_font_size: u32,
    /// Preferencia de tema (`"auto"`, `"light"`, `"dark"`, `"sepia"`, `"black"`).
    pub theme: String,
    /// Caminho da pasta de livros (vault_dir) no Desktop.
    pub vault_dir: Option<String>,
    /// Quando `true` no Desktop, nao inicia o servidor embutido e opera como cliente puro.
    pub client_only_mode: bool,
    /// Desktop: fechar a janela mantem o app (e o servidor) rodando na bandeja do sistema.
    /// `serde(default)` e obrigatorio: um `settings.json` antigo sem o campo nao pode resetar
    /// as demais preferencias (o `load` cai em `Default` se o JSON nao casar).
    #[serde(default = "default_close_to_tray")]
    pub close_to_tray: bool,
}

fn default_close_to_tray() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            default_font_size: 18,
            theme: "auto".to_string(),
            vault_dir: None,
            client_only_mode: false,
            close_to_tray: true,
        }
    }
}

impl AppSettings {
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join(SETTINGS_FILENAME);
        match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(data_dir).map_err(|e| format!("erro ao criar data_dir: {e}"))?;
        let path = data_dir.join(SETTINGS_FILENAME);
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("erro ao serializar settings: {e}"))?;
        std::fs::write(path, json).map_err(|e| format!("erro ao gravar settings: {e}"))
    }
}

#[cfg(not(target_os = "android"))]
pub struct DesktopServerRuntime {
    pub handle: ServerHandle,
    pub vault_dir: PathBuf,
    pub qr_payload: PairingQrPayload,
    pub qr_created_at_ms: u64,
    pub qr_expires_at_ms: u64,
    pub initial_paired_count: usize,
    /// `true` quando a porta fixa estava ocupada e o servidor subiu numa porta aleatoria
    /// (o firewall do PC so libera a fixa, entao o celular nao alcancaria o servidor).
    pub port_fallback: bool,
}

/// Estado global gerenciado pelo Tauri (`tauri::State<Arc<AppState>>`).
pub struct AppState {
    pub data_dir: PathBuf,
    pub client: Client,
    pub protocol: ProtocolHandler,
    pub connected: AtomicBool,
    pub settings: StdMutex<AppSettings>,
    pub active_downloads: StdMutex<HashMap<String, watch::Sender<bool>>>,
    /// Serializa os read-modify-write de `reader_prefs.json`.
    pub reader_prefs_lock: StdMutex<()>,
    #[cfg(not(target_os = "android"))]
    pub desktop_server: AsyncMutex<Option<DesktopServerRuntime>>,
    #[cfg(bookferry_e2e)]
    pub e2e_peer: AsyncMutex<Option<E2ePeerState>>,
}

#[cfg(bookferry_e2e)]
pub struct E2ePeerState {
    pub client: Client,
    pub received_positions: Arc<StdMutex<Vec<(u64, PositionRecord, PositionOrigin)>>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BookViewDto {
    pub book_id: String,
    pub partial_md5: String,
    pub title: String,
    pub format: BookFormat,
    pub size_bytes: u64,
    pub mtime_ms: u64,
    pub folder: String,
    pub downloaded: bool,
    pub display_title: Option<String>,
    pub cover_version: u64,
    pub cover_source: Option<String>,
    pub hidden_continue_at_ms: Option<u64>,
    pub position: Option<PositionRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibraryViewDto {
    pub books: Vec<BookViewDto>,
    pub stale: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DownloadReportDto {
    pub book_id: String,
    pub resumed_from_byte: u64,
    pub bytes_transferred_this_call: u64,
    pub total_bytes: u64,
    pub status_code: u16,
}

impl DownloadReportDto {
    fn from_report(book_id: String, r: DownloadReport) -> Self {
        Self {
            book_id,
            resumed_from_byte: r.resumed_from_byte,
            bytes_transferred_this_call: r.bytes_transferred_this_call,
            total_bytes: r.total_bytes,
            status_code: r.status_code,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResumeDecisionDto {
    /// `"nothing" | "use_local" | "use_remote" | "ask"`
    pub decision: String,
    pub position: Option<PositionRecord>,
    pub remote: Option<PositionRecord>,
    pub local: Option<PositionRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CbzInfoDto {
    pub book_id: String,
    pub page_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairingQrViewDto {
    /// SVG completo do QR Code renderizado no backend Rust.
    pub svg: String,
    /// Texto JSON copiavel do payload de pareamento.
    pub qr_json: String,
    pub server_name: String,
    pub ips: Vec<String>,
    pub port: u16,
    pub cert_sha256: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub remaining_ms: u64,
    pub expired: bool,
    pub used: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmbeddedServerStatusDto {
    pub running: bool,
    pub vault_dir: String,
    pub bind_addr: String,
    pub port: u16,
    /// Porta que o servidor tenta usar (a unica liberada no firewall).
    pub preferred_port: u16,
    /// A porta preferida estava ocupada e o servidor esta numa porta aleatoria.
    pub port_is_fallback: bool,
    pub cert_sha256: String,
    pub pairing_qr: Option<PairingQrViewDto>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceViewDto {
    pub id: String,
    pub name: String,
    pub created_at_ms: u64,
    pub last_seen_at_ms: u64,
    pub revoked_at_ms: Option<u64>,
    pub revoked: bool,
    pub is_self: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppStatusDto {
    /// `"desktop"` ou `"android"`
    pub platform: String,
    pub version: String,
    /// `"bookferry://localhost"` no Linux/macOS ou `"http://bookferry.localhost"` no Android e no Windows
    pub protocol_base_url: String,
    pub paired: bool,
    pub paired_server: Option<PairedServerInfo>,
    pub device_id: Option<String>,
    /// `"connected" | "syncing" | "offline"`
    pub connection_state: String,
    pub outbox_len: usize,
    pub settings: AppSettings,
    pub embedded_server: Option<EmbeddedServerStatusDto>,
    pub e2e_mode: bool,
}

pub fn current_protocol_base_url() -> &'static str {
    // O WebView2 (Windows) e o WebView do Android nao deixam a pagina buscar `bookferry://...`:
    // o Tauri serve o protocolo em `http://<esquema>.localhost` nessas duas plataformas.
    if cfg!(any(target_os = "android", windows)) {
        "http://bookferry.localhost"
    } else {
        "bookferry://localhost"
    }
}

pub fn current_platform_name() -> &'static str {
    if cfg!(target_os = "android") {
        "android"
    } else {
        "desktop"
    }
}

#[cfg(not(target_os = "android"))]
fn now_unix_ms() -> u64 {
    SystemClock.now_ms()
}

#[cfg(not(target_os = "android"))]
fn detect_lan_ipv4() -> Option<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.168.1.1:80")
        .or_else(|_| sock.connect("10.0.0.1:80"))
        .or_else(|_| sock.connect("8.8.8.8:80"))
        .ok()?;
    let ip = sock.local_addr().ok()?.ip();
    if ip.is_loopback() || ip.is_unspecified() {
        None
    } else {
        Some(ip)
    }
}

#[cfg(not(target_os = "android"))]
fn render_qr_svg(json_str: &str) -> Result<String, String> {
    use qrcode::render::svg;
    use qrcode::QrCode;

    let code = QrCode::new(json_str.as_bytes()).map_err(|e| format!("erro QR Code: {e}"))?;
    let svg_str = code
        .render()
        .min_dimensions(220, 220)
        .dark_color(svg::Color("#111827"))
        .light_color(svg::Color("#ffffff"))
        .build();
    Ok(svg_str)
}

#[cfg(not(target_os = "android"))]
fn build_qr_view(
    rt: &DesktopServerRuntime,
    active_device_count: usize,
) -> Result<PairingQrViewDto, String> {
    let now_ms = now_unix_ms();
    let qr_json = rt
        .qr_payload
        .to_json_string()
        .map_err(|e| format!("erro ao serializar QR JSON: {e}"))?;
    let svg = render_qr_svg(&qr_json)?;
    let remaining_ms = rt.qr_expires_at_ms.saturating_sub(now_ms);
    let expired = remaining_ms == 0;
    let used = active_device_count > rt.initial_paired_count;

    Ok(PairingQrViewDto {
        svg,
        qr_json,
        server_name: rt.qr_payload.name.clone(),
        ips: rt.qr_payload.ips.clone(),
        port: rt.qr_payload.port,
        cert_sha256: rt.qr_payload.cert_sha256.clone(),
        created_at_ms: rt.qr_created_at_ms,
        expires_at_ms: rt.qr_expires_at_ms,
        remaining_ms,
        expired,
        used,
    })
}

/// Sobe o servidor na porta preferida; se ela estiver ocupada, usa uma porta livre e devolve
/// `true` no segundo item (fallback). Sem porta preferida (E2E) usa direto uma porta livre.
#[cfg(not(target_os = "android"))]
async fn start_on_preferred_port(
    make_cfg: impl Fn(u16) -> ServerConfig,
    preferred: Option<u16>,
) -> Result<(ServerHandle, bool), String> {
    let mut fell_back = false;
    if let Some(port) = preferred {
        match bookferry_server::start(make_cfg(port)).await {
            Ok(handle) => return Ok((handle, false)),
            Err(err) => {
                fell_back = true;
                tracing::warn!(
                    port,
                    error = %err,
                    "porta fixa indisponivel; usando uma porta livre (o firewall pode bloquea-la para o celular)"
                );
            }
        }
    }
    let handle = bookferry_server::start(make_cfg(0))
        .await
        .map_err(|e| format!("falha ao iniciar servidor embutido: {e}"))?;
    Ok((handle, fell_back))
}

#[cfg(not(target_os = "android"))]
pub async fn start_or_restart_desktop_server(
    state: &Arc<AppState>,
    vault_dir: PathBuf,
) -> Result<(), String> {
    if !vault_dir.exists() || !vault_dir.is_dir() {
        return Err(format!(
            "A pasta de livros informada nao existe ou nao e um diretorio: {}",
            vault_dir.display()
        ));
    }

    // Encerra instancia anterior se existir
    {
        let mut guard = state.desktop_server.lock().await;
        if let Some(prev) = guard.take() {
            let _ = prev.handle.shutdown().await;
        }
    }

    let server_data_dir = state.data_dir.join("server");
    std::fs::create_dir_all(&server_data_dir)
        .map_err(|e| format!("falha ao criar diretorio do servidor: {e}"))?;

    let mut announce_ips = Vec::new();
    if let Some(lan_ip) = detect_lan_ipv4() {
        announce_ips.push(lan_ip);
    }
    let loopback_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    if !announce_ips.contains(&loopback_ip) {
        announce_ips.push(loopback_ip);
    }

    let make_cfg = |port: u16| {
        let mut cfg = ServerConfig::new(&server_data_dir, &vault_dir);
        cfg.bind_addr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
        cfg.port = port;
        cfg.announce_ips = announce_ips.clone();
        cfg.server_name = "Bookferry PC".to_string();
        cfg.enable_mdns = !cfg!(bookferry_e2e);
        cfg
    };

    // Porta fixa 47600: é a que o firewall do PC libera (uma porta aleatória fica bloqueada para o celular).
    // Se estiver ocupada (ou no E2E), cai para uma porta livre, e a interface avisa (`port_fallback`).
    let preferred = (!cfg!(bookferry_e2e)).then_some(DEFAULT_SERVER_PORT);
    let (handle, port_fallback) = start_on_preferred_port(make_cfg, preferred).await?;

    let bound_port = handle.local_addr().port();

    // Pareia o cliente local do proprio Desktop em loopback (127.0.0.1), a nao ser que o
    // pareamento da sessao anterior ainda valha para este servidor.
    if loopback_pairing_reusable(state, &handle).await {
        tracing::info!("pareamento loopback anterior reaproveitado");
    } else {
        let mut loopback_qr = handle.pairing_payload();
        loopback_qr.ips = vec!["127.0.0.1".to_string()];
        loopback_qr.ip = Some("127.0.0.1".to_string());

        state
            .client
            .pair(loopback_qr, "Bookferry Desktop")
            .await
            .map_err(|e| format!("falha no auto-pareamento loopback do desktop: {e}"))?;
    }

    // Rotaciona imediatamente o segredo de uso unico para que o QR exibido ao celular seja novo!
    let fresh_qr = handle
        .rotate_pairing_secret()
        .map_err(|e| format!("falha ao gerar segredo de pareamento inicial: {e}"))?;

    let now_ms = now_unix_ms();
    let ttl_ms = DEFAULT_PAIRING_TTL.as_millis() as u64;

    let runtime = DesktopServerRuntime {
        handle,
        vault_dir,
        qr_payload: fresh_qr,
        qr_created_at_ms: now_ms,
        qr_expires_at_ms: now_ms.saturating_add(ttl_ms),
        initial_paired_count: 1,
        port_fallback,
    };

    {
        let mut guard = state.desktop_server.lock().await;
        *guard = Some(runtime);
    }

    tracing::info!(
        port = bound_port,
        "servidor desktop embutido iniciado e pareado em loopback"
    );
    Ok(())
}

/// Lista os aparelhos no servidor embutido usando o token do proprio cliente loopback.
/// `Ok(None)` = nao ha token local para tentar (nao pareado).
#[cfg(not(target_os = "android"))]
async fn fetch_devices_via_loopback(
    state: &Arc<AppState>,
    port: u16,
    paired: &PairedServerInfo,
) -> Result<Option<DevicesListResponse>, String> {
    let token = std::fs::read_to_string(state.client.token_path())
        .map_err(|_| "token local nao encontrado".to_string())?;
    let token = token.trim();
    if token.is_empty() {
        return Ok(None);
    }

    let pin = TlsPin::new(
        Some(paired.cert_sha256.clone()),
        Some(paired.spki_sha256.clone()),
    )
    .map_err(|e| e.to_string())?;
    let http = PinnedHttpClient::new(pin, Duration::from_secs(2), Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);

    let resp: DevicesListResponse = http
        .request_json(
            addr,
            hyper::Method::GET,
            "/devices",
            Some(token),
            Vec::new(),
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(Some(resp))
}

/// O pareamento loopback ja guardado serve para o servidor que acabou de subir? Serve quando e o
/// MESMO servidor (certificado TLS persistido), na mesma porta (e a que o cliente guardou) e o
/// token ainda e aceito. Reaproveitar evita criar um "Bookferry Desktop" novo a cada abertura,
/// o que, com o app iniciando junto com o computador, entupiria a lista de aparelhos.
#[cfg(not(target_os = "android"))]
async fn loopback_pairing_reusable(state: &Arc<AppState>, handle: &ServerHandle) -> bool {
    let Ok(Some(paired)) = state.client.paired_info() else {
        return false;
    };
    let payload = handle.pairing_payload();
    let port = handle.local_addr().port();
    if !pairing_matches_server(&paired, &payload.cert_sha256, port) {
        return false;
    }
    matches!(
        fetch_devices_via_loopback(state, port, &paired).await,
        Ok(Some(_))
    )
}

/// Parte pura da decisao de reaproveitar o pareamento (testavel sem servidor).
#[cfg(not(target_os = "android"))]
fn pairing_matches_server(paired: &PairedServerInfo, server_cert_sha256: &str, port: u16) -> bool {
    paired.cert_sha256 == server_cert_sha256 && paired.port == port
}

#[cfg(not(target_os = "android"))]
async fn query_embedded_devices(state: &Arc<AppState>) -> Result<Vec<DeviceViewDto>, String> {
    let port = {
        let guard = state.desktop_server.lock().await;
        let Some(ref rt) = *guard else {
            return Ok(Vec::new());
        };
        rt.handle.local_addr().port()
    };

    let Some(paired) = state.client.paired_info().map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    let Some(resp) = fetch_devices_via_loopback(state, port, &paired).await? else {
        return Ok(Vec::new());
    };

    let my_id = paired.device_id;
    let list = resp
        .devices
        .into_iter()
        .map(|d: DeviceSummary| DeviceViewDto {
            is_self: d.id == my_id,
            id: d.id,
            name: d.name,
            created_at_ms: d.created_at_ms,
            last_seen_at_ms: d.last_seen_at_ms,
            revoked_at_ms: d.revoked_at_ms,
            revoked: d.revoked,
        })
        .collect();
    Ok(list)
}

pub async fn build_status_dto(state: &Arc<AppState>) -> Result<AppStatusDto, String> {
    let paired = state.client.is_paired().map_err(|e| e.to_string())?;
    let paired_server = state.client.paired_info().map_err(|e| e.to_string())?;
    let device_id = state.client.device_id().map_err(|e| e.to_string())?;
    let outbox_len = state.client.outbox_len().map_err(|e| e.to_string())?;
    let is_conn = state.connected.load(AtomicOrdering::SeqCst);

    let connection_state = if !paired {
        "offline".to_string()
    } else if is_conn && outbox_len > 0 {
        "syncing".to_string()
    } else if is_conn {
        "connected".to_string()
    } else {
        "offline".to_string()
    };

    let settings = state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();

    #[cfg(not(target_os = "android"))]
    let embedded_server = {
        let active_devices = query_embedded_devices(state)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|d| !d.revoked)
            .count();
        let guard = state.desktop_server.lock().await;
        if let Some(ref rt) = *guard {
            let qr_view = build_qr_view(rt, active_devices).ok();
            Some(EmbeddedServerStatusDto {
                running: true,
                vault_dir: rt.vault_dir.to_string_lossy().to_string(),
                bind_addr: rt.handle.local_addr().ip().to_string(),
                port: rt.handle.local_addr().port(),
                preferred_port: DEFAULT_SERVER_PORT,
                port_is_fallback: rt.port_fallback,
                cert_sha256: rt.handle.cert_sha256().to_string(),
                pairing_qr: qr_view,
            })
        } else {
            None
        }
    };

    #[cfg(target_os = "android")]
    let embedded_server: Option<EmbeddedServerStatusDto> = None;

    let e2e_mode =
        cfg!(bookferry_e2e) && std::env::var("BOOKFERRY_E2E").ok().as_deref() == Some("1");

    Ok(AppStatusDto {
        platform: current_platform_name().to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        protocol_base_url: current_protocol_base_url().to_string(),
        paired,
        paired_server,
        device_id,
        connection_state,
        outbox_len,
        settings,
        embedded_server,
        e2e_mode,
    })
}

// ============================================================================
// Comandos Tauri Expostos ao Frontend
// ============================================================================

#[tauri::command]
pub async fn status(state: tauri::State<'_, Arc<AppState>>) -> Result<AppStatusDto, String> {
    build_status_dto(state.inner()).await
}

#[tauri::command]
pub async fn pair(
    state: tauri::State<'_, Arc<AppState>>,
    qr_json: String,
    device_name: Option<String>,
) -> Result<PairedServerInfo, String> {
    let default_name = if cfg!(target_os = "android") {
        "Bookferry Android"
    } else {
        "Bookferry Desktop"
    };
    let effective_name = device_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(default_name);

    state
        .client
        .pair(&qr_json, effective_name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn unpair(state: tauri::State<'_, Arc<AppState>>) -> Result<(), String> {
    state.client.unpair().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn library(state: tauri::State<'_, Arc<AppState>>) -> Result<LibraryViewDto, String> {
    let snap = state.client.library().await.map_err(|e| e.to_string())?;
    let books = snap
        .books
        .into_iter()
        .map(|b| {
            let downloaded = b.local_path.is_some()
                || state.client.local_path(&b.book_id).ok().flatten().is_some();
            BookViewDto {
                book_id: b.book_id,
                partial_md5: b.partial_md5,
                title: b.title,
                format: b.format,
                size_bytes: b.size_bytes,
                mtime_ms: b.mtime_ms,
                folder: b.folder,
                downloaded,
                display_title: b.display_title,
                cover_version: b.cover_version,
                cover_source: b.cover_source,
                hidden_continue_at_ms: b.hidden_continue_at_ms,
                position: b.position,
            }
        })
        .collect();
    Ok(LibraryViewDto {
        books,
        stale: snap.stale,
    })
}

/// Resposta das escritas de metadados (nome, capa, "Continuar lendo").
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BookMetaDto {
    pub book_id: String,
    pub display_title: Option<String>,
    pub cover_version: u64,
    pub cover_source: Option<String>,
    pub hidden_continue_at_ms: Option<u64>,
    /// `false` quando uma capa automatica foi ignorada por ja existir capa escolhida pelo usuario.
    pub applied: bool,
}

impl BookMetaDto {
    fn from_meta(meta: bookferry_client::BookMeta, applied: bool) -> Self {
        Self {
            book_id: meta.book_id,
            display_title: meta.display_title,
            cover_version: meta.cover_version,
            cover_source: meta.cover_source,
            hidden_continue_at_ms: meta.hidden_continue_at_ms,
            applied,
        }
    }
}

#[tauri::command]
pub async fn set_display_title(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
    title: Option<String>,
) -> Result<BookMetaDto, String> {
    let meta = state
        .client
        .set_display_title(&book_id, title.as_deref())
        .await
        .map_err(|e| e.to_string())?;
    Ok(BookMetaDto::from_meta(meta, true))
}

#[tauri::command]
pub async fn set_continue_hidden(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
    hidden: bool,
) -> Result<BookMetaDto, String> {
    let meta = state
        .client
        .set_continue_hidden(&book_id, hidden)
        .await
        .map_err(|e| e.to_string())?;
    Ok(BookMetaDto::from_meta(meta, true))
}

/// Envia a capa de um livro. `source`: `embedded` (gerada do arquivo), `custom` (imagem do
/// usuario) ou `web` (buscada na internet por escolha do usuario).
#[tauri::command]
pub async fn set_book_cover(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
    source: String,
    image: Vec<u8>,
) -> Result<BookMetaDto, String> {
    let source = match source.as_str() {
        "embedded" => bookferry_client::CoverSource::Embedded,
        "custom" => bookferry_client::CoverSource::Custom,
        "web" => bookferry_client::CoverSource::Web,
        other => return Err(format!("origem de capa invalida: {other}")),
    };
    let (applied, meta) = state
        .client
        .set_cover(&book_id, source, image)
        .await
        .map_err(|e| e.to_string())?;
    Ok(BookMetaDto::from_meta(meta, applied))
}

#[tauri::command]
pub async fn remove_book_cover(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
) -> Result<BookMetaDto, String> {
    let meta = state
        .client
        .remove_cover(&book_id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(BookMetaDto::from_meta(meta, true))
}

/// Garante a capa no disco (baixa se preciso) e devolve a URL do protocolo local, ou `None`
/// se o livro nao tem capa ou ela nao esta disponivel agora (offline e nunca baixada).
#[tauri::command]
pub async fn ensure_cover(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
) -> Result<Option<String>, String> {
    let version = match state.client.cover_path(&book_id).await {
        Ok(Some(path)) => path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.rsplit_once('-'))
            .map(|(_, v)| v.to_string()),
        Ok(None) => None,
        Err(e) if e.is_transient() => None,
        Err(e) => return Err(e.to_string()),
    };
    Ok(version.map(|v| format!("{}/covers/{book_id}/{v}", current_protocol_base_url())))
}

/// Ids de livros sem capa no servidor (o app gera a embutida para eles em segundo plano).
#[tauri::command]
pub async fn list_books_without_cover(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<Vec<String>, String> {
    state
        .client
        .books_without_cover()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rescan(state: tauri::State<'_, Arc<AppState>>) -> Result<RescanSummary, String> {
    state.client.rescan().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn download_book(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
) -> Result<DownloadReportDto, String> {
    let (cancel_tx, cancel_rx) = watch::channel(false);
    {
        let mut map = state
            .active_downloads
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.insert(book_id.clone(), cancel_tx);
    }

    let res = state
        .client
        .download_book_with_options(&book_id, None, Some(cancel_rx), None)
        .await;

    {
        let mut map = state
            .active_downloads
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.remove(&book_id);
    }

    let report = res.map_err(|e| e.to_string())?;
    Ok(DownloadReportDto::from_report(book_id, report))
}

#[tauri::command]
pub async fn cancel_download(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
) -> Result<bool, String> {
    let tx_opt = {
        let mut map = state
            .active_downloads
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        map.remove(&book_id)
    };
    if let Some(tx) = tx_opt {
        let _ = tx.send(true);
        Ok(true)
    } else {
        Ok(false)
    }
}

#[tauri::command]
pub async fn remove_download(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
) -> Result<bool, String> {
    state
        .client
        .remove_download(&book_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_position(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
    locator: String,
    snippet: Option<String>,
    percent: f64,
) -> Result<PositionRecord, String> {
    state
        .client
        .set_position(&book_id, &locator, snippet, percent)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_position(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
) -> Result<Option<PositionRecord>, String> {
    state
        .client
        .get_position(&book_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn decide_book_resume(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
) -> Result<ResumeDecisionDto, String> {
    // Tenta reconciliacao rapida antes de decidir o resume (ignora erro se offline)
    let _ = state.client.pull().await;

    let decision = state
        .client
        .decide_book_resume(&book_id)
        .map_err(|e| e.to_string())?;
    let winning = state
        .client
        .get_position(&book_id)
        .await
        .map_err(|e| e.to_string())?;

    match decision {
        ResumeDecision::Nothing => Ok(ResumeDecisionDto {
            decision: "nothing".to_string(),
            position: None,
            remote: None,
            local: None,
        }),
        ResumeDecision::UseLocal => Ok(ResumeDecisionDto {
            decision: "use_local".to_string(),
            position: winning.clone(),
            remote: None,
            local: winning,
        }),
        ResumeDecision::UseRemote => Ok(ResumeDecisionDto {
            decision: "use_remote".to_string(),
            position: winning.clone(),
            remote: winning,
            local: None,
        }),
        ResumeDecision::Ask { remote, local } => Ok(ResumeDecisionDto {
            decision: "ask".to_string(),
            position: winning,
            remote: Some(remote),
            local: Some(local),
        }),
    }
}

#[tauri::command]
pub async fn on_resume(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<Vec<PositionRecord>, String> {
    state.client.on_resume().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn cbz_info(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
) -> Result<CbzInfoDto, String> {
    let protocol = state.protocol.clone();
    let bid = book_id.clone();
    let page_count = tokio::task::spawn_blocking(move || protocol.inspect_cbz_page_count(&bid))
        .await
        .map_err(|e| format!("falha na tarefa CBZ: {e}"))??;
    Ok(CbzInfoDto {
        book_id,
        page_count,
    })
}

#[tauri::command]
pub async fn save_settings(
    state: tauri::State<'_, Arc<AppState>>,
    default_font_size: Option<u32>,
    theme: Option<String>,
    client_only_mode: Option<bool>,
    close_to_tray: Option<bool>,
) -> Result<AppStatusDto, String> {
    {
        let mut settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sz) = default_font_size {
            settings.default_font_size = sz.clamp(12, 36);
        }
        if let Some(th) = theme {
            let clean = th.trim().to_ascii_lowercase();
            if matches!(
                clean.as_str(),
                "auto" | "light" | "dark" | "sepia" | "black"
            ) {
                settings.theme = clean;
            }
        }
        if let Some(co) = client_only_mode {
            settings.client_only_mode = co;
        }
        if let Some(ct) = close_to_tray {
            settings.close_to_tray = ct;
        }
        settings.save(&state.data_dir)?;
    }
    build_status_dto(state.inner()).await
}

/// Abre o seletor de pasta do sistema. Devolve `None` se a pessoa cancelar.
/// `start_dir` (se for uma pasta que existe) e onde o seletor abre.
#[tauri::command]
pub async fn pick_vault_folder(
    app: tauri::AppHandle,
    start_dir: Option<String>,
) -> Result<Option<String>, String> {
    #[cfg(target_os = "android")]
    {
        let _ = (app, start_dir);
        Err("O seletor de pasta so existe no computador".to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        use tauri_plugin_dialog::DialogExt;
        let mut dialog = app
            .dialog()
            .file()
            .set_title("Escolha a pasta onde ficam seus livros");
        if let Some(dir) = start_dir.map(PathBuf::from).filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        // O seletor bloqueia ate a pessoa escolher; nao pode rodar na thread principal.
        let picked = tauri::async_runtime::spawn_blocking(move || dialog.blocking_pick_folder())
            .await
            .map_err(|e| format!("o seletor de pasta falhou: {e}"))?;
        match picked {
            None => Ok(None),
            Some(path) => path
                .into_path()
                .map(|p| Some(p.to_string_lossy().into_owned()))
                .map_err(|e| format!("pasta invalida: {e}")),
        }
    }
}

#[tauri::command]
pub async fn configure_server_vault(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    vault_dir: String,
) -> Result<AppStatusDto, String> {
    #[cfg(target_os = "android")]
    {
        let _ = (app, state, vault_dir);
        Err("O servidor embutido nao esta disponivel no Android".to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let trimmed = vault_dir.trim().to_string();
        if trimmed.is_empty() {
            return Err("Informe um caminho valido para a pasta de livros".to_string());
        }
        let path = PathBuf::from(&trimmed);
        start_or_restart_desktop_server(state.inner(), path).await?;

        {
            let mut settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
            settings.vault_dir = Some(trimmed);
            settings.client_only_mode = false;
            settings.save(&state.data_dir)?;
        }

        // A linha de status da bandeja deixa de dizer "servidor desligado".
        let port = state
            .desktop_server
            .lock()
            .await
            .as_ref()
            .map(|rt| rt.handle.local_addr().port());
        crate::desktop::refresh_tray_status(&app, port);

        build_status_dto(state.inner()).await
    }
}

/// Reinicia o servidor para tentar voltar a porta fixa (usado quando ela estava ocupada).
/// Devolve `true` se AINDA esta numa porta aleatoria. O segredo de pareamento muda.
#[tauri::command]
pub async fn retry_preferred_port(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        let _ = (app, state);
        Err("O servidor embutido nao esta disponivel no Android".to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let vault_dir = {
            let guard = state.desktop_server.lock().await;
            let Some(ref rt) = *guard else {
                return Err("Servidor desktop embutido nao esta em execucao".to_string());
            };
            rt.vault_dir.clone()
        };
        start_or_restart_desktop_server(state.inner(), vault_dir).await?;
        let (port, still_fallback) = {
            let guard = state.desktop_server.lock().await;
            let rt = guard.as_ref().ok_or("servidor nao subiu")?;
            (rt.handle.local_addr().port(), rt.port_fallback)
        };
        crate::desktop::refresh_tray_status(&app, Some(port));
        Ok(still_fallback)
    }
}

/// Estado do "iniciar com o computador" (a verdade e o arquivo de autostart do sistema, nao um
/// campo em `settings.json`: assim some junto se o usuario apagar a entrada por fora).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AutostartDto {
    pub supported: bool,
    pub enabled: bool,
}

#[tauri::command]
pub fn get_autostart() -> AutostartDto {
    #[cfg(target_os = "android")]
    {
        AutostartDto {
            supported: false,
            enabled: false,
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        AutostartDto {
            supported: crate::desktop::autostart_supported(),
            enabled: crate::desktop::autostart_enabled(),
        }
    }
}

#[tauri::command]
pub fn set_autostart(enabled: bool) -> Result<AutostartDto, String> {
    #[cfg(target_os = "android")]
    {
        let _ = enabled;
        Err("Iniciar com o computador nao existe no Android".to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        crate::desktop::set_autostart(enabled)?;
        Ok(get_autostart())
    }
}

#[tauri::command]
pub async fn rotate_pairing_qr(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<PairingQrViewDto, String> {
    #[cfg(target_os = "android")]
    {
        let _ = state;
        Err("QR de servidor disponivel apenas no Desktop".to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let active_devices = query_embedded_devices(state.inner())
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|d| !d.revoked)
            .count();

        let mut guard = state.desktop_server.lock().await;
        let Some(ref mut rt) = *guard else {
            return Err("Servidor desktop embutido nao esta em execucao".to_string());
        };
        let new_qr = rt
            .handle
            .rotate_pairing_secret()
            .map_err(|e| format!("falha ao rotacionar segredo QR: {e}"))?;
        let now_ms = now_unix_ms();
        let ttl_ms = DEFAULT_PAIRING_TTL.as_millis() as u64;
        rt.qr_payload = new_qr;
        rt.qr_created_at_ms = now_ms;
        rt.qr_expires_at_ms = now_ms.saturating_add(ttl_ms);
        rt.initial_paired_count = active_devices;

        build_qr_view(rt, active_devices)
    }
}

#[tauri::command]
pub async fn list_paired_devices(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<Vec<DeviceViewDto>, String> {
    #[cfg(target_os = "android")]
    {
        let _ = state;
        Ok(Vec::new())
    }
    #[cfg(not(target_os = "android"))]
    {
        query_embedded_devices(state.inner()).await
    }
}

#[tauri::command]
pub async fn revoke_paired_device(
    state: tauri::State<'_, Arc<AppState>>,
    device_id: String,
) -> Result<bool, String> {
    #[cfg(target_os = "android")]
    {
        let _ = (state, device_id);
        Err("Revogacao de dispositivos disponivel apenas no servidor Desktop".to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let port = {
            let guard = state.desktop_server.lock().await;
            let Some(ref rt) = *guard else {
                return Err("Servidor embutido nao iniciado".to_string());
            };
            rt.handle.local_addr().port()
        };

        let Some(paired) = state.client.paired_info().map_err(|e| e.to_string())? else {
            return Err("Cliente desktop nao pareado".to_string());
        };
        if device_id.trim() == paired.device_id {
            return Err(
                "Nao e permitido revogar o proprio cliente loopback do Desktop".to_string(),
            );
        }

        let token = std::fs::read_to_string(state.client.token_path())
            .map_err(|_| "token local ausente".to_string())?;
        let token = token.trim();

        let pin = TlsPin::new(
            Some(paired.cert_sha256.clone()),
            Some(paired.spki_sha256.clone()),
        )
        .map_err(|e| e.to_string())?;
        let http = PinnedHttpClient::new(pin, Duration::from_secs(2), Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        let uri = format!("/devices/{}", device_id.trim());

        let resp: RevokeDeviceResponse = http
            .request_json(addr, hyper::Method::DELETE, &uri, Some(token), Vec::new())
            .await
            .map_err(|e| e.to_string())?;

        Ok(resp.revoked)
    }
}

// ============================================================================
// Harness de Teste E2E (Compilado APENAS quando `BOOKFERRY_E2E=1` no build)
// ============================================================================

#[cfg(bookferry_e2e)]
fn ensure_e2e_runtime_enabled() -> Result<(), String> {
    if std::env::var("BOOKFERRY_E2E").as_deref() == Ok("1") {
        Ok(())
    } else {
        Err("BOOKFERRY_E2E nao esta ativo em runtime".to_string())
    }
}

#[cfg(bookferry_e2e)]
#[tauri::command]
pub async fn e2e_pair_second_client(
    state: tauri::State<'_, Arc<AppState>>,
    device_name: Option<String>,
) -> Result<serde_json::Value, String> {
    ensure_e2e_runtime_enabled()?;
    let peer_name = device_name.unwrap_or_else(|| "Galaxy S23".to_string());

    // Obtem o QR atual do servidor embutido e pareia um segundo `bookferry_client::Client`
    let mut qr = {
        let guard = state.desktop_server.lock().await;
        let Some(ref rt) = *guard else {
            return Err("Servidor desktop nao iniciado para E2E".to_string());
        };
        rt.qr_payload.clone()
    };
    qr.ips = vec!["127.0.0.1".to_string()];
    qr.ip = Some("127.0.0.1".to_string());

    let peer_dir = state.data_dir.join("e2e-mobile-client");
    let _ = std::fs::remove_dir_all(&peer_dir);
    std::fs::create_dir_all(&peer_dir).map_err(|e| e.to_string())?;

    let mut cfg = bookferry_client::ClientConfig::new(&peer_dir, &peer_name);
    cfg.sse_backoff_initial = Duration::from_millis(80);
    cfg.outbox_backoff_initial = Duration::from_millis(50);

    let peer_client = Client::open(cfg).await.map_err(|e| e.to_string())?;
    let mut rx = peer_client.subscribe();
    let received_positions: Arc<StdMutex<Vec<(u64, PositionRecord, PositionOrigin)>>> =
        Arc::new(StdMutex::new(Vec::new()));
    let recv_clone = received_positions.clone();

    tokio::spawn(async move {
        while let Ok(ev) = rx.recv().await {
            if let bookferry_client::ClientEvent::PositionChanged {
                position, origin, ..
            } = ev
            {
                let ts = SystemClock.now_ms();
                recv_clone
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((ts, position, origin));
            }
        }
    });

    let info = peer_client
        .pair(qr, &peer_name)
        .await
        .map_err(|e| format!("falha ao parear segundo cliente E2E: {e}"))?;

    // Rotaciona o QR do servidor para que a tela de Configuracoes exiba um segredo novo
    {
        let mut guard = state.desktop_server.lock().await;
        if let Some(ref mut rt) = *guard {
            if let Ok(new_qr) = rt.handle.rotate_pairing_secret() {
                let now_ms = now_unix_ms();
                rt.qr_payload = new_qr;
                rt.qr_created_at_ms = now_ms;
                rt.qr_expires_at_ms = now_ms + 300_000;
                rt.initial_paired_count = 2;
            }
        }
    }

    let snap = peer_client.library().await.map_err(|e| e.to_string())?;
    let book_count = snap.books.len();

    {
        let mut guard = state.e2e_peer.lock().await;
        *guard = Some(E2ePeerState {
            client: peer_client,
            received_positions,
        });
    }

    Ok(serde_json::json!({
        "paired": true,
        "device_id": info.device_id,
        "device_name": info.device_name,
        "peer_library_books": book_count,
    }))
}

#[cfg(bookferry_e2e)]
#[tauri::command]
pub async fn e2e_wait_peer_position(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
    expected_locator_prefix: Option<String>,
    min_percent: Option<f64>,
    timeout_ms: Option<u64>,
) -> Result<serde_json::Value, String> {
    ensure_e2e_runtime_enabled()?;
    let max_wait = Duration::from_millis(timeout_ms.unwrap_or(5_000));
    let start = std::time::Instant::now();

    loop {
        {
            let guard = state.e2e_peer.lock().await;
            let Some(ref peer) = *guard else {
                return Err("Segundo cliente E2E nao inicializado".to_string());
            };

            // Checa eventos recebidos via SSE no segundo cliente
            let list = peer
                .received_positions
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some((_, pos, origin)) = list.iter().rev().find(|(_, p, orig)| {
                p.book_id == book_id
                    && *orig == PositionOrigin::RemoteSse
                    && expected_locator_prefix
                        .as_deref()
                        .is_none_or(|pref| p.locator.starts_with(pref))
                    && min_percent.is_none_or(|min_p| p.percent >= min_p)
            }) {
                let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                return Ok(serde_json::json!({
                    "ok": true,
                    "via": format!("{origin:?}"),
                    "elapsed_ms": (elapsed_ms * 100.0).round() / 100.0,
                    "position": pos,
                }));
            }
        }

        if start.elapsed() >= max_wait {
            return Err(format!(
                "Timeout ({max_wait:?}) aguardando posicao SSE no segundo cliente para book_id={book_id}"
            ));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(bookferry_e2e)]
#[tauri::command]
pub async fn e2e_peer_set_position(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
    locator: String,
    snippet: Option<String>,
    percent: f64,
) -> Result<PositionRecord, String> {
    ensure_e2e_runtime_enabled()?;
    let peer_client = {
        let guard = state.e2e_peer.lock().await;
        let Some(ref peer) = *guard else {
            return Err("Segundo cliente E2E nao inicializado".to_string());
        };
        peer.client.clone()
    };

    let rec = peer_client
        .set_position(&book_id, &locator, snippet, percent)
        .await
        .map_err(|e| e.to_string())?;
    let _ = peer_client
        .flush_outbox()
        .await
        .map_err(|e| e.to_string())?;
    Ok(rec)
}

#[cfg(bookferry_e2e)]
#[tauri::command]
pub async fn e2e_capture_screenshot(name: String) -> Result<serde_json::Value, String> {
    ensure_e2e_runtime_enabled()?;
    let safe_name: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if safe_name.is_empty() {
        return Err("Nome de screenshot invalido".to_string());
    }

    let shots_dir = std::env::var("BOOKFERRY_SCREENSHOTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("tests/screenshots"));
    std::fs::create_dir_all(&shots_dir).map_err(|e| e.to_string())?;
    let out_path = shots_dir.join(format!("{safe_name}.png"));
    let out_str = out_path.to_string_lossy().to_string();

    let status = tokio::task::spawn_blocking(move || {
        std::process::Command::new("import")
            .env_remove("LD_LIBRARY_PATH")
            .args(["-window", "root", &out_str])
            .status()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("falha ao executar `import -window root`: {e}"))?;

    if !status.success() {
        return Err(format!("`import -window root` retornou status {status}"));
    }

    let meta = std::fs::metadata(&out_path).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "name": safe_name,
        "path": out_path.to_string_lossy(),
        "bytes": meta.len(),
    }))
}

#[cfg(bookferry_e2e)]
#[tauri::command]
pub async fn e2e_finish(report_json: String) -> Result<(), String> {
    ensure_e2e_runtime_enabled()?;
    if let Ok(out_file) = std::env::var("BOOKFERRY_E2E_OUT") {
        let _ = std::fs::write(&out_file, &report_json);
    }
    println!("[BOOKFERRY_E2E_REPORT] {report_json}");
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(250)).await;
        std::process::exit(0);
    });
    Ok(())
}

// ---------------------------------------------------------------------------
// Preferencias de leitura (ver `reader_prefs.rs`)
// ---------------------------------------------------------------------------

/// Executa `f` sob o mutex que serializa o acesso a `reader_prefs.json`.
fn with_prefs_lock<T>(state: &AppState, f: impl FnOnce(&Path) -> T) -> T {
    let _guard = state
        .reader_prefs_lock
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    f(&state.data_dir)
}

#[tauri::command]
pub async fn get_reader_prefs(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
) -> Result<ReaderPrefsView, String> {
    with_prefs_lock(&state, |dir| reader_prefs::op_get(dir, &book_id))
}

#[tauri::command]
pub async fn save_reader_prefs(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
    prefs: serde_json::Value,
) -> Result<ReaderPrefsView, String> {
    with_prefs_lock(&state, |dir| {
        reader_prefs::op_save_book(dir, &book_id, &prefs)
    })
}

#[tauri::command]
pub async fn reset_reader_prefs(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
) -> Result<ReaderPrefsView, String> {
    with_prefs_lock(&state, |dir| reader_prefs::op_reset_book(dir, &book_id))
}

#[tauri::command]
pub async fn set_default_reader_prefs(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
    prefs: serde_json::Value,
) -> Result<ReaderPrefsView, String> {
    with_prefs_lock(&state, |dir| {
        reader_prefs::op_set_default(dir, &book_id, &prefs)
    })
}

#[tauri::command]
pub async fn save_reader_preset(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
    name: String,
    prefs: serde_json::Value,
) -> Result<ReaderPrefsView, String> {
    with_prefs_lock(&state, |dir| {
        reader_prefs::op_save_preset(dir, &book_id, &name, &prefs)
    })
}

#[tauri::command]
pub async fn delete_reader_preset(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
    name: String,
) -> Result<ReaderPrefsView, String> {
    with_prefs_lock(&state, |dir| {
        reader_prefs::op_delete_preset(dir, &book_id, &name)
    })
}

#[tauri::command]
pub async fn save_progress_prefs(
    state: tauri::State<'_, Arc<AppState>>,
    book_id: String,
    progress: serde_json::Value,
) -> Result<ReaderPrefsView, String> {
    with_prefs_lock(&state, |dir| {
        reader_prefs::op_save_progress(dir, &book_id, &progress)
    })
}

/// Liga/desliga tela cheia. No Android a barra de sistema e controlada pela Activity,
/// entao aqui e um no-op.
#[tauri::command]
pub async fn set_fullscreen(fullscreen: bool, window: tauri::WebviewWindow) -> Result<(), String> {
    #[cfg(not(target_os = "android"))]
    {
        window
            .set_fullscreen(fullscreen)
            .map_err(|e| format!("erro ao alternar tela cheia: {e}"))
    }
    #[cfg(target_os = "android")]
    {
        let _ = (fullscreen, window);
        Ok(())
    }
}

#[tauri::command]
pub async fn is_fullscreen(window: tauri::WebviewWindow) -> Result<bool, String> {
    #[cfg(not(target_os = "android"))]
    {
        window
            .is_fullscreen()
            .map_err(|e| format!("erro ao consultar tela cheia: {e}"))
    }
    #[cfg(target_os = "android")]
    {
        let _ = window;
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cfg(dir: &std::path::Path) -> impl Fn(u16) -> ServerConfig + '_ {
        move |port| {
            let mut cfg = ServerConfig::new(dir.join("data"), dir.join("vault"));
            cfg.bind_addr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
            cfg.port = port;
            cfg.enable_mdns = false;
            cfg
        }
    }

    #[test]
    fn url_do_protocolo_usa_http_localhost_no_windows_e_android() {
        // WebView2 e o WebView do Android nao buscam `bookferry://`: so `http://bookferry.localhost`.
        let esperado = if cfg!(any(target_os = "android", windows)) {
            "http://bookferry.localhost"
        } else {
            "bookferry://localhost"
        };
        assert_eq!(current_protocol_base_url(), esperado);
    }

    #[tokio::test]
    async fn porta_preferida_ocupada_cai_para_porta_livre_e_sinaliza_fallback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("vault")).unwrap();
        // Ocupa uma porta do mesmo jeito que outro programa ocuparia a 47600.
        let squatter = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        let busy = squatter.local_addr().unwrap().port();

        let (handle, fell_back) = start_on_preferred_port(test_cfg(dir.path()), Some(busy))
            .await
            .unwrap();
        assert!(fell_back, "porta ocupada deve sinalizar fallback");
        assert_ne!(handle.local_addr().port(), busy);
        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn porta_preferida_livre_e_usada_sem_fallback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("vault")).unwrap();
        // Descobre uma porta livre e a solta para o servidor usar.
        let free = {
            let l = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let (handle, fell_back) = start_on_preferred_port(test_cfg(dir.path()), Some(free))
            .await
            .unwrap();
        assert!(!fell_back);
        assert_eq!(handle.local_addr().port(), free);
        handle.shutdown().await.unwrap();
    }

    #[test]
    fn settings_antigo_sem_close_to_tray_mantem_as_outras_preferencias() {
        // settings.json gravado antes da bandeja existir: nao pode voltar tudo ao padrao.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(SETTINGS_FILENAME),
            r#"{"default_font_size":26,"theme":"black","vault_dir":"/livros","client_only_mode":false}"#,
        )
        .unwrap();
        let s = AppSettings::load(dir.path());
        assert_eq!(s.default_font_size, 26);
        assert_eq!(s.theme, "black");
        assert_eq!(s.vault_dir.as_deref(), Some("/livros"));
        assert!(s.close_to_tray, "padrao: fechar mantem na bandeja");
    }

    #[test]
    fn close_to_tray_desligado_sobrevive_ao_salvar_e_carregar() {
        let dir = tempfile::tempdir().unwrap();
        let s = AppSettings {
            close_to_tray: false,
            ..AppSettings::default()
        };
        s.save(dir.path()).unwrap();
        assert!(!AppSettings::load(dir.path()).close_to_tray);
    }

    #[cfg(not(target_os = "android"))]
    fn paired(cert: &str, port: u16) -> PairedServerInfo {
        PairedServerInfo {
            device_id: "dev_x".into(),
            device_name: "Bookferry Desktop".into(),
            server_name: "Bookferry PC".into(),
            port,
            last_good_ip: Some("127.0.0.1".into()),
            known_ips: vec!["127.0.0.1".into()],
            cert_sha256: cert.into(),
            spki_sha256: "spki".into(),
            paired_at_ms: 1,
        }
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn pareamento_so_e_reaproveitado_para_o_mesmo_servidor_e_a_mesma_porta() {
        let p = paired("aaaa", 47600);
        assert!(pairing_matches_server(&p, "aaaa", 47600));
        // certificado diferente = outro servidor (dados apagados): precisa parear de novo
        assert!(!pairing_matches_server(&p, "bbbb", 47600));
        // o cliente guardou a porta antiga; com outra porta ele nao conecta
        assert!(!pairing_matches_server(&p, "aaaa", 51234));
    }
}

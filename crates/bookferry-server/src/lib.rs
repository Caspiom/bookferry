#![forbid(unsafe_code)]

pub mod db;
pub mod error;
pub mod library;
pub mod mdns;
pub mod meta;
pub mod routes;
pub mod tls;

pub use crate::db::{BookWithPosition, DeviceSummary, SsePositionPayload};
pub use crate::error::{ApiError, ErrorBody, ServerError, HEADER_BOOKFERRY_API};
pub use crate::meta::{BookMetaRecord, CoverSource, SseBookMetaPayload, COVER_MAX_BYTES};
pub use crate::routes::{
    BookMetaResponse, BooksListResponse, ContinueHiddenRequest, DevicesListResponse,
    DisplayTitleRequest, GetPositionResponse, PairRequest, PairResponse, PositionsListResponse,
    PutPositionResponse, RescanResponse, RevokeDeviceResponse, StatusResponse,
};
pub use crate::tls::{
    build_pinned_client_config, extract_spki_from_x509_der, PinnedServerCertVerifier,
    CERT_DER_FILENAME, KEY_DER_FILENAME,
};

use crate::db::Database;
use crate::library::scan_vault_dir;
use crate::mdns::MdnsHandle;
use crate::routes::{build_router, AppState, PairingSession};
use crate::tls::load_or_generate_tls;
use bookferry_core::{
    generate_random_hex, Clock, HlcClock, PairingQrPayload, SystemClock,
    DEFAULT_MAX_FUTURE_DRIFT_MS, DEFAULT_POSITION_HISTORY_LIMIT,
};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;
use hyper_util::service::TowerToHyperService;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

/// Tamanho maximo padrao do corpo JSON (64 KiB).
pub const DEFAULT_MAX_JSON_BODY_BYTES: usize = 64 * 1024;

/// Validade padrao do segredo de pareamento (5 minutos).
pub const DEFAULT_PAIRING_TTL: Duration = Duration::from_secs(300);

/// Limite padrao de tentativas falhas de pareamento antes de bloqueio temporario.
pub const DEFAULT_MAX_FAILED_PAIR_ATTEMPTS: u32 = 5;

/// Duracao padrao do bloqueio apos exceder tentativas falhas de pareamento (60 s).
pub const DEFAULT_PAIR_LOCKOUT_DURATION: Duration = Duration::from_secs(60);

/// Atraso padrao em tentativa falha de pareamento (anti brute-force).
pub const DEFAULT_FAILURE_DELAY: Duration = Duration::from_millis(25);

/// Intervalo padrao de keepalive no SSE (5 segundos).
pub const DEFAULT_SSE_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);

/// Quantidade padrao de eventos SSE retidos para replay via `Last-Event-ID`.
pub const DEFAULT_SSE_REPLAY_LIMIT: usize = 200;

/// Configuracao completa do servidor Bookferry (sem variáveis globais nem caminhos fixos).
#[derive(Clone)]
pub struct Config {
    /// Diretorio onde o SQLite (`bookferry.db`) e as chaves TLS (`tls_cert.der`, `tls_key.der`) sao gravados.
    pub data_dir: PathBuf,
    /// Diretorio somente-leitura onde os livros (`.epub`, `.pdf`, `.cbz`) do usuario ficam armazenados.
    pub vault_dir: PathBuf,
    /// Endereco IP de bind do socket TCP (ex.: `127.0.0.1` nos testes ou `0.0.0.0` em producao LAN).
    pub bind_addr: IpAddr,
    /// Porta TCP (`0` escolhe porta efemera livre do sistema operacional).
    pub port: u16,
    /// Lista de IPs da LAN a anunciar no QR Code e no mDNS.
    pub announce_ips: Vec<IpAddr>,
    /// Lista de nomes de interfaces de rede a habilitar no mDNS (ex.: `["enp3s0", "wlan0"]`).
    pub announce_interfaces: Vec<String>,
    /// Habilita ou desabilita o anuncio mDNS (`_bookferry._tcp.local.`).
    pub enable_mdns: bool,
    /// Nome legivel da instancia do servidor (ex.: `"Bookferry PC"`).
    pub server_name: String,
    /// Tempo de vida do segredo de pareamento de uso unico.
    pub pairing_ttl: Duration,
    /// Numero maximo de tentativas falhas de pareamento antes do bloqueio temporario.
    pub max_failed_pair_attempts: u32,
    /// Duracao do bloqueio temporario apos exceder `max_failed_pair_attempts`.
    pub pair_lockout_duration: Duration,
    /// Atraso aplicado a cada tentativa falha de pareamento.
    pub failure_delay: Duration,
    /// Intervalo de emissao de `: keepalive` no stream SSE.
    pub sse_keepalive_interval: Duration,
    /// Quantidade maxima de eventos SSE persistidos para replay via `Last-Event-ID`.
    pub sse_replay_limit: usize,
    /// Quantidade maxima de entradas no historico de posicoes por livro.
    pub position_history_limit: usize,
    /// Deriva maxima permitida no futuro para o `wall_ms` de um HLC recebido.
    pub max_future_drift_ms: u64,
    /// Limite maximo em bytes para corpos de requisicao JSON.
    pub max_json_body_bytes: usize,
    /// Relogio injetavel (padrao: `SystemClock`).
    pub clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("data_dir", &self.data_dir)
            .field("vault_dir", &self.vault_dir)
            .field("bind_addr", &self.bind_addr)
            .field("port", &self.port)
            .field("announce_ips", &self.announce_ips)
            .field("announce_interfaces", &self.announce_interfaces)
            .field("enable_mdns", &self.enable_mdns)
            .field("server_name", &self.server_name)
            .field("pairing_ttl", &self.pairing_ttl)
            .field("max_failed_pair_attempts", &self.max_failed_pair_attempts)
            .field("pair_lockout_duration", &self.pair_lockout_duration)
            .field("sse_keepalive_interval", &self.sse_keepalive_interval)
            .field("sse_replay_limit", &self.sse_replay_limit)
            .field("position_history_limit", &self.position_history_limit)
            .field("max_future_drift_ms", &self.max_future_drift_ms)
            .field("max_json_body_bytes", &self.max_json_body_bytes)
            .finish()
    }
}

impl Config {
    /// Cria uma configuracao com valores padrao seguros para os diretorios informados.
    pub fn new(data_dir: impl Into<PathBuf>, vault_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            vault_dir: vault_dir.into(),
            bind_addr: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            announce_ips: Vec::new(),
            announce_interfaces: Vec::new(),
            enable_mdns: false,
            server_name: "bookferry-pc".to_string(),
            pairing_ttl: DEFAULT_PAIRING_TTL,
            max_failed_pair_attempts: DEFAULT_MAX_FAILED_PAIR_ATTEMPTS,
            pair_lockout_duration: DEFAULT_PAIR_LOCKOUT_DURATION,
            failure_delay: DEFAULT_FAILURE_DELAY,
            sse_keepalive_interval: DEFAULT_SSE_KEEPALIVE_INTERVAL,
            sse_replay_limit: DEFAULT_SSE_REPLAY_LIMIT,
            position_history_limit: DEFAULT_POSITION_HISTORY_LIMIT,
            max_future_drift_ms: DEFAULT_MAX_FUTURE_DRIFT_MS,
            max_json_body_bytes: DEFAULT_MAX_JSON_BODY_BYTES,
            clock: Arc::new(SystemClock),
        }
    }
}

/// Handle do servidor em execucao, permitindo inspecionar enderecos, fingerprints TLS,
/// payload de pareamento e acionar o desligamento limpo (`shutdown`).
pub struct ServerHandle {
    local_addr: SocketAddr,
    cert_sha256: String,
    spki_sha256: String,
    cert_der: Vec<u8>,
    db_path: PathBuf,
    state: Arc<AppState>,
    mdns: Option<MdnsHandle>,
    shutdown_tx: watch::Sender<bool>,
    accept_task: JoinHandle<()>,
}

impl ServerHandle {
    /// Endereco `IP:porta` real em que o servidor esta escutando.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Fingerprint SHA-256 hex lowercase do certificado X.509 DER completo.
    pub fn cert_sha256(&self) -> &str {
        &self.cert_sha256
    }

    /// Fingerprint SHA-256 hex lowercase do SubjectPublicKeyInfo (SPKI) DER.
    pub fn spki_sha256(&self) -> &str {
        &self.spki_sha256
    }

    /// Bytes DER brutos do certificado X.509 do servidor.
    pub fn cert_der(&self) -> &[u8] {
        &self.cert_der
    }

    /// Caminho do arquivo SQLite dentro de `data_dir`.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Retorna o payload JSON versionado do QR Code de pareamento atual.
    pub fn pairing_payload(&self) -> PairingQrPayload {
        self.state.current_pairing_payload()
    }

    /// Gera um novo segredo de pareamento de uso unico e retorna o novo payload de QR Code.
    pub fn rotate_pairing_secret(&self) -> Result<PairingQrPayload, ServerError> {
        Ok(self.state.rotate_pairing_secret()?)
    }

    /// Desliga o servidor de forma limpa: encerra o anuncio mDNS, interrompe o loop de
    /// aceitacao TCP, fecha streams SSE abertos e aguarda o encerramento das tarefas.
    pub async fn shutdown(mut self) -> Result<(), ServerError> {
        if let Some(mdns) = self.mdns.take() {
            mdns.shutdown();
        }
        let _ = self.shutdown_tx.send(true);
        self.accept_task
            .await
            .map_err(|e| ServerError::TaskJoin(e.to_string()))?;
        Ok(())
    }
}

/// Inicia o servidor Bookferry de forma assincrona a partir de `Config`.
pub async fn start(config: Config) -> Result<ServerHandle, ServerError> {
    std::fs::create_dir_all(&config.data_dir)?;
    if !config.vault_dir.exists() {
        return Err(ServerError::InvalidConfig(format!(
            "vault_dir nao existe: {}",
            config.vault_dir.display()
        )));
    }

    let db = Arc::new(Database::open(&config.data_dir)?);
    let db_path = db.path().to_path_buf();

    let hlc_node = sanitize_hlc_node(&config.server_name);
    let hlc_clock = Arc::new(HlcClock::with_clock(hlc_node, config.clock.clone())?);
    if let Some(highest_hlc) = db.highest_stored_hlc()? {
        let _ = hlc_clock.receive(&highest_hlc, u64::MAX);
    }

    // Escaneia a biblioteca na inicializacao
    let now_ms = config.clock.now_ms();
    let initial_scan = scan_vault_dir(&config.vault_dir)?;
    db.upsert_scanned_books(&initial_scan.scanned_books, now_ms)?;

    let mut tls_ips = config.announce_ips.clone();
    if !tls_ips.contains(&config.bind_addr) && !config.bind_addr.is_unspecified() {
        tls_ips.push(config.bind_addr);
    }
    let tls_identity = load_or_generate_tls(&config.data_dir, &tls_ips, &config.server_name)?;

    let listener = TcpListener::bind(SocketAddr::new(config.bind_addr, config.port)).await?;
    let local_addr = listener.local_addr()?;

    let announce_ip_strings: Vec<String> = if config.announce_ips.is_empty() {
        vec![local_addr.ip().to_string()]
    } else {
        config
            .announce_ips
            .iter()
            .map(ToString::to_string)
            .collect()
    };

    let initial_secret = format!("bf_pair_{}", generate_random_hex(32)?);
    let pairing_ttl_ms = config.pairing_ttl.as_millis() as u64;
    let pairing_session = PairingSession::new(initial_secret, now_ms, pairing_ttl_ms);

    let (sse_tx, _) = broadcast::channel(256);
    let (revocation_tx, _) = broadcast::channel(64);
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    let state = Arc::new(AppState {
        db,
        hlc_clock,
        clock: config.clock.clone(),
        started_at_ms: now_ms,
        server_name: config.server_name.clone(),
        vault_dir: config.vault_dir.clone(),
        cert_sha256: tls_identity.cert_sha256.clone(),
        spki_sha256: tls_identity.spki_sha256.clone(),
        announce_ips: announce_ip_strings,
        bound_port: local_addr.port(),
        pairing: Mutex::new(pairing_session),
        pairing_ttl_ms,
        max_failed_pair_attempts: config.max_failed_pair_attempts.max(1),
        pair_lockout_ms: config.pair_lockout_duration.as_millis() as u64,
        failure_delay: config.failure_delay,
        sse_keepalive_interval: config.sse_keepalive_interval,
        sse_replay_limit: config.sse_replay_limit.max(1),
        position_history_limit: config.position_history_limit.max(1),
        max_future_drift_ms: config.max_future_drift_ms,
        sse_tx,
        revocation_tx,
        shutdown_rx: shutdown_rx.clone(),
    });

    let mdns = if config.enable_mdns {
        MdnsHandle::try_start(
            &config.server_name,
            local_addr.port(),
            &config.announce_ips,
            &config.announce_interfaces,
            &tls_identity.spki_sha256,
            &tls_identity.cert_sha256,
        )
    } else {
        None
    };

    let router = build_router(state.clone(), config.max_json_body_bytes);
    let acceptor = tls_identity.acceptor.clone();

    let accept_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;

                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }

                accepted = listener.accept() => {
                    let (tcp_stream, peer_addr) = match accepted {
                        Ok(pair) => pair,
                        Err(err) => {
                            tracing::warn!(error = %err, "erro ao aceitar conexao TCP");
                            continue;
                        }
                    };
                    let _ = tcp_stream.set_nodelay(true);
                    let acceptor = acceptor.clone();
                    let router = router.clone();
                    let mut conn_shutdown_rx = shutdown_rx.clone();

                    tokio::spawn(async move {
                        let tls_stream = match tokio::time::timeout(
                            Duration::from_secs(10),
                            acceptor.accept(tcp_stream),
                        )
                        .await
                        {
                            Ok(Ok(s)) => s,
                            Ok(Err(err)) => {
                                tracing::debug!(peer = %peer_addr, error = %err, "handshake TLS recusado");
                                return;
                            }
                            Err(_) => {
                                tracing::debug!(peer = %peer_addr, "timeout no handshake TLS");
                                return;
                            }
                        };

                        let io = TokioIo::new(tls_stream);
                        let svc = TowerToHyperService::new(router);
                        let builder = ConnBuilder::new(TokioExecutor::new());
                        let conn = builder.serve_connection(io, svc);
                        tokio::pin!(conn);

                        tokio::select! {
                            _ = conn.as_mut() => {}
                            _ = conn_shutdown_rx.changed() => {
                                conn.as_mut().graceful_shutdown();
                                let _ = tokio::time::timeout(Duration::from_secs(2), conn).await;
                            }
                        }
                    });
                }
            }
        }
    });

    Ok(ServerHandle {
        local_addr,
        cert_sha256: tls_identity.cert_sha256,
        spki_sha256: tls_identity.spki_sha256,
        cert_der: tls_identity.cert_der,
        db_path,
        state,
        mdns,
        shutdown_tx,
        accept_task,
    })
}

fn sanitize_hlc_node(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_graphic() && *c != ':')
        .take(64)
        .collect();
    if cleaned.is_empty() {
        "bookferry-server".to_string()
    } else {
        cleaned
    }
}

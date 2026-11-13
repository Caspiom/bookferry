#![forbid(unsafe_code)]

pub mod db;
pub mod discovery;
pub mod error;
pub mod resume;
pub mod tls;

pub use crate::db::{
    CachedBook, ClientDatabase, OutboxEntry, PairedServerInfo, ServerBookEntry, CLIENT_DB_FILENAME,
    CLIENT_SCHEMA_V1_DDL, TOKEN_SECRET_FILENAME,
};
pub use crate::discovery::{
    build_ordered_candidates, collect_qr_ips, validate_mdns_candidate, DiscoveryProvider,
    MdnsCandidate, MdnsSdDiscovery, MockDiscoveryProvider, MDNS_SERVICE_TYPE,
};
pub use crate::error::{ApiErrorBody, ClientError, HEADER_BOOKFERRY_API};
pub use crate::resume::{
    decide_resume, decide_resume_with_threshold, ResumeDecision,
    DEFAULT_RESUME_ASK_THRESHOLD_PERCENT,
};
pub use crate::tls::{
    build_pinned_tls_config, extract_spki_from_x509_der, is_valid_sha256_hex,
    PinnedServerCertVerifier, TlsPin,
};

use crate::tls::{parse_api_error, PinnedHttpClient};
use bookferry_core::{
    BookId, Clock, Hlc, PairingQrPayload, PositionRecord, SystemClock, PAIRING_QR_VERSION,
};
use http_body_util::BodyExt;
use hyper::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::SeekFrom;
use std::net::{IpAddr, SocketAddr};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::{broadcast, mpsc, watch, Mutex as AsyncMutex, Notify};
use tokio::task::JoinHandle;

/// Timeout padrao de conexao TCP + handshake TLS (2 segundos).
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Timeout padrao de requisicao HTTP (10 segundos).
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Timeout padrao de leitura do stream SSE (~25 segundos; servidor envia `: keepalive` a cada ~5 s).
pub const DEFAULT_SSE_READ_TIMEOUT: Duration = Duration::from_secs(25);

/// Backoff inicial de reconexao SSE (~0,4 segundo).
pub const DEFAULT_SSE_BACKOFF_INITIAL: Duration = Duration::from_millis(400);

/// Backoff maximo de reconexao SSE (~10 segundos).
pub const DEFAULT_SSE_BACKOFF_MAX: Duration = Duration::from_secs(10);

/// Backoff inicial de envio da fila `outbox` (~0,2 segundo).
pub const DEFAULT_OUTBOX_BACKOFF_INITIAL: Duration = Duration::from_millis(200);

/// Backoff maximo de envio da fila `outbox` (~10 segundos).
pub const DEFAULT_OUTBOX_BACKOFF_MAX: Duration = Duration::from_secs(10);

/// Timeout padrao de busca mDNS quando os IPs conhecidos falham (600 ms).
pub const DEFAULT_MDNS_TIMEOUT: Duration = Duration::from_millis(600);

/// Configuracao do cliente `bookferry-client` (sem variaveis globais nem caminhos fixos).
#[derive(Clone)]
pub struct ClientConfig {
    /// Diretorio local onde `client.db`, `token.secret` (`0600`) e `books/` sao armazenados.
    pub data_dir: PathBuf,
    /// Nome legivel deste dispositivo (ex.: `"Galaxy S23"` ou `"Bookferry Desktop"`).
    pub device_name: String,
    /// Timeout curto por candidato de IP no connect TCP + handshake TLS.
    pub connect_timeout: Duration,
    /// Timeout para requisicoes HTTP regulares.
    pub request_timeout: Duration,
    /// Timeout de leitura no stream SSE para deteccao de conexao meio-aberta (~25 s).
    pub sse_read_timeout: Duration,
    /// Backoff inicial de reconexao SSE (~0,4 s).
    pub sse_backoff_initial: Duration,
    /// Limite maximo do backoff de reconexao SSE (~10 s).
    pub sse_backoff_max: Duration,
    /// Backoff inicial de retry da `outbox`.
    pub outbox_backoff_initial: Duration,
    /// Limite maximo do backoff de retry da `outbox`.
    pub outbox_backoff_max: Duration,
    /// Tempo maximo de busca mDNS quando IPs salvos nao respondem.
    pub mdns_timeout: Duration,
    /// Habilita descoberta mDNS `_bookferry._tcp.local.`.
    pub enable_mdns: bool,
    /// Limiar em pontos percentuais para `ResumeDecision::Ask` (padrao: `2.0`).
    pub resume_ask_threshold_percent: f64,
    /// Relogio fisico injetavel (padrao: `SystemClock`).
    pub clock: Arc<dyn Clock>,
    /// Provedor de descoberta mDNS injetavel (padrao: `MdnsSdDiscovery`).
    pub discovery_provider: Arc<dyn DiscoveryProvider>,
}

impl fmt::Debug for ClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientConfig")
            .field("data_dir", &self.data_dir)
            .field("device_name", &self.device_name)
            .field("connect_timeout", &self.connect_timeout)
            .field("request_timeout", &self.request_timeout)
            .field("sse_read_timeout", &self.sse_read_timeout)
            .field("sse_backoff_initial", &self.sse_backoff_initial)
            .field("sse_backoff_max", &self.sse_backoff_max)
            .field("outbox_backoff_initial", &self.outbox_backoff_initial)
            .field("outbox_backoff_max", &self.outbox_backoff_max)
            .field("mdns_timeout", &self.mdns_timeout)
            .field("enable_mdns", &self.enable_mdns)
            .field(
                "resume_ask_threshold_percent",
                &self.resume_ask_threshold_percent,
            )
            .finish()
    }
}

impl ClientConfig {
    pub fn new(data_dir: impl Into<PathBuf>, device_name: impl Into<String>) -> Self {
        Self {
            data_dir: data_dir.into(),
            device_name: device_name.into(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            sse_read_timeout: DEFAULT_SSE_READ_TIMEOUT,
            sse_backoff_initial: DEFAULT_SSE_BACKOFF_INITIAL,
            sse_backoff_max: DEFAULT_SSE_BACKOFF_MAX,
            outbox_backoff_initial: DEFAULT_OUTBOX_BACKOFF_INITIAL,
            outbox_backoff_max: DEFAULT_OUTBOX_BACKOFF_MAX,
            mdns_timeout: DEFAULT_MDNS_TIMEOUT,
            enable_mdns: false,
            resume_ask_threshold_percent: DEFAULT_RESUME_ASK_THRESHOLD_PERCENT,
            clock: Arc::new(SystemClock),
            discovery_provider: Arc::new(MdnsSdDiscovery),
        }
    }
}

/// Origem de uma mudanca de posicao emitida em `ClientEvent::PositionChanged`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionOrigin {
    /// Posicao gravada localmente neste dispositivo (`set_position`).
    Local,
    /// Posicao recebida em tempo real via stream SSE (`GET /events`).
    RemoteSse,
    /// Posicao recebida via reconciliacao (`pull()` / `GET /positions` / `GET /books`).
    RemotePull,
    /// Posicao vencedora do servidor adotada quando um `PUT` da `outbox` retornou `applied = false`.
    ServerConflictWinner,
}

/// Eventos assincronos emitidos pelo `Client` para a camada de UI (via `broadcast::Receiver`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientEvent {
    /// Conexao com o servidor estabelecida (SSE ou sincronizacao ativa).
    Connected { addr: SocketAddr },
    /// Conexao com o servidor perdida (queda de rede, Doze, timeout ou reinicio do servidor).
    Disconnected { reason: String },
    /// Dispositivo despareado (`unpair()` ou token revogado com `401 Unauthorized`).
    Unpaired { reason: String },
    /// Posicao de leitura de um livro alterada (local, SSE, pull ou resolucao de conflito LWW).
    PositionChanged {
        book_id: String,
        position: PositionRecord,
        origin: PositionOrigin,
    },
    /// Cache local da biblioteca foi atualizado.
    LibraryChanged { total_books: usize },
    /// Progresso de download de um arquivo de livro.
    DownloadProgress {
        book_id: String,
        bytes: u64,
        total: Option<u64>,
    },
    /// Download de livro concluido e verificado com sucesso.
    DownloadCompleted {
        book_id: String,
        path: PathBuf,
        bytes: u64,
    },
}

/// Progresso detalhado enviado pelo canal opcional de `download_book`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub book_id: String,
    pub bytes: u64,
    pub total: Option<u64>,
    pub resumed_from: u64,
}

/// Relatorio detalhado de execucao de `download_book_with_options`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadReport {
    pub path: PathBuf,
    pub resumed_from_byte: u64,
    pub bytes_transferred_this_call: u64,
    pub total_bytes: u64,
    pub status_code: u16,
    pub etag: Option<String>,
}

/// Snapshot da biblioteca retornado por `Client::library()`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LibrarySnapshot {
    pub books: Vec<CachedBook>,
    /// `false` quando obtido online de `GET /books`; `true` quando servido do cache SQLite offline.
    pub stale: bool,
}

/// Resposta de `Client::rescan()` (`POST /library/rescan`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RescanSummary {
    pub scanned_books: usize,
    pub duplicates_skipped: usize,
    pub total_books: u64,
}

/// Trait para aceitar tanto `PairingQrPayload` quanto o texto JSON do QR Code em `Client::pair`.
pub trait PairingInput {
    fn into_pairing_payload(self) -> Result<PairingQrPayload, ClientError>;
}

impl PairingInput for PairingQrPayload {
    fn into_pairing_payload(self) -> Result<PairingQrPayload, ClientError> {
        Ok(self)
    }
}

impl PairingInput for &PairingQrPayload {
    fn into_pairing_payload(self) -> Result<PairingQrPayload, ClientError> {
        Ok(self.clone())
    }
}

impl PairingInput for &str {
    fn into_pairing_payload(self) -> Result<PairingQrPayload, ClientError> {
        PairingQrPayload::from_json_str(self).map_err(ClientError::Json)
    }
}

impl PairingInput for String {
    fn into_pairing_payload(self) -> Result<PairingQrPayload, ClientError> {
        PairingQrPayload::from_json_str(&self).map_err(ClientError::Json)
    }
}

impl PairingInput for &String {
    fn into_pairing_payload(self) -> Result<PairingQrPayload, ClientError> {
        PairingQrPayload::from_json_str(self).map_err(ClientError::Json)
    }
}

#[derive(Serialize)]
struct PairRequestDto<'a> {
    secret: &'a str,
    device_name: &'a str,
}

impl fmt::Debug for PairRequestDto<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairRequestDto")
            .field("secret", &"[REDACTED]")
            .field("device_name", &self.device_name)
            .finish()
    }
}

#[derive(Deserialize)]
struct PairResponseDto {
    device_id: String,
    device_name: String,
    token: String,
    cert_sha256: String,
    spki_sha256: String,
}

impl fmt::Debug for PairResponseDto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairResponseDto")
            .field("device_id", &self.device_id)
            .field("device_name", &self.device_name)
            .field("token", &"[REDACTED]")
            .field("cert_sha256", &self.cert_sha256)
            .field("spki_sha256", &self.spki_sha256)
            .finish()
    }
}

#[derive(Deserialize)]
struct BooksListResponseDto {
    books: Vec<ServerBookEntry>,
}

#[derive(Deserialize)]
struct PositionsListResponseDto {
    positions: Vec<PositionRecord>,
}

#[derive(Deserialize)]
struct PutPositionResponseDto {
    applied: bool,
    position: PositionRecord,
    #[allow(dead_code)]
    server_seq: u64,
    server_hlc: String,
    #[allow(dead_code)]
    history_count: usize,
}

#[derive(Deserialize)]
struct GetPositionResponseDto {
    position: PositionRecord,
}

#[derive(Deserialize)]
struct SsePositionPayloadDto {
    seq: u64,
    #[allow(dead_code)]
    book_id: String,
    origin_device_id: String,
    position: PositionRecord,
}

struct Inner {
    config: ClientConfig,
    db: Arc<ClientDatabase>,
    events_tx: broadcast::Sender<ClientEvent>,
    outbox_notify: Notify,
    reconnect_notify: Notify,
    flush_lock: AsyncMutex<()>,
    connected_flag: AtomicBool,
    offline_flag: AtomicBool,
    offline_tx: watch::Sender<bool>,
    shutdown_tx: watch::Sender<bool>,
}

/// Cliente principal de sincronizacao do Bookferry.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
    tasks: Arc<AsyncMutex<Vec<JoinHandle<()>>>>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("config", &self.inner.config)
            .field(
                "connected",
                &self.inner.connected_flag.load(AtomicOrdering::SeqCst),
            )
            .finish()
    }
}

impl Client {
    /// Abre (ou cria) o banco local em `config.data_dir` e inicia os lacos de sincronizacao
    /// em background (`outbox` e `SSE`).
    pub async fn open(config: ClientConfig) -> Result<Self, ClientError> {
        std::fs::create_dir_all(&config.data_dir)?;
        let books_dir = config.data_dir.join("books");
        std::fs::create_dir_all(&books_dir)?;

        let db = Arc::new(ClientDatabase::open(&config.data_dir, &config.device_name)?);
        let (events_tx, _) = broadcast::channel(512);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (offline_tx, _) = watch::channel(false);

        let inner = Arc::new(Inner {
            config,
            db,
            events_tx,
            outbox_notify: Notify::new(),
            reconnect_notify: Notify::new(),
            flush_lock: AsyncMutex::new(()),
            connected_flag: AtomicBool::new(false),
            offline_flag: AtomicBool::new(false),
            offline_tx,
            shutdown_tx,
        });

        let outbox_task = spawn_outbox_loop(inner.clone(), shutdown_rx.clone());
        let sse_task = spawn_sse_loop(inner.clone(), shutdown_rx);

        let client = Self {
            inner,
            tasks: Arc::new(AsyncMutex::new(vec![outbox_task, sse_task])),
        };

        // Se ja estiver pareado ao abrir, aciona imediatamente a sincronizacao
        if client.is_paired()? {
            client.inner.outbox_notify.notify_one();
            client.inner.reconnect_notify.notify_one();
        }

        Ok(client)
    }

    /// Inscreve um receptor para receber `ClientEvent`s em tempo real.
    pub fn subscribe(&self) -> broadcast::Receiver<ClientEvent> {
        self.inner.events_tx.subscribe()
    }

    /// Caminho do arquivo SQLite local (`client.db`).
    pub fn db_path(&self) -> &Path {
        self.inner.db.db_path()
    }

    /// Caminho do arquivo de token (`token.secret`, permissao `0600`).
    pub fn token_path(&self) -> &Path {
        self.inner.db.token_path()
    }

    /// Retorna `true` se houver um servidor pareado ativo.
    pub fn is_paired(&self) -> Result<bool, ClientError> {
        Ok(self.inner.db.load_pairing()?.is_some())
    }

    /// Retorna os metadados do servidor pareado (sem expor o token).
    pub fn paired_info(&self) -> Result<Option<PairedServerInfo>, ClientError> {
        Ok(self.inner.db.load_pairing()?.map(|s| s.info))
    }

    /// Retorna o `device_id` atribuido pelo servidor no pareamento, se pareado.
    pub fn device_id(&self) -> Result<Option<String>, ClientError> {
        Ok(self.inner.db.load_pairing()?.map(|s| s.info.device_id))
    }

    /// Pareia o cliente com o servidor a partir de um `PairingQrPayload` ou string JSON do QR Code.
    ///
    /// Suporta apenas um servidor por instalacao (um novo pareamento substitui o anterior).
    /// Valida estritamente o certificado TLS pelo PIN (`cert_sha256` e/ou `spki_sha256`) do payload
    /// sem usar CAs do sistema, troca o segredo unico por um token de dispositivo e persiste
    /// o token com permissao `0600` em `data_dir`.
    pub async fn pair(
        &self,
        payload: impl PairingInput,
        device_name: &str,
    ) -> Result<PairedServerInfo, ClientError> {
        let qr = payload.into_pairing_payload()?;
        if qr.v != PAIRING_QR_VERSION {
            return Err(ClientError::InvalidInput(format!(
                "versao de QR Code nao suportada: {}",
                qr.v
            )));
        }

        let effective_device_name = if device_name.trim().is_empty() {
            self.inner.config.device_name.trim()
        } else {
            device_name.trim()
        };
        if effective_device_name.is_empty() {
            return Err(ClientError::InvalidInput(
                "device_name nao pode ser vazio".to_string(),
            ));
        }

        let pin = TlsPin::new(Some(qr.cert_sha256.clone()), Some(qr.spki_sha256.clone()))?;
        let http = PinnedHttpClient::new(
            pin.clone(),
            self.inner.config.connect_timeout,
            self.inner.config.request_timeout,
        )?;

        let known_ips = collect_qr_ips(&qr.ips, qr.ip.as_deref());
        let mut candidate_addrs: Vec<SocketAddr> = known_ips
            .iter()
            .filter_map(|ip_str| ip_str.parse::<IpAddr>().ok())
            .map(|ip| SocketAddr::new(ip, qr.port))
            .collect();

        if candidate_addrs.is_empty() && self.inner.config.enable_mdns {
            let disc = self.inner.config.discovery_provider.clone();
            let timeout = self.inner.config.mdns_timeout;
            let mdns_cands = tokio::task::spawn_blocking(move || disc.discover(timeout))
                .await
                .unwrap_or_default();
            candidate_addrs = build_ordered_candidates(None, &[], qr.port, &mdns_cands, &pin);
        }

        if candidate_addrs.is_empty() {
            return Err(ClientError::ConnectionFailed(
                "nenhum endereco IP valido no payload de pareamento".to_string(),
            ));
        }

        let req_body = serde_json::to_vec(&PairRequestDto {
            secret: &qr.secret,
            device_name: effective_device_name,
        })?;

        let mut last_err: Option<ClientError> = None;
        for addr in candidate_addrs {
            match http
                .request_bytes(addr, Method::POST, "/pair", None, &[], req_body.clone())
                .await
            {
                Ok((status, _, bytes)) => {
                    if !status.is_success() {
                        // Erro HTTP explicito do servidor (401 segredo errado, 403 reuso, 410 expirado, 429)
                        return Err(parse_api_error(status, &bytes));
                    }
                    let pair_resp: PairResponseDto =
                        serde_json::from_slice(&bytes).map_err(ClientError::Json)?;

                    let info = PairedServerInfo {
                        device_id: pair_resp.device_id,
                        device_name: pair_resp.device_name,
                        server_name: qr.name.clone(),
                        port: addr.port(),
                        last_good_ip: Some(addr.ip().to_string()),
                        known_ips,
                        cert_sha256: pair_resp.cert_sha256,
                        spki_sha256: pair_resp.spki_sha256,
                        paired_at_ms: self.inner.config.clock.now_ms(),
                    };

                    self.inner.db.save_pairing(&info, &pair_resp.token)?;
                    tracing::info!(
                        device_id = %info.device_id,
                        server_name = %info.server_name,
                        addr = %addr,
                        "pareamento concluido com sucesso"
                    );

                    self.inner.outbox_notify.notify_one();
                    self.inner.reconnect_notify.notify_one();

                    return Ok(info);
                }
                Err(err) => {
                    // Preserva erros especificos de TLS/Pinning sobre erros genericos de TCP
                    let replace = match (&last_err, &err) {
                        (None, _) => true,
                        (Some(ClientError::TlsPinMismatch(_)), _) => false,
                        (_, ClientError::TlsPinMismatch(_)) => true,
                        (Some(ClientError::CleartextOrInvalidTls(_)), _) => false,
                        (_, ClientError::CleartextOrInvalidTls(_)) => true,
                        _ => true,
                    };
                    if replace {
                        last_err = Some(err);
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            ClientError::ConnectionFailed("nao foi possivel alcancar o servidor".to_string())
        }))
    }

    /// Desfaz o pareamento: se o servidor estiver alcancavel, chama `DELETE /devices/{id}`,
    /// apaga todos os dados e tokens locais e emite `ClientEvent::Unpaired`.
    pub async fn unpair(&self) -> Result<(), ClientError> {
        if let Some(stored) = self.inner.db.load_pairing()? {
            let uri = format!("/devices/{}", stored.info.device_id);
            let _ = self
                .inner
                .try_request_bytes(&stored, Method::DELETE, &uri, &[], Vec::new())
                .await;
        }

        self.inner.db.wipe_all_on_unpair()?;
        self.inner
            .connected_flag
            .store(false, AtomicOrdering::SeqCst);
        let _ = self.inner.events_tx.send(ClientEvent::Unpaired {
            reason: "unpaired".to_string(),
        });
        self.inner.reconnect_notify.notify_one();
        Ok(())
    }

    /// Retorna a biblioteca (`GET /books`), atualizando o cache SQLite local quando online
    /// (`stale = false`) ou retornando o cache local com `stale = true` quando offline.
    pub async fn library(&self) -> Result<LibrarySnapshot, ClientError> {
        let Some(stored) = self.inner.db.load_pairing()? else {
            return Err(ClientError::NotPaired);
        };

        match self
            .inner
            .try_request_json::<BooksListResponseDto>(&stored, Method::GET, "/books", Vec::new())
            .await
        {
            Ok(resp) => {
                let raw_now_ms = self.inner.config.clock.now_ms();
                let (books, lib_changed, applied_positions) = self
                    .inner
                    .db
                    .store_library_snapshot(&resp.books, raw_now_ms)?;

                for pos in applied_positions {
                    let _ = self.inner.events_tx.send(ClientEvent::PositionChanged {
                        book_id: pos.book_id.clone(),
                        position: pos,
                        origin: PositionOrigin::RemotePull,
                    });
                }
                if lib_changed {
                    let _ = self.inner.events_tx.send(ClientEvent::LibraryChanged {
                        total_books: books.len(),
                    });
                }

                Ok(LibrarySnapshot {
                    books,
                    stale: false,
                })
            }
            Err(err) if err.is_unauthorized() => Err(err),
            Err(err) if err.is_transient() => {
                tracing::debug!(error = %err, "servidor inacessivel em library(); retornando cache local (stale=true)");
                let cached = self.inner.db.load_cached_library()?;
                Ok(LibrarySnapshot {
                    books: cached,
                    stale: true,
                })
            }
            Err(err) => Err(err),
        }
    }

    /// Solicita um re-escaneamento do cofre de livros no servidor (`POST /library/rescan`)
    /// e atualiza o cache local da biblioteca.
    pub async fn rescan(&self) -> Result<RescanSummary, ClientError> {
        let Some(stored) = self.inner.db.load_pairing()? else {
            return Err(ClientError::NotPaired);
        };

        let summary: RescanSummary = self
            .inner
            .try_request_json(&stored, Method::POST, "/library/rescan", Vec::new())
            .await?;

        let _ = self.library().await;
        Ok(summary)
    }

    /// Retorna o caminho local seguro (`data_dir/books/<book_id>`) se o livro ja estiver
    /// baixado no disco, ou `Ok(None)` caso contrario. Rejeita `book_id` malicioso.
    pub fn local_path(&self, book_id: &str) -> Result<Option<PathBuf>, ClientError> {
        let parsed = BookId::parse(book_id)?;
        let (final_path, _) = self.inner.safe_book_paths(&parsed)?;
        if final_path.is_file() {
            Ok(Some(final_path))
        } else {
            Ok(None)
        }
    }

    /// Remove o arquivo baixado (e eventual `.part`) de `data_dir/books/<book_id>`.
    pub async fn remove_download(&self, book_id: &str) -> Result<bool, ClientError> {
        let parsed = BookId::parse(book_id)?;
        let (final_path, part_path) = self.inner.safe_book_paths(&parsed)?;
        let mut removed = false;
        if final_path.exists() {
            tokio::fs::remove_file(&final_path).await?;
            removed = true;
        }
        if part_path.exists() {
            tokio::fs::remove_file(&part_path).await?;
            removed = true;
        }
        let _ = self.inner.db.set_download_etag(parsed.as_str(), None);
        Ok(removed)
    }

    /// Baixa um livro (`GET /books/{id}/file`) para `data_dir/books/<book_id>` escrevendo em
    /// `<book_id>.part` com retomada automatica via HTTP `Range` e verificacao de integridade.
    pub async fn download_book(
        &self,
        book_id: &str,
        progress: Option<mpsc::UnboundedSender<DownloadProgress>>,
    ) -> Result<PathBuf, ClientError> {
        let report = self
            .download_book_with_options(book_id, progress, None, None)
            .await?;
        Ok(report.path)
    }

    /// Versao completa de download com suporte a cancelamento (`cancel_rx`) e limite opcional
    /// de bytes por chamada (`max_bytes_this_call`, util para testar interrupcao e retomada Range).
    pub async fn download_book_with_options(
        &self,
        book_id: &str,
        progress: Option<mpsc::UnboundedSender<DownloadProgress>>,
        mut cancel_rx: Option<watch::Receiver<bool>>,
        max_bytes_this_call: Option<u64>,
    ) -> Result<DownloadReport, ClientError> {
        let parsed_id = BookId::parse(book_id)?;
        let (expected_md5, expected_size) = split_book_id_parts(&parsed_id)?;
        let (final_path, part_path) = self.inner.safe_book_paths(&parsed_id)?;

        let Some(stored) = self.inner.db.load_pairing()? else {
            return Err(ClientError::NotPaired);
        };

        if let Some(ref rx) = cancel_rx {
            if *rx.borrow() {
                return Err(ClientError::Cancelled);
            }
        }

        // Inspeciona eventual arquivo `.part` deixado por tentativa anterior interrompida
        let mut resume_offset: u64 = match tokio::fs::metadata(&part_path).await {
            Ok(meta) => meta.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => return Err(ClientError::Io(e)),
        };

        if resume_offset > expected_size {
            let _ = tokio::fs::remove_file(&part_path).await;
            resume_offset = 0;
        }

        let uri = format!("/books/{}/file", parsed_id.as_str());
        let range_hdr_val = format!("bytes={resume_offset}-");
        let extra_headers: Vec<(&str, &str)> = if resume_offset > 0 {
            vec![("range", range_hdr_val.as_str())]
        } else {
            Vec::new()
        };

        let (_, mut resp) = self
            .inner
            .try_open_stream(&stored, Method::GET, &uri, &extra_headers, Vec::new())
            .await?;

        let mut status = resp.status();
        if status == StatusCode::RANGE_NOT_SATISFIABLE && resume_offset > 0 {
            let _ = tokio::fs::remove_file(&part_path).await;
            resume_offset = 0;
            let (_, retry_resp) = self
                .inner
                .try_open_stream(&stored, Method::GET, &uri, &[], Vec::new())
                .await?;
            resp = retry_resp;
            status = resp.status();
        }

        if !status.is_success() {
            let bytes = resp
                .into_body()
                .collect()
                .await
                .map(|c| c.to_bytes().to_vec())
                .unwrap_or_default();
            return Err(parse_api_error(status, &bytes));
        }

        let response_etag = resp
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(ToString::to_string);

        // Se estavamos retomando (206 Partial Content), verifica se o ETag mudou no servidor
        if resume_offset > 0 {
            if status == StatusCode::PARTIAL_CONTENT {
                if let (Some(saved_etag), Some(curr_etag)) = (
                    self.inner.db.get_download_etag(parsed_id.as_str())?,
                    response_etag.as_deref(),
                ) {
                    if saved_etag != curr_etag {
                        // O arquivo mudou no servidor; descarta `.part` e reinicia do zero
                        let _ = tokio::fs::remove_file(&part_path).await;
                        resume_offset = 0;
                        let (_, full_resp) = self
                            .inner
                            .try_open_stream(&stored, Method::GET, &uri, &[], Vec::new())
                            .await?;
                        resp = full_resp;
                        status = resp.status();
                    }
                }
            } else if status == StatusCode::OK {
                // Servidor respondeu 200 OK completo em vez de 206; regrava do zero
                resume_offset = 0;
            }
        }

        if let Some(ref etag) = response_etag {
            self.inner
                .db
                .set_download_etag(parsed_id.as_str(), Some(etag))?;
        }

        let mut file = if resume_offset > 0 {
            let mut f = tokio::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(false)
                .open(&part_path)
                .await?;
            f.seek(SeekFrom::Start(resume_offset)).await?;
            f
        } else {
            tokio::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&part_path)
                .await?
        };

        let mut current_bytes = resume_offset;
        let mut transferred_this_call = 0u64;
        let mut body = resp.into_body();

        loop {
            if let Some(ref rx) = cancel_rx {
                if *rx.borrow() {
                    file.flush().await?;
                    return Err(ClientError::Cancelled);
                }
            }

            let next_frame = if let Some(ref mut rx) = cancel_rx {
                tokio::select! {
                    biased;
                    changed = rx.changed() => {
                        if changed.is_ok() && *rx.borrow() {
                            file.flush().await?;
                            return Err(ClientError::Cancelled);
                        }
                        continue;
                    }
                    f = tokio::time::timeout(self.inner.config.request_timeout, body.frame()) => f,
                }
            } else {
                tokio::time::timeout(self.inner.config.request_timeout, body.frame()).await
            };

            let frame = match next_frame {
                Ok(Some(Ok(frame))) => frame,
                Ok(Some(Err(e))) => {
                    file.flush().await?;
                    return Err(ClientError::Http(format!(
                        "stream de download interrompido apos {current_bytes} bytes: {e}"
                    )));
                }
                Ok(None) => break,
                Err(_) => {
                    file.flush().await?;
                    return Err(ClientError::Timeout(format!(
                        "download de {}",
                        parsed_id.as_str()
                    )));
                }
            };

            if let Ok(data) = frame.into_data() {
                if data.is_empty() {
                    continue;
                }

                if let Some(limit) = max_bytes_this_call {
                    let remaining_allowed = limit.saturating_sub(transferred_this_call);
                    if data.len() as u64 >= remaining_allowed {
                        let slice = &data[..remaining_allowed as usize];
                        if !slice.is_empty() {
                            file.write_all(slice).await?;
                            current_bytes += slice.len() as u64;
                        }
                        file.flush().await?;
                        file.sync_all().await?;
                        return Err(ClientError::DownloadInterrupted {
                            downloaded_bytes: current_bytes,
                            expected_bytes: expected_size,
                        });
                    }
                }

                file.write_all(&data).await?;
                let chunk_len = data.len() as u64;
                current_bytes += chunk_len;
                transferred_this_call += chunk_len;

                let _ = self.inner.events_tx.send(ClientEvent::DownloadProgress {
                    book_id: parsed_id.as_str().to_string(),
                    bytes: current_bytes,
                    total: Some(expected_size),
                });
                if let Some(ref tx) = progress {
                    let _ = tx.send(DownloadProgress {
                        book_id: parsed_id.as_str().to_string(),
                        bytes: current_bytes,
                        total: Some(expected_size),
                        resumed_from: resume_offset,
                    });
                }
            }
        }

        file.flush().await?;
        file.sync_all().await?;
        drop(file);

        if current_bytes != expected_size {
            return Err(ClientError::DownloadInterrupted {
                downloaded_bytes: current_bytes,
                expected_bytes: expected_size,
            });
        }

        // Verifica tamanho e Partial MD5 estilo KOReader no arquivo `.part` antes de renomear
        let part_clone = part_path.clone();
        let (actual_id, actual_md5, actual_size) =
            tokio::task::spawn_blocking(move || BookId::from_file(&part_clone))
                .await
                .map_err(|e| ClientError::Io(std::io::Error::other(e.to_string())))??;

        if actual_size != expected_size || actual_md5 != expected_md5 || actual_id != parsed_id {
            let _ = tokio::fs::remove_file(&part_path).await;
            return Err(ClientError::IntegrityMismatch(format!(
                "esperado id={parsed_id} (size={expected_size}), obtido id={actual_id} (size={actual_size})"
            )));
        }

        if let Some(ref etag) = response_etag {
            let clean_etag = etag.trim_matches('"');
            if !clean_etag.starts_with(parsed_id.as_str()) {
                let _ = tokio::fs::remove_file(&part_path).await;
                return Err(ClientError::IntegrityMismatch(format!(
                    "ETag ({etag}) incompativel com book_id ({parsed_id})"
                )));
            }
        }

        tokio::fs::rename(&part_path, &final_path).await?;
        self.inner.db.set_download_etag(parsed_id.as_str(), None)?;

        let _ = self.inner.events_tx.send(ClientEvent::DownloadCompleted {
            book_id: parsed_id.as_str().to_string(),
            path: final_path.clone(),
            bytes: expected_size,
        });

        Ok(DownloadReport {
            path: final_path,
            resumed_from_byte: resume_offset,
            bytes_transferred_this_call: transferred_this_call,
            total_bytes: expected_size,
            status_code: status.as_u16(),
            etag: response_etag,
        })
    }

    /// Grava uma nova posicao de leitura LOCALMENTE na hora (offline-first, nao bloqueia na rede),
    /// enfileira na `outbox` persistente e aciona o laco de sincronizacao em background.
    pub async fn set_position(
        &self,
        book_id: &str,
        locator: &str,
        snippet: Option<String>,
        percent: f64,
    ) -> Result<PositionRecord, ClientError> {
        let parsed_id = BookId::parse(book_id)?;
        let raw_now_ms = self.inner.config.clock.now_ms();
        let record = self
            .inner
            .db
            .set_local_position(&parsed_id, locator, snippet, percent, raw_now_ms)?;

        let _ = self.inner.events_tx.send(ClientEvent::PositionChanged {
            book_id: record.book_id.clone(),
            position: record.clone(),
            origin: PositionOrigin::Local,
        });

        self.inner.outbox_notify.notify_one();
        Ok(record)
    }

    /// Retorna a melhor posicao conhecida para `book_id` (local vs servidor por HLC).
    pub async fn get_position(&self, book_id: &str) -> Result<Option<PositionRecord>, ClientError> {
        let parsed_id = BookId::parse(book_id)?;
        self.inner.db.get_position(&parsed_id)
    }

    /// Quantidade de livros com posicao pendente de envio na `outbox` local.
    pub fn outbox_len(&self) -> Result<usize, ClientError> {
        self.inner.db.outbox_len()
    }

    /// Envia imediatamente todos os itens pendentes na `outbox` para o servidor.
    /// Retorna a quantidade de itens processados com sucesso.
    pub async fn flush_outbox(&self) -> Result<usize, ClientError> {
        self.inner.flush_outbox_once().await
    }

    /// Executa `GET /positions?since=<ultimo_hlc_visto>` e mescla as posicoes no banco local por HLC.
    pub async fn pull(&self) -> Result<Vec<PositionRecord>, ClientError> {
        self.inner.pull_once().await
    }

    /// Chamado pela UI ao retomar o app de background (ex.: `onResume` no Android apos Doze).
    /// Acorda a reconexao SSE, drena a `outbox` se houver itens pendentes e executa `pull()`.
    pub async fn on_resume(&self) -> Result<Vec<PositionRecord>, ClientError> {
        self.inner.reconnect_notify.notify_one();
        self.inner.outbox_notify.notify_one();
        let _ = self.inner.flush_outbox_once().await;
        self.inner.pull_once().await
    }

    /// Avalia a decisao de retomada (`ResumeDecision`) na abertura de `book_id` comparando
    /// a ultima posicao local deste dispositivo e a ultima posicao remota de outro dispositivo.
    pub fn decide_book_resume(&self, book_id: &str) -> Result<ResumeDecision, ClientError> {
        let parsed_id = BookId::parse(book_id)?;
        let (local, remote) = self.inner.db.get_local_and_remote_positions(&parsed_id)?;
        let now_ms = self.inner.config.clock.now_ms();
        Ok(decide_resume_with_threshold(
            local.as_ref(),
            remote.as_ref(),
            now_ms,
            self.inner.config.resume_ask_threshold_percent,
        ))
    }

    /// Informa ao cliente que a conectividade de rede do sistema operacional caiu (`offline = true`)
    /// ou voltou (`offline = false`). Quando `offline = true`, interrompe imediatamente streams SSE
    /// ativos e opera apenas sobre o banco SQLite local; quando `offline = false`, aciona a
    /// reconexao SSE e o envio da `outbox`.
    pub fn set_offline(&self, offline: bool) {
        self.inner
            .offline_flag
            .store(offline, AtomicOrdering::SeqCst);
        let _ = self.inner.offline_tx.send(offline);
        if !offline {
            self.inner.outbox_notify.notify_one();
            self.inner.reconnect_notify.notify_one();
        }
    }

    /// Encerra de forma limpa as tarefas de background (`SSE` e `outbox`) do cliente.
    pub async fn shutdown(&self) -> Result<(), ClientError> {
        let _ = self.inner.shutdown_tx.send(true);
        self.inner.outbox_notify.notify_waiters();
        self.inner.reconnect_notify.notify_waiters();
        let mut guard = self.tasks.lock().await;
        while let Some(handle) = guard.pop() {
            let _ = handle.await;
        }
        Ok(())
    }
}

impl Inner {
    fn safe_book_paths(&self, book_id: &BookId) -> Result<(PathBuf, PathBuf), ClientError> {
        let books_dir = self.config.data_dir.join("books");
        std::fs::create_dir_all(&books_dir)?;

        let rel = Path::new(book_id.as_str());
        let mut comps = rel.components();
        match (comps.next(), comps.next()) {
            (Some(Component::Normal(_)), None) => {}
            _ => {
                return Err(ClientError::Core(bookferry_core::CoreError::InvalidBookId(
                    book_id.as_str().to_string(),
                )));
            }
        }

        let final_path = books_dir.join(book_id.as_str());
        let part_path = books_dir.join(format!("{}.part", book_id.as_str()));
        Ok((final_path, part_path))
    }

    fn handle_revocation(&self, reason: &str) {
        let _ = self.db.clear_auth_on_revocation();
        self.connected_flag.store(false, AtomicOrdering::SeqCst);
        let _ = self.events_tx.send(ClientEvent::Unpaired {
            reason: reason.to_string(),
        });
    }

    /// Resolve os candidatos de endereco do servidor (ultimo IP bom -> demais IPs do QR -> mDNS
    /// validado pelo TXT `fp`) e executa a requisicao HTTP. Atualiza `last_good_ip` quando mudar.
    async fn try_open_stream(
        &self,
        stored: &crate::db::StoredPairing,
        method: Method,
        uri: &str,
        extra_headers: &[(&str, &str)],
        body_bytes: Vec<u8>,
    ) -> Result<(SocketAddr, hyper::Response<hyper::body::Incoming>), ClientError> {
        if self.offline_flag.load(AtomicOrdering::SeqCst) {
            return Err(ClientError::ConnectionFailed(
                "cliente em modo offline".to_string(),
            ));
        }
        let pin = stored.info.tls_pin()?;
        let http = PinnedHttpClient::new(
            pin.clone(),
            self.config.connect_timeout,
            self.config.request_timeout,
        )?;

        // Fase 1: tenta ultimo IP bom + demais IPs do payload de pareamento
        let primary_addrs = build_ordered_candidates(
            stored.info.last_good_ip.as_deref(),
            &stored.info.known_ips,
            stored.info.port,
            &[],
            &pin,
        );

        let mut last_err: Option<ClientError> = None;
        for addr in &primary_addrs {
            match http
                .send_streaming_request(
                    *addr,
                    method.clone(),
                    uri,
                    Some(&stored.token),
                    extra_headers,
                    body_bytes.clone(),
                )
                .await
            {
                Ok(resp) => {
                    if self.offline_flag.load(AtomicOrdering::SeqCst) {
                        return Err(ClientError::ConnectionFailed(
                            "cliente em modo offline".to_string(),
                        ));
                    }
                    let _ = self
                        .db
                        .update_last_good_endpoint(&addr.ip().to_string(), addr.port());
                    if resp.status() == StatusCode::UNAUTHORIZED {
                        self.handle_revocation("token_revoked");
                        return Err(ClientError::Unauthorized);
                    }
                    return Ok((*addr, resp));
                }
                Err(err) => {
                    last_err = Some(err);
                }
            }
        }

        // Fase 2: se todos os IPs salvos falharem e mDNS estiver habilitado, descobre via mDNS
        if self.config.enable_mdns {
            let disc = self.config.discovery_provider.clone();
            let timeout = self.config.mdns_timeout;
            let mdns_cands = tokio::task::spawn_blocking(move || disc.discover(timeout))
                .await
                .unwrap_or_default();

            let all_addrs = build_ordered_candidates(
                stored.info.last_good_ip.as_deref(),
                &stored.info.known_ips,
                stored.info.port,
                &mdns_cands,
                &pin,
            );

            for addr in all_addrs {
                if primary_addrs.contains(&addr) {
                    continue;
                }
                match http
                    .send_streaming_request(
                        addr,
                        method.clone(),
                        uri,
                        Some(&stored.token),
                        extra_headers,
                        body_bytes.clone(),
                    )
                    .await
                {
                    Ok(resp) => {
                        let _ = self
                            .db
                            .update_last_good_endpoint(&addr.ip().to_string(), addr.port());
                        if resp.status() == StatusCode::UNAUTHORIZED {
                            self.handle_revocation("token_revoked");
                            return Err(ClientError::Unauthorized);
                        }
                        return Ok((addr, resp));
                    }
                    Err(err) => {
                        last_err = Some(err);
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            ClientError::ConnectionFailed("nenhum candidato de servidor alcancavel".to_string())
        }))
    }

    async fn try_request_bytes(
        &self,
        stored: &crate::db::StoredPairing,
        method: Method,
        uri: &str,
        extra_headers: &[(&str, &str)],
        body_bytes: Vec<u8>,
    ) -> Result<(StatusCode, Vec<u8>), ClientError> {
        let (_, resp) = self
            .try_open_stream(stored, method, uri, extra_headers, body_bytes)
            .await?;
        let status = resp.status();
        let bytes = tokio::time::timeout(self.config.request_timeout, resp.into_body().collect())
            .await
            .map_err(|_| ClientError::Timeout(format!("lendo resposta de {uri}")))?
            .map_err(|e| ClientError::Http(format!("erro ao ler corpo de {uri}: {e}")))?
            .to_bytes()
            .to_vec();
        Ok((status, bytes))
    }

    async fn try_request_json<T: serde::de::DeserializeOwned>(
        &self,
        stored: &crate::db::StoredPairing,
        method: Method,
        uri: &str,
        body_bytes: Vec<u8>,
    ) -> Result<T, ClientError> {
        let (status, bytes) = self
            .try_request_bytes(stored, method, uri, &[], body_bytes)
            .await?;
        if !status.is_success() {
            let err = parse_api_error(status, &bytes);
            if err.is_unauthorized() {
                self.handle_revocation("token_revoked");
            }
            return Err(err);
        }
        serde_json::from_slice(&bytes).map_err(ClientError::Json)
    }

    async fn flush_outbox_once(&self) -> Result<usize, ClientError> {
        let _guard = self.flush_lock.lock().await;
        let Some(stored) = self.db.load_pairing()? else {
            return Err(ClientError::NotPaired);
        };

        let entries = self.db.list_outbox()?;
        if entries.is_empty() {
            return Ok(0);
        }

        let mut sent_count = 0usize;
        for entry in entries {
            let uri = format!("/books/{}/position", entry.book_id);
            let body = serde_json::to_vec(&entry.to_position_update())?;

            match self
                .try_request_json::<PutPositionResponseDto>(&stored, Method::PUT, &uri, body)
                .await
            {
                Ok(resp) => {
                    let raw_now_ms = self.config.clock.now_ms();
                    let conflict_winner = self.db.complete_outbox_put(
                        &entry.book_id,
                        &entry.hlc,
                        resp.applied,
                        &resp.position,
                        &resp.server_hlc,
                        raw_now_ms,
                    )?;

                    if let Some(winner) = conflict_winner {
                        let _ = self.events_tx.send(ClientEvent::PositionChanged {
                            book_id: winner.book_id.clone(),
                            position: winner,
                            origin: PositionOrigin::ServerConflictWinner,
                        });
                    }
                    sent_count += 1;
                }
                Err(err) if err.is_unauthorized() => {
                    return Err(err);
                }
                Err(err) if err.is_hlc_too_far_in_future() => {
                    // 422 hlc_too_far_in_future: extrai now_ms do servidor, ajusta o HLC e reenvia!
                    let server_now_ms = extract_server_now_ms_from_err(&err)
                        .unwrap_or_else(|| self.config.clock.now_ms());
                    let current_srv_pos = self
                        .try_request_json::<GetPositionResponseDto>(
                            &stored,
                            Method::GET,
                            &uri,
                            Vec::new(),
                        )
                        .await
                        .ok()
                        .map(|r| r.position);

                    let raw_now_ms = self.config.clock.now_ms();
                    if let Some(adjusted) = self.db.adjust_hlc_on_future_rejection(
                        &entry.book_id,
                        server_now_ms,
                        current_srv_pos.as_ref(),
                        raw_now_ms,
                    )? {
                        let retry_body = serde_json::to_vec(&adjusted.to_position_update())?;
                        let resp: PutPositionResponseDto = self
                            .try_request_json(&stored, Method::PUT, &uri, retry_body)
                            .await?;
                        let conflict_winner = self.db.complete_outbox_put(
                            &adjusted.book_id,
                            &adjusted.hlc,
                            resp.applied,
                            &resp.position,
                            &resp.server_hlc,
                            raw_now_ms,
                        )?;
                        if let Some(winner) = conflict_winner {
                            let _ = self.events_tx.send(ClientEvent::PositionChanged {
                                book_id: winner.book_id.clone(),
                                position: winner,
                                origin: PositionOrigin::ServerConflictWinner,
                            });
                        }
                        sent_count += 1;
                    }
                }
                Err(err) if err.is_transient() => {
                    let _ = self.db.record_outbox_retry_error(
                        &entry.book_id,
                        &entry.hlc,
                        &err.to_string(),
                    );
                    return Err(err);
                }
                Err(err) => {
                    // Erro 4xx permanente neste livro (ex.: 404 book_not_found): remove da outbox
                    // para nao bloquear os demais livros
                    tracing::warn!(
                        book_id = %entry.book_id,
                        error = %err,
                        "removendo item invalido permanente da outbox"
                    );
                    let _ = self
                        .db
                        .remove_outbox_if_matching(&entry.book_id, &entry.hlc);
                }
            }
        }

        Ok(sent_count)
    }

    async fn pull_once(&self) -> Result<Vec<PositionRecord>, ClientError> {
        let Some(stored) = self.db.load_pairing()? else {
            return Err(ClientError::NotPaired);
        };

        let uri = match self.db.last_pull_hlc()? {
            Some(since) if Hlc::parse(&since).is_ok() => format!("/positions?since={since}"),
            _ => "/positions".to_string(),
        };

        let resp: PositionsListResponseDto = self
            .try_request_json(&stored, Method::GET, &uri, Vec::new())
            .await?;

        let raw_now_ms = self.config.clock.now_ms();
        let changed = self
            .db
            .merge_remote_positions(&resp.positions, true, raw_now_ms)?;

        for pos in &changed {
            let _ = self.events_tx.send(ClientEvent::PositionChanged {
                book_id: pos.book_id.clone(),
                position: pos.clone(),
                origin: PositionOrigin::RemotePull,
            });
        }

        Ok(changed)
    }
}

fn spawn_outbox_loop(inner: Arc<Inner>, mut shutdown_rx: watch::Receiver<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut attempt = 0u32;
        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            let has_work = inner.db.outbox_len().unwrap_or(0) > 0
                && inner.db.load_pairing().ok().flatten().is_some();

            if !has_work {
                attempt = 0;
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => break,
                    _ = inner.outbox_notify.notified() => continue,
                }
            }

            match inner.flush_outbox_once().await {
                Ok(_) => {
                    attempt = 0;
                }
                Err(err) if err.is_unauthorized() || matches!(err, ClientError::NotPaired) => {
                    attempt = 0;
                }
                Err(_) => {
                    attempt = attempt.saturating_add(1);
                    let delay = compute_backoff_with_jitter(
                        attempt,
                        inner.config.outbox_backoff_initial,
                        inner.config.outbox_backoff_max,
                    );
                    tokio::select! {
                        biased;
                        _ = shutdown_rx.changed() => break,
                        _ = inner.outbox_notify.notified() => {},
                        _ = inner.reconnect_notify.notified() => {},
                        _ = tokio::time::sleep(delay) => {},
                    }
                }
            }
        }
    })
}

fn spawn_sse_loop(inner: Arc<Inner>, mut shutdown_rx: watch::Receiver<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut attempt = 0u32;
        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            let Some(stored) = inner.db.load_pairing().ok().flatten() else {
                attempt = 0;
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => break,
                    _ = inner.reconnect_notify.notified() => continue,
                }
            };

            let last_id_str = inner
                .db
                .last_sse_event_id()
                .ok()
                .flatten()
                .map(|id| id.to_string());
            let mut extra_headers = Vec::new();
            if let Some(ref id_s) = last_id_str {
                extra_headers.push(("last-event-id", id_s.as_str()));
            }

            match inner
                .try_open_stream(&stored, Method::GET, "/events", &extra_headers, Vec::new())
                .await
            {
                Ok((addr, resp)) => {
                    if !resp.status().is_success() {
                        let status = resp.status();
                        let bytes = resp
                            .into_body()
                            .collect()
                            .await
                            .map(|c| c.to_bytes().to_vec())
                            .unwrap_or_default();
                        let err = parse_api_error(status, &bytes);
                        if err.is_unauthorized() {
                            inner.handle_revocation("token_revoked");
                            continue;
                        }
                    } else {
                        attempt = 0;
                        inner.connected_flag.store(true, AtomicOrdering::SeqCst);
                        let _ = inner.events_tx.send(ClientEvent::Connected { addr });

                        // Regra obrigatoria: ao (re)conectar o SSE, drena a outbox e faz pull()
                        let _ = inner.flush_outbox_once().await;
                        let _ = inner.pull_once().await;

                        let disconnect_reason = read_sse_stream_until_end(
                            &inner,
                            &stored.info.device_id,
                            resp.into_body(),
                            &mut shutdown_rx,
                        )
                        .await;

                        if *shutdown_rx.borrow() {
                            break;
                        }

                        if inner.connected_flag.swap(false, AtomicOrdering::SeqCst) {
                            let _ = inner.events_tx.send(ClientEvent::Disconnected {
                                reason: disconnect_reason.clone(),
                            });
                        }

                        // Se o stream fechou com EOF limpo (ex.: servidor revogou o token ou reiniciou),
                        // faz uma verificacao imediata antes do backoff para detectar 401 instantaneamente.
                        if disconnect_reason == "eof" {
                            if let Some(curr_stored) = inner.db.load_pairing().ok().flatten() {
                                match inner
                                    .try_request_bytes(
                                        &curr_stored,
                                        Method::GET,
                                        "/positions",
                                        &[],
                                        Vec::new(),
                                    )
                                    .await
                                {
                                    Err(e) if e.is_unauthorized() => continue,
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                Err(err) if err.is_unauthorized() => {
                    continue;
                }
                Err(err) => {
                    if inner.connected_flag.swap(false, AtomicOrdering::SeqCst) {
                        let _ = inner.events_tx.send(ClientEvent::Disconnected {
                            reason: err.to_string(),
                        });
                    }
                }
            }

            attempt = attempt.saturating_add(1);
            let delay = compute_backoff_with_jitter(
                attempt,
                inner.config.sse_backoff_initial,
                inner.config.sse_backoff_max,
            );
            tokio::select! {
                biased;
                _ = shutdown_rx.changed() => break,
                _ = inner.reconnect_notify.notified() => {},
                _ = tokio::time::sleep(delay) => {},
            }
        }
    })
}

async fn read_sse_stream_until_end(
    inner: &Arc<Inner>,
    my_device_id: &str,
    mut body: hyper::body::Incoming,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> String {
    let mut buffer = String::new();
    let read_timeout = inner.config.sse_read_timeout;
    let mut offline_rx = inner.offline_tx.subscribe();

    loop {
        if *offline_rx.borrow() {
            return "offline".to_string();
        }
        while let Some(pos) = buffer.find("\n\n") {
            let block = buffer[..pos].to_string();
            buffer.drain(..pos + 2);
            process_sse_block(inner, my_device_id, &block);
        }

        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                return "shutdown".to_string();
            }
            _ = offline_rx.changed() => {
                if *offline_rx.borrow() {
                    return "offline".to_string();
                }
            }
            frame_res = tokio::time::timeout(read_timeout, body.frame()) => {
                match frame_res {
                    Ok(Some(Ok(frame))) => {
                        if let Ok(data) = frame.into_data() {
                            buffer.push_str(&String::from_utf8_lossy(&data));
                        }
                    }
                    Ok(Some(Err(e))) => {
                        return format!("stream_error: {e}");
                    }
                    Ok(None) => {
                        return "eof".to_string();
                    }
                    Err(_) => {
                        return "read_timeout_half_open".to_string();
                    }
                }
            }
        }
    }
}

fn process_sse_block(inner: &Arc<Inner>, my_device_id: &str, block: &str) {
    let trimmed = block.trim();
    if trimmed.is_empty() || trimmed.starts_with(':') {
        return;
    }

    let mut event_id: Option<u64> = None;
    let mut event_type = "message";
    let mut data_lines = Vec::new();

    for line in block.lines() {
        if let Some(rest) = line.strip_prefix("id:") {
            event_id = rest.trim().parse::<u64>().ok();
        } else if let Some(rest) = line.strip_prefix("event:") {
            event_type = rest.trim();
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim());
        }
    }

    if let Some(seq) = event_id {
        let _ = inner.db.set_last_sse_event_id(seq);
    }

    if event_type != "position" || data_lines.is_empty() {
        return;
    }

    let data_joined = data_lines.join("\n");
    let Ok(payload) = serde_json::from_str::<SsePositionPayloadDto>(&data_joined) else {
        return;
    };

    if payload.seq > 0 {
        let _ = inner.db.set_last_sse_event_id(payload.seq);
    }

    // Nao reaplica eco do proprio dispositivo
    if payload.origin_device_id == my_device_id || payload.position.device_id == my_device_id {
        return;
    }

    let raw_now_ms = inner.config.clock.now_ms();
    if let Ok(changed) = inner
        .db
        .merge_remote_positions(&[payload.position], true, raw_now_ms)
    {
        for pos in changed {
            let _ = inner.events_tx.send(ClientEvent::PositionChanged {
                book_id: pos.book_id.clone(),
                position: pos,
                origin: PositionOrigin::RemoteSse,
            });
        }
    }
}

fn split_book_id_parts(book_id: &BookId) -> Result<(String, u64), ClientError> {
    let (md5_part, size_part) = book_id.as_str().split_once('-').ok_or_else(|| {
        ClientError::Core(bookferry_core::CoreError::InvalidBookId(
            book_id.as_str().to_string(),
        ))
    })?;
    let size = size_part.parse::<u64>().map_err(|_| {
        ClientError::Core(bookferry_core::CoreError::InvalidBookId(
            book_id.as_str().to_string(),
        ))
    })?;
    Ok((md5_part.to_string(), size))
}

fn extract_server_now_ms_from_err(err: &ClientError) -> Option<u64> {
    let ClientError::Api { message, .. } = err else {
        return None;
    };
    let idx = message.find("now_ms=")?;
    let tail = &message[idx + "now_ms=".len()..];
    let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u64>().ok()
}

/// Calcula backoff exponencial limitado a `max_delay` com jitter aleatorio de +/- 15%.
pub fn compute_backoff_with_jitter(
    attempt: u32,
    initial: Duration,
    max_delay: Duration,
) -> Duration {
    let shift = attempt.saturating_sub(1).min(10);
    let factor = 1u64 << shift;
    let base_ms = (initial.as_millis() as u64)
        .saturating_mul(factor)
        .min(max_delay.as_millis() as u64)
        .max(1);

    let mut rand_byte = [0u8; 1];
    let _ = getrandom::getrandom(&mut rand_byte);
    // Jitter em [85%, 115%] de base_ms
    let pct = 85u64 + (u64::from(rand_byte[0]) % 31);
    let jittered_ms = (base_ms.saturating_mul(pct) / 100).max(1);
    Duration::from_millis(jittered_ms)
}

/// Utilitario auxiliar para aguardar uma condicao assincrona em testes sem `sleep` fixo longo.
pub async fn wait_until<F, Fut>(timeout: Duration, poll_interval: Duration, mut cond: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if cond().await {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        let sleep_dur = poll_interval.min(deadline - now);
        tokio::time::sleep(sleep_dur).await;
    }
}

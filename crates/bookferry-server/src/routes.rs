use crate::db::{
    AuthenticatedDevice, BookPositionLookup, BookWithPosition, Database, DeviceSummary,
    PersistedSseEvent,
};
use crate::error::{ApiError, ErrorBody, HEADER_BOOKFERRY_API};
use crate::library::{resolve_safe_vault_file, scan_vault_dir};
use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{
    DefaultBodyLimit, FromRequest, FromRequestParts, Path as AxumPath, Query, Request, State,
};
use axum::http::header::{
    ACCEPT_RANGES, AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG,
    LAST_MODIFIED, RANGE,
};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use bookferry_core::{
    constant_time_eq_str, generate_random_hex, sha256_hex, BookId, Clock, CoreError, Hlc, HlcClock,
    PairingQrPayload, PositionHistoryEntry, PositionRecord, PositionUpdate, API_VERSION,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fmt;
use std::io::SeekFrom;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::{broadcast, watch};
use tokio_util::io::ReaderStream;

#[derive(Debug)]
pub struct PairingSession {
    pub secret: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub used: bool,
    pub failed_attempts: u32,
    pub locked_until_ms: Option<u64>,
}

impl PairingSession {
    pub fn new(secret: String, now_ms: u64, ttl_ms: u64) -> Self {
        Self {
            secret,
            created_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(ttl_ms),
            used: false,
            failed_attempts: 0,
            locked_until_ms: None,
        }
    }
}

pub struct AppState {
    pub db: Arc<Database>,
    pub hlc_clock: Arc<HlcClock<Arc<dyn Clock>>>,
    pub clock: Arc<dyn Clock>,
    pub started_at_ms: u64,
    pub server_name: String,
    pub vault_dir: PathBuf,
    pub cert_sha256: String,
    pub spki_sha256: String,
    pub announce_ips: Vec<String>,
    pub bound_port: u16,
    pub pairing: Mutex<PairingSession>,
    pub pairing_ttl_ms: u64,
    pub max_failed_pair_attempts: u32,
    pub pair_lockout_ms: u64,
    pub failure_delay: Duration,
    pub sse_keepalive_interval: Duration,
    pub sse_replay_limit: usize,
    pub position_history_limit: usize,
    pub max_future_drift_ms: u64,
    pub sse_tx: broadcast::Sender<PersistedSseEvent>,
    pub revocation_tx: broadcast::Sender<String>,
    pub shutdown_rx: watch::Receiver<bool>,
}

impl AppState {
    pub fn current_pairing_payload(&self) -> PairingQrPayload {
        let session = self.pairing.lock().unwrap_or_else(|e| e.into_inner());
        PairingQrPayload::new(
            self.announce_ips.clone(),
            self.bound_port,
            self.cert_sha256.clone(),
            self.spki_sha256.clone(),
            session.secret.clone(),
            self.server_name.clone(),
        )
    }

    pub fn rotate_pairing_secret(&self) -> Result<PairingQrPayload, CoreError> {
        let new_secret = format!("bf_pair_{}", generate_random_hex(32)?);
        let now_ms = self.clock.now_ms();
        let mut session = self.pairing.lock().unwrap_or_else(|e| e.into_inner());
        *session = PairingSession::new(new_secret.clone(), now_ms, self.pairing_ttl_ms);
        drop(session);
        Ok(self.current_pairing_payload())
    }
}

pub fn build_router(state: Arc<AppState>, max_json_body_bytes: usize) -> Router {
    Router::new()
        .route("/status", get(handle_status))
        .route("/pair", post(handle_pair))
        .route("/devices", get(handle_list_devices))
        .route("/devices/:id", delete(handle_revoke_device))
        .route("/library/rescan", post(handle_rescan_library))
        .route("/books", get(handle_list_books))
        .route("/books/:id/file", get(handle_get_book_file))
        .route(
            "/books/:id/position",
            put(handle_put_position).get(handle_get_position),
        )
        .route("/positions", get(handle_list_positions_since))
        .route("/events", get(handle_sse_events))
        .fallback(handle_not_found)
        .layer(DefaultBodyLimit::max(max_json_body_bytes))
        .layer(middleware::from_fn(uniform_api_response_middleware))
        .with_state(state)
}

async fn handle_not_found() -> ApiError {
    ApiError::not_found("route_not_found", "Rota HTTP nao encontrada")
}

/// Garante que TODAS as respostas contenham `X-Bookferry-Api: 1` e que rejeicoes
/// do framework (ex.: `413 Payload Too Large` do `DefaultBodyLimit` ou `405 Method Not Allowed`)
/// sejam sempre convertidas para o formato JSON uniforme `{error, message}`.
async fn uniform_api_response_middleware(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    resp.headers_mut()
        .insert(HEADER_BOOKFERRY_API, HeaderValue::from_static(API_VERSION));

    let status = resp.status();
    if status.is_client_error() || status.is_server_error() {
        let is_json = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.starts_with("application/json"));

        if !is_json {
            let (code, msg) = match status {
                StatusCode::PAYLOAD_TOO_LARGE => (
                    "payload_too_large",
                    "Corpo da requisicao excede o limite permitido",
                ),
                StatusCode::METHOD_NOT_ALLOWED => {
                    ("method_not_allowed", "Metodo HTTP nao permitido nesta rota")
                }
                StatusCode::NOT_FOUND => ("route_not_found", "Rota HTTP nao encontrada"),
                _ => ("http_error", "Requisicao invalida"),
            };
            let mut err_resp = (
                status,
                Json(ErrorBody {
                    error: code.to_string(),
                    message: msg.to_string(),
                }),
            )
                .into_response();
            err_resp
                .headers_mut()
                .insert(HEADER_BOOKFERRY_API, HeaderValue::from_static(API_VERSION));
            return err_resp;
        }
    }

    resp
}

/// Extrator JSON que converte erros de desserializacao ou limite de corpo em `ApiError`.
pub struct ApiJson<T>(pub T);

#[axum::async_trait]
impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(val)) => Ok(ApiJson(val)),
            Err(JsonRejection::BytesRejection(_)) => Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "Corpo JSON excede o tamanho maximo permitido",
            )),
            Err(rejection) => Err(ApiError::bad_request(
                "invalid_json",
                format!("JSON invalido: {rejection}"),
            )),
        }
    }
}

/// Extrator de autenticacao `Authorization: Bearer <token>`.
/// Nunca registra o token nos logs.
#[axum::async_trait]
impl FromRequestParts<Arc<AppState>> for AuthenticatedDevice {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let auth_header = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ApiError::unauthorized("Cabecalho Authorization ausente"))?;

        let raw_token = auth_header
            .strip_prefix("Bearer ")
            .or_else(|| auth_header.strip_prefix("bearer "))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| ApiError::unauthorized("Formato Bearer token invalido"))?;

        let candidate_sha256 = sha256_hex(raw_token.as_bytes());
        let now_ms = state.clock.now_ms();
        let dev = state
            .db
            .authenticate_token_sha256(&candidate_sha256, now_ms)?
            .ok_or_else(|| ApiError::unauthorized("Token de dispositivo invalido ou revogado"))?;

        Ok(dev)
    }
}

// ============================================================================
// GET /status
// ============================================================================

#[derive(Serialize, Deserialize, Debug)]
pub struct StatusResponse {
    pub status: String,
    pub version: String,
    pub server_name: String,
    pub book_count: u64,
    pub uptime_ms: u64,
}

async fn handle_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<StatusResponse>, ApiError> {
    let book_count = state.db.count_books()?;
    let now_ms = state.clock.now_ms();
    let uptime_ms = now_ms.saturating_sub(state.started_at_ms);

    Ok(Json(StatusResponse {
        status: "ok".to_string(),
        version: API_VERSION.to_string(),
        server_name: state.server_name.clone(),
        book_count,
        uptime_ms,
    }))
}

// ============================================================================
// POST /pair & /devices
// ============================================================================

#[derive(Deserialize)]
pub struct PairRequest {
    pub secret: String,
    pub device_name: String,
}

impl fmt::Debug for PairRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairRequest")
            .field("secret", &"[REDACTED]")
            .field("device_name", &self.device_name)
            .finish()
    }
}

#[derive(Serialize, Deserialize)]
pub struct PairResponse {
    pub device_id: String,
    pub device_name: String,
    pub token: String,
    pub cert_sha256: String,
    pub spki_sha256: String,
}

impl fmt::Debug for PairResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairResponse")
            .field("device_id", &self.device_id)
            .field("device_name", &self.device_name)
            .field("token", &"[REDACTED]")
            .field("cert_sha256", &self.cert_sha256)
            .field("spki_sha256", &self.spki_sha256)
            .finish()
    }
}

async fn handle_pair(
    State(state): State<Arc<AppState>>,
    ApiJson(req): ApiJson<PairRequest>,
) -> Result<Json<PairResponse>, ApiError> {
    let now_ms = state.clock.now_ms();
    let device_name = req.device_name.trim().to_string();
    if device_name.is_empty() || device_name.len() > 128 {
        return Err(ApiError::unprocessable(
            "invalid_device_name",
            "device_name deve ter entre 1 e 128 caracteres",
        ));
    }

    let mut wrong_secret = false;
    {
        let mut session = state.pairing.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(until_ms) = session.locked_until_ms {
            if now_ms < until_ms {
                return Err(ApiError::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    "pairing_rate_limited",
                    "Limite de tentativas falhas de pareamento excedido; tente novamente mais tarde",
                ));
            } else {
                session.locked_until_ms = None;
                session.failed_attempts = 0;
            }
        }

        if !constant_time_eq_str(&req.secret, &session.secret) {
            session.failed_attempts = session.failed_attempts.saturating_add(1);
            if session.failed_attempts >= state.max_failed_pair_attempts {
                session.locked_until_ms = Some(now_ms.saturating_add(state.pair_lockout_ms));
            }
            wrong_secret = true;
        } else if session.used {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "pairing_secret_already_used",
                "Segredo de pareamento de uso unico ja foi utilizado",
            ));
        } else if now_ms > session.expires_at_ms {
            return Err(ApiError::new(
                StatusCode::GONE,
                "pairing_secret_expired",
                "Segredo de pareamento expirado",
            ));
        } else {
            session.used = true;
            session.failed_attempts = 0;
            session.locked_until_ms = None;
        }
    }

    if wrong_secret {
        if !state.failure_delay.is_zero() {
            tokio::time::sleep(state.failure_delay).await;
        }
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_pairing_secret",
            "Segredo de pareamento invalido",
        ));
    }

    let device_id = format!("dev_{}", generate_random_hex(8)?);
    let raw_token = format!("bf_tok_{}", generate_random_hex(32)?);
    let token_sha256 = sha256_hex(raw_token.as_bytes());

    state
        .db
        .insert_device(&device_id, &device_name, &token_sha256, now_ms)?;

    tracing::info!(
        device_id = %device_id,
        device_name = %device_name,
        "novo dispositivo pareado com sucesso"
    );

    Ok(Json(PairResponse {
        device_id,
        device_name,
        token: raw_token,
        cert_sha256: state.cert_sha256.clone(),
        spki_sha256: state.spki_sha256.clone(),
    }))
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DevicesListResponse {
    pub devices: Vec<DeviceSummary>,
}

async fn handle_list_devices(
    _auth: AuthenticatedDevice,
    State(state): State<Arc<AppState>>,
) -> Result<Json<DevicesListResponse>, ApiError> {
    let devices = state.db.list_devices()?;
    Ok(Json(DevicesListResponse { devices }))
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RevokeDeviceResponse {
    pub revoked: bool,
    pub id: String,
}

async fn handle_revoke_device(
    _auth: AuthenticatedDevice,
    State(state): State<Arc<AppState>>,
    AxumPath(device_id): AxumPath<String>,
) -> Result<Json<RevokeDeviceResponse>, ApiError> {
    let now_ms = state.clock.now_ms();
    let found = state.db.revoke_device(&device_id, now_ms)?;
    if !found {
        return Err(ApiError::not_found(
            "device_not_found",
            "Dispositivo nao encontrado",
        ));
    }

    // Notifica streams SSE ativos desse dispositivo para encerramento imediato
    let _ = state.revocation_tx.send(device_id.clone());

    tracing::info!(revoked_device_id = %device_id, "dispositivo revogado");

    Ok(Json(RevokeDeviceResponse {
        revoked: true,
        id: device_id,
    }))
}

// ============================================================================
// Biblioteca: POST /library/rescan, GET /books, GET /books/{id}/file
// ============================================================================

#[derive(Serialize, Deserialize, Debug)]
pub struct RescanResponse {
    pub scanned_books: usize,
    pub duplicates_skipped: usize,
    pub total_books: u64,
}

async fn handle_rescan_library(
    _auth: AuthenticatedDevice,
    State(state): State<Arc<AppState>>,
) -> Result<Json<RescanResponse>, ApiError> {
    let vault_dir = state.vault_dir.clone();
    let report = tokio::task::spawn_blocking(move || scan_vault_dir(&vault_dir))
        .await
        .map_err(|e| ApiError::internal(format!("falha na tarefa de scan: {e}")))?
        .map_err(|e| ApiError::internal(format!("falha ao escanear biblioteca: {e}")))?;

    let now_ms = state.clock.now_ms();
    let scanned_books = state
        .db
        .upsert_scanned_books(&report.scanned_books, now_ms)?;
    let total_books = state.db.count_books()?;

    Ok(Json(RescanResponse {
        scanned_books,
        duplicates_skipped: report.duplicates_skipped,
        total_books,
    }))
}

#[derive(Serialize, Deserialize, Debug)]
pub struct BooksListResponse {
    pub books: Vec<BookWithPosition>,
}

async fn handle_list_books(
    _auth: AuthenticatedDevice,
    State(state): State<Arc<AppState>>,
) -> Result<Json<BooksListResponse>, ApiError> {
    let books = state.db.list_books_with_positions()?;
    Ok(Json(BooksListResponse { books }))
}

async fn handle_get_book_file(
    _auth: AuthenticatedDevice,
    State(state): State<Arc<AppState>>,
    AxumPath(raw_id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let book_id = BookId::parse(&raw_id)?;
    let book = state
        .db
        .get_book_by_id(&book_id)?
        .ok_or_else(|| ApiError::not_found("book_not_found", "Livro nao encontrado"))?;

    let canonical_path = resolve_safe_vault_file(&state.vault_dir, &book.rel_path)?;
    let mut file = tokio::fs::File::open(&canonical_path)
        .await
        .map_err(|_| ApiError::not_found("book_not_found", "Arquivo do livro ausente no disco"))?;

    let meta = file
        .metadata()
        .await
        .map_err(|_| ApiError::internal("Falha ao ler metadados do arquivo"))?;
    let total_size = meta.len();

    let etag_value = format!("\"{}-{}\"", book.book_id, book.mtime_ms);
    let last_modified_time = UNIX_EPOCH + Duration::from_millis(book.mtime_ms);
    let last_modified_str = httpdate::fmt_http_date(last_modified_time);
    let content_type = book.format.content_type();

    if let Some(range_hdr) = headers.get(RANGE) {
        let range_str = range_hdr.to_str().map_err(|_| {
            ApiError::new(
                StatusCode::RANGE_NOT_SATISFIABLE,
                "invalid_range",
                "Cabecalho Range malformado",
            )
            .with_header("content-range", format!("bytes */{total_size}"))
        })?;

        let (start, end) = parse_single_byte_range(range_str, total_size).ok_or_else(|| {
            ApiError::new(
                StatusCode::RANGE_NOT_SATISFIABLE,
                "invalid_range",
                "Intervalo HTTP Range invalido ou multiplos ranges nao suportados",
            )
            .with_header("content-range", format!("bytes */{total_size}"))
        })?;

        let chunk_len = end - start + 1;
        file.seek(SeekFrom::Start(start))
            .await
            .map_err(|_| ApiError::internal("Falha ao posicionar leitura do arquivo"))?;

        let stream = ReaderStream::with_capacity(file.take(chunk_len), 64 * 1024);
        let body = Body::from_stream(stream);

        let mut resp = Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(CONTENT_TYPE, content_type)
            .header(ACCEPT_RANGES, "bytes")
            .header(CONTENT_LENGTH, chunk_len.to_string())
            .header(CONTENT_RANGE, format!("bytes {start}-{end}/{total_size}"))
            .header(ETAG, etag_value)
            .header(LAST_MODIFIED, last_modified_str)
            .header(HEADER_BOOKFERRY_API, API_VERSION)
            .body(body)
            .map_err(|_| ApiError::internal("Falha ao construir resposta HTTP 206"))?;
        let _ = &mut resp;
        Ok(resp)
    } else {
        let stream = ReaderStream::with_capacity(file, 64 * 1024);
        let body = Body::from_stream(stream);

        let resp = Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, content_type)
            .header(ACCEPT_RANGES, "bytes")
            .header(CONTENT_LENGTH, total_size.to_string())
            .header(ETAG, etag_value)
            .header(LAST_MODIFIED, last_modified_str)
            .header(HEADER_BOOKFERRY_API, API_VERSION)
            .body(body)
            .map_err(|_| ApiError::internal("Falha ao construir resposta HTTP 200"))?;
        Ok(resp)
    }
}

/// Analisa um cabecalho `Range: bytes=...` de intervalo unico (RFC 9110).
/// Rejeita multiplos intervalos (`bytes=0-1,3-4`) ou intervalos fora dos limites do arquivo.
fn parse_single_byte_range(header: &str, total_size: u64) -> Option<(u64, u64)> {
    if total_size == 0 {
        return None;
    }
    let spec = header.trim().strip_prefix("bytes=")?;
    // Rejeita multiplos ranges de forma deterministica (documentado)
    if spec.contains(',') {
        return None;
    }
    let (start_str, end_str) = spec.split_once('-')?;
    let start_str = start_str.trim();
    let end_str = end_str.trim();

    if start_str.is_empty() {
        // Sufixo: `bytes=-N` (ultimos N bytes)
        let suffix_len = end_str.parse::<u64>().ok()?;
        if suffix_len == 0 {
            return None;
        }
        let start = total_size.saturating_sub(suffix_len);
        Some((start, total_size - 1))
    } else {
        let start = start_str.parse::<u64>().ok()?;
        if start >= total_size {
            return None;
        }
        if end_str.is_empty() {
            Some((start, total_size - 1))
        } else {
            let end = end_str.parse::<u64>().ok()?;
            if start > end {
                return None;
            }
            Some((start, end.min(total_size - 1)))
        }
    }
}

// ============================================================================
// Posicao de Leitura: PUT/GET /books/{id}/position e GET /positions?since=
// ============================================================================

#[derive(Serialize, Deserialize, Debug)]
pub struct PutPositionResponse {
    pub applied: bool,
    pub position: PositionRecord,
    pub server_seq: u64,
    pub server_hlc: String,
    pub history_count: usize,
}

async fn handle_put_position(
    _auth: AuthenticatedDevice,
    State(state): State<Arc<AppState>>,
    AxumPath(raw_id): AxumPath<String>,
    ApiJson(payload): ApiJson<PositionUpdate>,
) -> Result<Json<PutPositionResponse>, ApiError> {
    let book_id = BookId::parse(&raw_id)?;
    let now_ms = state.clock.now_ms();
    let validated = payload.validate(now_ms, state.max_future_drift_ms)?;

    let server_hlc = state
        .hlc_clock
        .receive(&validated.hlc, state.max_future_drift_ms)?;

    let outcome = state
        .db
        .put_position(
            &book_id,
            validated,
            now_ms,
            state.position_history_limit,
            state.sse_replay_limit,
        )?
        .ok_or_else(|| ApiError::not_found("book_not_found", "Livro nao encontrado"))?;

    if let Some(sse_ev) = outcome.sse_event {
        let _ = state.sse_tx.send(sse_ev);
    }

    Ok(Json(PutPositionResponse {
        applied: outcome.applied,
        position: outcome.position,
        server_seq: outcome.server_seq,
        server_hlc: server_hlc.to_canonical_string(),
        history_count: outcome.history_count,
    }))
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GetPositionResponse {
    pub position: PositionRecord,
    pub history: Vec<PositionHistoryEntry>,
}

async fn handle_get_position(
    _auth: AuthenticatedDevice,
    State(state): State<Arc<AppState>>,
    AxumPath(raw_id): AxumPath<String>,
) -> Result<Json<GetPositionResponse>, ApiError> {
    let book_id = BookId::parse(&raw_id)?;
    match state
        .db
        .get_book_position(&book_id, state.position_history_limit)?
    {
        BookPositionLookup::BookNotFound => Err(ApiError::not_found(
            "book_not_found",
            "Livro nao encontrado",
        )),
        BookPositionLookup::NoPositionYet => Err(ApiError::not_found(
            "position_not_found",
            "Nenhuma posicao registrada para este livro",
        )),
        BookPositionLookup::Found { position, history } => Ok(Json(GetPositionResponse {
            position: *position,
            history,
        })),
    }
}

#[derive(Deserialize, Debug, Default)]
pub struct PositionsSinceQuery {
    pub since: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PositionsListResponse {
    pub positions: Vec<PositionRecord>,
}

async fn handle_list_positions_since(
    _auth: AuthenticatedDevice,
    State(state): State<Arc<AppState>>,
    Query(query): Query<PositionsSinceQuery>,
) -> Result<Json<PositionsListResponse>, ApiError> {
    let since_canonical = match query.since {
        Some(ref raw) if !raw.trim().is_empty() => {
            let parsed = Hlc::parse(raw).map_err(|e| {
                ApiError::bad_request("invalid_hlc", format!("Parametro since invalido: {e}"))
            })?;
            Some(parsed.to_canonical_string())
        }
        _ => None,
    };

    let positions = state.db.list_positions_since(since_canonical.as_deref())?;
    Ok(Json(PositionsListResponse { positions }))
}

// ============================================================================
// Tempo Real: GET /events (Server-Sent Events)
// ============================================================================

async fn handle_sse_events(
    auth: AuthenticatedDevice,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let last_event_id: Option<u64> = match headers.get("last-event-id") {
        Some(val) => {
            let s = val.to_str().map_err(|_| {
                ApiError::bad_request("invalid_last_event_id", "Cabecalho Last-Event-ID invalido")
            })?;
            if s.trim().is_empty() {
                None
            } else {
                let parsed = s.trim().parse::<u64>().map_err(|_| {
                    ApiError::bad_request(
                        "invalid_last_event_id",
                        "Last-Event-ID deve ser um inteiro nao-negativo",
                    )
                })?;
                Some(parsed)
            }
        }
        None => None,
    };

    let mut sse_rx = state.sse_tx.subscribe();
    let mut revocation_rx = state.revocation_tx.subscribe();
    let mut shutdown_rx = state.shutdown_rx.clone();

    let replayed_events = match last_event_id {
        Some(after_seq) => state.db.list_sse_events_after(after_seq)?,
        None => Vec::new(),
    };

    let keepalive_interval = state.sse_keepalive_interval;
    let conn_device_id = auth.id;
    let conn_device_name = auth.name;

    let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(64);

    tokio::spawn(async move {
        // Comentario inicial para liberar cabecalhos HTTP imediatamente na conexao
        if chunk_tx
            .send(Ok(b": connected\n\n".to_vec()))
            .await
            .is_err()
        {
            return;
        }

        let mut max_sent_seq = last_event_id.unwrap_or(0);
        for ev in replayed_events {
            if ev.seq > max_sent_seq {
                max_sent_seq = ev.seq;
            }
            if is_same_origin(&ev.origin_device_id, &conn_device_id, &conn_device_name) {
                continue;
            }
            let frame = format_sse_frame(ev.seq, &ev.event_type, &ev.payload_json);
            if chunk_tx.send(Ok(frame)).await.is_err() {
                return;
            }
        }

        let mut ticker = tokio::time::interval(keepalive_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Consome o primeiro tick imediato do interval
        ticker.tick().await;

        loop {
            tokio::select! {
                biased;

                revoked = revocation_rx.recv() => {
                    if let Ok(revoked_id) = revoked {
                        if revoked_id == conn_device_id {
                            tracing::info!(device_id = %conn_device_id, "encerrando stream SSE de dispositivo revogado");
                            break;
                        }
                    }
                }

                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }

                maybe_ev = sse_rx.recv() => {
                    match maybe_ev {
                        Ok(ev) => {
                            if ev.seq <= max_sent_seq {
                                continue;
                            }
                            max_sent_seq = ev.seq;
                            if is_same_origin(&ev.origin_device_id, &conn_device_id, &conn_device_name) {
                                continue;
                            }
                            let frame = format_sse_frame(ev.seq, &ev.event_type, &ev.payload_json);
                            if chunk_tx.send(Ok(frame)).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!(device_id = %conn_device_id, skipped = skipped, "cliente SSE atrasou no canal broadcast");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }

                _ = ticker.tick() => {
                    if chunk_tx.send(Ok(b": keepalive\n\n".to_vec())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(chunk_rx);
    let body = Body::from_stream(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .header(CACHE_CONTROL, "no-cache")
        .header(HEADER_BOOKFERRY_API, API_VERSION)
        .body(body)
        .map_err(|_| ApiError::internal("Falha ao iniciar stream SSE"))
}

fn is_same_origin(origin_device_id: &str, conn_device_id: &str, conn_device_name: &str) -> bool {
    origin_device_id == conn_device_id || origin_device_id == conn_device_name
}

fn format_sse_frame(seq: u64, event_type: &str, data_json: &str) -> Vec<u8> {
    format!("id: {seq}\nevent: {event_type}\ndata: {data_json}\n\n").into_bytes()
}

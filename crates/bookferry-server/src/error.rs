use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bookferry_core::{CoreError, API_VERSION};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const HEADER_BOOKFERRY_API: &str = "x-bookferry-api";

/// Erro de inicializacao ou operacao interna do servidor.
#[derive(Debug, Error)]
pub enum ServerError {
    #[error("erro de I/O: {0}")]
    Io(#[from] std::io::Error),

    #[error("erro de banco de dados SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("erro de configuracao TLS: {0}")]
    Tls(String),

    #[error("erro de dominio: {0}")]
    Core(#[from] CoreError),

    #[error("configuracao invalida: {0}")]
    InvalidConfig(String),

    #[error("falha em tarefa interna: {0}")]
    TaskJoin(String),
}

/// Corpo JSON uniforme para todos os erros HTTP da API.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
    pub message: String,
}

/// Erro HTTP estruturado retornado pelos handlers da API.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub extra_header: Option<(&'static str, String)>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            extra_header: None,
        }
    }

    pub fn with_header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.extra_header = Some((name, value.into()));
        self
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    pub fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    pub fn not_found(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, code, message)
    }

    pub fn unprocessable(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, code, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
    }
}

impl From<CoreError> for ApiError {
    fn from(err: CoreError) -> Self {
        let code = err.error_code();
        let status = match err {
            CoreError::InvalidBookId(_) => StatusCode::BAD_REQUEST,
            CoreError::HlcTooFarInFuture { .. }
            | CoreError::InvalidHlcFormat(_)
            | CoreError::InvalidHlcNode(_)
            | CoreError::HlcLogicalOverflow
            | CoreError::InvalidDeviceId
            | CoreError::InvalidLocator
            | CoreError::SnippetTooLong { .. }
            | CoreError::InvalidPercent(_) => StatusCode::UNPROCESSABLE_ENTITY,
            CoreError::RandomGenerationFailed => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self::new(status, code, err.to_string())
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(err: rusqlite::Error) -> Self {
        tracing::error!(error = %err, "erro inesperado no SQLite");
        Self::internal("Erro interno ao acessar o banco de dados")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            error: self.code.to_string(),
            message: self.message,
        };
        let mut resp = (self.status, Json(body)).into_response();
        resp.headers_mut()
            .insert(HEADER_BOOKFERRY_API, HeaderValue::from_static(API_VERSION));
        if let Some((k, v)) = self.extra_header {
            if let Ok(hv) = HeaderValue::from_str(&v) {
                resp.headers_mut().insert(k, hv);
            }
        }
        resp
    }
}

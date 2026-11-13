use bookferry_core::CoreError;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const HEADER_BOOKFERRY_API: &str = "x-bookferry-api";

/// Corpo JSON uniforme de erro retornado pelo servidor Bookferry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub error: String,
    pub message: String,
}

/// Erros da biblioteca cliente `bookferry-client`.
/// Nenhuma variante armazena tokens ou segredos de pareamento em claro.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("erro de I/O: {0}")]
    Io(#[from] std::io::Error),

    #[error("erro no banco SQLite local: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("erro de validacao de dominio: {0}")]
    Core(#[from] CoreError),

    #[error("erro de serializacao JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("pin TLS obrigatorio ausente ou malformado (cert_sha256/spki_sha256)")]
    MissingTlsPin,

    #[error("verificacao de pin TLS SHA-256 falhou: {0}")]
    TlsPinMismatch(String),

    #[error("servidor respondeu sem TLS (HTTP puro ou protocolo invalido): {0}")]
    CleartextOrInvalidTls(String),

    #[error("falha no handshake TLS: {0}")]
    TlsHandshake(String),

    #[error("falha ao conectar ao servidor ({0})")]
    ConnectionFailed(String),

    #[error("timeout na operacao de rede ({0})")]
    Timeout(String),

    #[error("erro de protocolo HTTP: {0}")]
    Http(String),

    #[error("erro da API HTTP ({status} {code}): {message}")]
    Api {
        status: u16,
        code: String,
        message: String,
    },

    #[error("nao autorizado (token revogado ou invalido)")]
    Unauthorized,

    #[error("cliente nao esta pareado com nenhum servidor")]
    NotPaired,

    #[error("download cancelado pelo chamador")]
    Cancelled,

    #[error("download interrompido antes do fim ({downloaded_bytes}/{expected_bytes} bytes)")]
    DownloadInterrupted {
        downloaded_bytes: u64,
        expected_bytes: u64,
    },

    #[error("falha na verificacao de integridade do arquivo baixado: {0}")]
    IntegrityMismatch(String),

    #[error("configuracao ou argumento invalido: {0}")]
    InvalidInput(String),
}

impl ClientError {
    /// Indica se o erro corresponde a revogacao/invalidade de token (`401 Unauthorized`).
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, ClientError::Unauthorized)
            || matches!(self, ClientError::Api { status: 401, .. })
    }

    /// Indica se o erro e `422 hlc_too_far_in_future`.
    pub fn is_hlc_too_far_in_future(&self) -> bool {
        matches!(
            self,
            ClientError::Api {
                status: 422,
                code,
                ..
            } if code == "hlc_too_far_in_future"
        )
    }

    /// Indica se o erro e transitorio (rede, timeout, TLS temporario por queda ou HTTP 5xx),
    /// devendo preservar a fila `outbox` e usar cache offline quando aplicavel.
    pub fn is_transient(&self) -> bool {
        match self {
            ClientError::ConnectionFailed(_)
            | ClientError::Timeout(_)
            | ClientError::Http(_)
            | ClientError::Io(_) => true,
            ClientError::Api { status, .. } => *status >= 500 || *status == 429,
            _ => false,
        }
    }

    /// Retorna o codigo de erro da API, se for `ClientError::Api`.
    pub fn api_code(&self) -> Option<&str> {
        match self {
            ClientError::Api { code, .. } => Some(code.as_str()),
            ClientError::Unauthorized => Some("unauthorized"),
            _ => None,
        }
    }
}

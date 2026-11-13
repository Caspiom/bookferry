use crate::error::{ApiErrorBody, ClientError, HEADER_BOOKFERRY_API};
use bookferry_core::{constant_time_eq_str, sha256_hex, API_VERSION};
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::{HeaderMap, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

const PIN_MISMATCH_MARKER: &str = "[BOOKFERRY_PIN_MISMATCH]";

/// Pins criptograficos SHA-256 fixados no pareamento (certificado X.509 DER e/ou SPKI DER).
/// Pelo menos um dos dois deve ser um hash SHA-256 hex lowercase de 64 caracteres;
/// sem pin valido o cliente recusa qualquer conexao.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsPin {
    pub cert_sha256: Option<String>,
    pub spki_sha256: Option<String>,
}

impl TlsPin {
    pub fn new(
        cert_sha256: Option<String>,
        spki_sha256: Option<String>,
    ) -> Result<Self, ClientError> {
        let cert = normalize_optional_pin(cert_sha256)?;
        let spki = normalize_optional_pin(spki_sha256)?;
        if cert.is_none() && spki.is_none() {
            return Err(ClientError::MissingTlsPin);
        }
        Ok(Self {
            cert_sha256: cert,
            spki_sha256: spki,
        })
    }

    /// Verifica em tempo constante se um fingerprint `fp` (vindo do TXT mDNS)
    /// corresponde ao `cert_sha256` ou ao `spki_sha256` fixado.
    pub fn matches_fingerprint(&self, candidate_fp: &str) -> bool {
        let trimmed = candidate_fp.trim();
        if !is_valid_sha256_hex(trimmed) {
            return false;
        }
        let cert_ok = self
            .cert_sha256
            .as_deref()
            .is_some_and(|c| constant_time_eq_str(c, trimmed));
        let spki_ok = self
            .spki_sha256
            .as_deref()
            .is_some_and(|s| constant_time_eq_str(s, trimmed));
        cert_ok || spki_ok
    }
}

fn normalize_optional_pin(raw: Option<String>) -> Result<Option<String>, ClientError> {
    match raw {
        None => Ok(None),
        Some(s) => {
            let trimmed = s.trim().to_ascii_lowercase();
            if trimmed.is_empty() {
                Ok(None)
            } else if is_valid_sha256_hex(&trimmed) {
                Ok(Some(trimmed))
            } else {
                Err(ClientError::MissingTlsPin)
            }
        }
    }
}

pub fn is_valid_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Verificador customizado de certificado para `rustls` (provider `ring`) que valida
/// estritamente o pin SHA-256 (`cert_sha256` e/ou `spki_sha256`) em tempo constante
/// e verifica a assinatura criptografica do handshake TLS 1.2/1.3 sem nenhuma CA do sistema.
#[derive(Debug)]
pub struct PinnedServerCertVerifier {
    pin: TlsPin,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl PinnedServerCertVerifier {
    pub fn new(pin: TlsPin) -> Self {
        Self {
            pin,
            provider: Arc::new(rustls::crypto::ring::default_provider()),
        }
    }
}

impl ServerCertVerifier for PinnedServerCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let actual_cert_sha256 = sha256_hex(end_entity.as_ref());
        if let Some(ref expected_cert) = self.pin.cert_sha256 {
            if !constant_time_eq_str(&actual_cert_sha256, expected_cert) {
                return Err(TlsError::General(format!(
                    "{PIN_MISMATCH_MARKER} cert_sha256 esperado={expected_cert}, recebido={actual_cert_sha256}"
                )));
            }
        }

        if let Some(ref expected_spki) = self.pin.spki_sha256 {
            let spki_bytes = extract_spki_from_x509_der(end_entity.as_ref()).ok_or_else(|| {
                TlsError::General(format!(
                    "{PIN_MISMATCH_MARKER} nao foi possivel extrair SPKI do certificado X.509"
                ))
            })?;
            let actual_spki_sha256 = sha256_hex(spki_bytes);
            if !constant_time_eq_str(&actual_spki_sha256, expected_spki) {
                return Err(TlsError::General(format!(
                    "{PIN_MISMATCH_MARKER} spki_sha256 esperado={expected_spki}, recebido={actual_spki_sha256}"
                )));
            }
        }

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
            &self.provider.signature_verification_algorithms,
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
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Constroi `rustls::ClientConfig` usando `ring` e verificador de pin SHA-256 (zero CAs do sistema).
pub fn build_pinned_tls_config(pin: TlsPin) -> Result<ClientConfig, ClientError> {
    let verifier = PinnedServerCertVerifier::new(pin);
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| ClientError::TlsHandshake(format!("protocolo TLS invalido: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Ok(cfg)
}

/// Extrai o elemento ASN.1 DER `SubjectPublicKeyInfo` de um certificado X.509 v3 DER.
pub fn extract_spki_from_x509_der(cert_der: &[u8]) -> Option<&[u8]> {
    let (_, cert_body) = read_asn1_tlv(cert_der, 0x30)?;
    let (_, tbs_body) = read_asn1_tlv(cert_body, 0x30)?;

    let mut cursor = tbs_body;
    if cursor.first().copied() == Some(0xa0) {
        let (_, rest) = skip_asn1_tlv(cursor)?;
        cursor = rest;
    }
    for _ in 0..5 {
        let (_, rest) = skip_asn1_tlv(cursor)?;
        cursor = rest;
    }
    let (spki_full, _) = skip_asn1_tlv(cursor)?;
    if spki_full.first().copied() == Some(0x30) {
        Some(spki_full)
    } else {
        None
    }
}

fn read_asn1_tlv(input: &[u8], expected_tag: u8) -> Option<(&[u8], &[u8])> {
    if input.first().copied()? != expected_tag {
        return None;
    }
    let (len, hdr_len) = parse_asn1_len(&input[1..])?;
    let total = 1usize.checked_add(hdr_len)?.checked_add(len)?;
    if input.len() < total {
        return None;
    }
    Some((&input[..total], &input[1 + hdr_len..total]))
}

fn skip_asn1_tlv(input: &[u8]) -> Option<(&[u8], &[u8])> {
    if input.is_empty() {
        return None;
    }
    let (len, hdr_len) = parse_asn1_len(&input[1..])?;
    let total = 1usize.checked_add(hdr_len)?.checked_add(len)?;
    if input.len() < total {
        return None;
    }
    Some((&input[..total], &input[total..]))
}

fn parse_asn1_len(bytes: &[u8]) -> Option<(usize, usize)> {
    let first = *bytes.first()?;
    if (first & 0x80) == 0 {
        Some((first as usize, 1))
    } else {
        let num_octets = (first & 0x7f) as usize;
        if num_octets == 0 || num_octets > 4 || bytes.len() < 1 + num_octets {
            return None;
        }
        let mut len = 0usize;
        for &b in &bytes[1..=num_octets] {
            len = (len << 8) | (b as usize);
        }
        Some((len, 1 + num_octets))
    }
}

/// Cliente HTTP/1.1 sobre `tokio-rustls` com Pinning SHA-256 obrigatorio.
#[derive(Clone)]
pub struct PinnedHttpClient {
    connector: TlsConnector,
    connect_timeout: Duration,
    request_timeout: Duration,
}

impl PinnedHttpClient {
    pub fn new(
        pin: TlsPin,
        connect_timeout: Duration,
        request_timeout: Duration,
    ) -> Result<Self, ClientError> {
        let tls_cfg = build_pinned_tls_config(pin)?;
        Ok(Self {
            connector: TlsConnector::from(Arc::new(tls_cfg)),
            connect_timeout,
            request_timeout,
        })
    }

    pub async fn connect_tls(
        &self,
        addr: SocketAddr,
    ) -> Result<tokio_rustls::client::TlsStream<TcpStream>, ClientError> {
        let tcp = match tokio::time::timeout(self.connect_timeout, TcpStream::connect(addr)).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                return Err(ClientError::ConnectionFailed(format!("{addr}: {e}")));
            }
            Err(_) => {
                return Err(ClientError::Timeout(format!("TCP connect para {addr}")));
            }
        };
        let _ = tcp.set_nodelay(true);

        let domain = ServerName::try_from("bookferry.local")
            .map_err(|e| ClientError::TlsHandshake(format!("ServerName invalido: {e}")))?;

        match tokio::time::timeout(self.connect_timeout, self.connector.connect(domain, tcp)).await
        {
            Ok(Ok(tls)) => Ok(tls),
            Ok(Err(err)) => {
                let msg = err.to_string();
                if msg.contains(PIN_MISMATCH_MARKER) {
                    Err(ClientError::TlsPinMismatch(msg))
                } else if msg.contains("corrupt message")
                    || msg.contains("InvalidContentType")
                    || msg.contains("UnsupportedProtocolVersion")
                    || msg.contains("unexpected message")
                {
                    Err(ClientError::CleartextOrInvalidTls(msg))
                } else {
                    Err(ClientError::TlsHandshake(msg))
                }
            }
            Err(_) => Err(ClientError::Timeout(format!("TLS handshake com {addr}"))),
        }
    }

    /// Abre uma conexao HTTP/1.1 sobre TLS pinado e envia a requisicao retornando
    /// o `Response<Incoming>` (usado para streaming Range e SSE).
    pub async fn send_streaming_request(
        &self,
        addr: SocketAddr,
        method: Method,
        uri: &str,
        bearer_token: Option<&str>,
        extra_headers: &[(&str, &str)],
        body_bytes: Vec<u8>,
    ) -> Result<Response<Incoming>, ClientError> {
        let tls_stream = self.connect_tls(addr).await?;
        let io = TokioIo::new(tls_stream);
        let (mut sender, conn) = tokio::time::timeout(
            self.connect_timeout,
            hyper::client::conn::http1::handshake(io),
        )
        .await
        .map_err(|_| ClientError::Timeout(format!("HTTP handshake com {addr}")))?
        .map_err(|e| ClientError::Http(format!("HTTP handshake falhou ({addr}): {e}")))?;

        tokio::spawn(async move {
            let _ = conn.await;
        });

        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "bookferry.local");

        if let Some(tok) = bearer_token {
            // Nunca registra o token em log
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
            .map_err(|e| ClientError::Http(format!("requisicao HTTP invalida ({uri}): {e}")))?;

        let resp = tokio::time::timeout(self.request_timeout, sender.send_request(req))
            .await
            .map_err(|_| ClientError::Timeout(format!("aguardando cabecalhos HTTP de {uri}")))?
            .map_err(|e| ClientError::Http(format!("erro ao enviar {uri}: {e}")))?;

        let api_hdr = resp
            .headers()
            .get(HEADER_BOOKFERRY_API)
            .and_then(|v| v.to_str().ok());
        if api_hdr != Some(API_VERSION) {
            return Err(ClientError::Http(format!(
                "resposta sem X-Bookferry-Api: {API_VERSION} em {uri}"
            )));
        }

        Ok(resp)
    }

    /// Executa uma requisicao HTTP/1.1 e coleta o corpo completo.
    pub async fn request_bytes(
        &self,
        addr: SocketAddr,
        method: Method,
        uri: &str,
        bearer_token: Option<&str>,
        extra_headers: &[(&str, &str)],
        body_bytes: Vec<u8>,
    ) -> Result<(StatusCode, HeaderMap, Vec<u8>), ClientError> {
        let resp = self
            .send_streaming_request(addr, method, uri, bearer_token, extra_headers, body_bytes)
            .await?;
        let status = resp.status();
        let headers = resp.headers().clone();
        let collected = tokio::time::timeout(self.request_timeout, resp.into_body().collect())
            .await
            .map_err(|_| ClientError::Timeout(format!("lendo corpo HTTP de {uri}")))?
            .map_err(|e| ClientError::Http(format!("erro ao ler corpo HTTP de {uri}: {e}")))?
            .to_bytes()
            .to_vec();

        Ok((status, headers, collected))
    }

    /// Executa uma requisicao JSON tipada, mapeando codigos de erro da API (`401`, `422`, etc.).
    pub async fn request_json<T: DeserializeOwned>(
        &self,
        addr: SocketAddr,
        method: Method,
        uri: &str,
        bearer_token: Option<&str>,
        body_bytes: Vec<u8>,
    ) -> Result<T, ClientError> {
        let (status, _, bytes) = self
            .request_bytes(addr, method, uri, bearer_token, &[], body_bytes)
            .await?;

        if !status.is_success() {
            return Err(parse_api_error(status, &bytes));
        }

        serde_json::from_slice(&bytes).map_err(ClientError::Json)
    }
}

pub fn parse_api_error(status: StatusCode, bytes: &[u8]) -> ClientError {
    if let Ok(err_body) = serde_json::from_slice::<ApiErrorBody>(bytes) {
        if status == StatusCode::UNAUTHORIZED && err_body.error == "unauthorized" {
            return ClientError::Unauthorized;
        }
        return ClientError::Api {
            status: status.as_u16(),
            code: err_body.error,
            message: err_body.message,
        };
    }

    if status == StatusCode::UNAUTHORIZED {
        ClientError::Unauthorized
    } else {
        ClientError::Api {
            status: status.as_u16(),
            code: "http_error".to_string(),
            message: String::from_utf8_lossy(bytes).into_owned(),
        }
    }
}

use crate::error::ServerError;
use bookferry_core::{constant_time_eq_str, sha256_hex};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{
    ClientConfig, DigitallySignedStruct, Error as TlsError, ServerConfig, SignatureScheme,
};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use tokio_rustls::TlsAcceptor;

pub const CERT_DER_FILENAME: &str = "tls_cert.der";
pub const KEY_DER_FILENAME: &str = "tls_key.der";

/// Material criptografico TLS carregado ou gerado em `data_dir`.
#[derive(Clone)]
pub struct TlsIdentity {
    pub cert_der: Vec<u8>,
    pub spki_der: Vec<u8>,
    pub cert_sha256: String,
    pub spki_sha256: String,
    pub acceptor: TlsAcceptor,
}

impl std::fmt::Debug for TlsIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsIdentity")
            .field("cert_sha256", &self.cert_sha256)
            .field("spki_sha256", &self.spki_sha256)
            .finish()
    }
}

/// Carrega o par de chaves e certificado TLS de `data_dir` se ja existirem,
/// ou gera um novo certificado X.509 autoassinado ECDSA P-256 na primeira execucao,
/// gravando a chave privada com permissao `0600`.
pub fn load_or_generate_tls(
    data_dir: &Path,
    announce_ips: &[IpAddr],
    server_name: &str,
) -> Result<TlsIdentity, ServerError> {
    fs::create_dir_all(data_dir)?;
    let cert_path = data_dir.join(CERT_DER_FILENAME);
    let key_path = data_dir.join(KEY_DER_FILENAME);

    let (cert_der, key_der, spki_der) = if cert_path.exists() && key_path.exists() {
        enforce_key_permissions(&key_path)?;
        let cert_bytes = fs::read(&cert_path)?;
        let key_bytes = fs::read(&key_path)?;
        let pkcs8 = PrivatePkcs8KeyDer::from(key_bytes.as_slice());
        let key_pair = KeyPair::try_from(&pkcs8)
            .map_err(|e| ServerError::Tls(format!("chave TLS persistida invalida: {e}")))?;
        let spki = key_pair.public_key_der();
        (cert_bytes, key_bytes, spki)
    } else {
        let key_pair = KeyPair::generate()
            .map_err(|e| ServerError::Tls(format!("falha ao gerar par de chaves ECDSA: {e}")))?;
        let spki = key_pair.public_key_der();

        let mut params = CertificateParams::default();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, server_name);
        params.distinguished_name = dn;
        params.subject_alt_names.push(SanType::DnsName(
            "bookferry.local"
                .try_into()
                .map_err(|e| ServerError::Tls(format!("SAN DNS invalido: {e}")))?,
        ));
        params.subject_alt_names.push(SanType::DnsName(
            "localhost"
                .try_into()
                .map_err(|e| ServerError::Tls(format!("SAN DNS invalido: {e}")))?,
        ));
        for &ip in announce_ips {
            params.subject_alt_names.push(SanType::IpAddress(ip));
        }

        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| ServerError::Tls(format!("falha ao autoassinar certificado: {e}")))?;

        let cert_bytes = cert.der().to_vec();
        let key_bytes = key_pair.serialize_der();

        write_private_key_0600(&key_path, &key_bytes)?;
        fs::write(&cert_path, &cert_bytes)?;

        (cert_bytes, key_bytes, spki)
    };

    let cert_sha256 = sha256_hex(&cert_der);
    let spki_sha256 = sha256_hex(&spki_der);

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let server_config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| ServerError::Tls(format!("erro de protocolo TLS: {e}")))?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(cert_der.clone())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
        )
        .map_err(|e| ServerError::Tls(format!("certificado/chave TLS incompativeis: {e}")))?;

    Ok(TlsIdentity {
        cert_der,
        spki_der,
        cert_sha256,
        spki_sha256,
        acceptor: TlsAcceptor::from(Arc::new(server_config)),
    })
}

fn write_private_key_0600(path: &Path, bytes: &[u8]) -> Result<(), ServerError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        fs::write(path, bytes)?;
    }
    Ok(())
}

fn enforce_key_permissions(path: &Path) -> Result<(), ServerError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Verificador customizado de certificado para clientes `rustls` com pinning SHA-256
/// do certificado DER completo e/ou do SPKI, sem depender de CAs do sistema.
#[derive(Debug)]
pub struct PinnedServerCertVerifier {
    expected_cert_sha256: Option<String>,
    expected_spki_sha256: Option<String>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl PinnedServerCertVerifier {
    pub fn by_cert_sha256(cert_sha256: impl Into<String>) -> Self {
        Self {
            expected_cert_sha256: Some(cert_sha256.into()),
            expected_spki_sha256: None,
            provider: Arc::new(rustls::crypto::ring::default_provider()),
        }
    }

    pub fn by_cert_and_spki(
        cert_sha256: impl Into<String>,
        spki_sha256: impl Into<String>,
    ) -> Self {
        Self {
            expected_cert_sha256: Some(cert_sha256.into()),
            expected_spki_sha256: Some(spki_sha256.into()),
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
        if let Some(ref expected) = self.expected_cert_sha256 {
            if !constant_time_eq_str(&actual_cert_sha256, expected) {
                return Err(TlsError::General(format!(
                    "pinning SHA-256 do certificado recusado: esperado={expected}, recebido={actual_cert_sha256}"
                )));
            }
        }

        if let Some(ref expected_spki) = self.expected_spki_sha256 {
            let spki_bytes = extract_spki_from_x509_der(end_entity.as_ref()).ok_or_else(|| {
                TlsError::General("nao foi possivel extrair SPKI do certificado X.509".into())
            })?;
            let actual_spki_sha256 = sha256_hex(spki_bytes);
            if !constant_time_eq_str(&actual_spki_sha256, expected_spki) {
                return Err(TlsError::General(format!(
                    "pinning SHA-256 do SPKI recusado: esperado={expected_spki}, recebido={actual_spki_sha256}"
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

/// Constroi um `rustls::ClientConfig` que valida estritamente o pin SHA-256 do servidor.
pub fn build_pinned_client_config(
    cert_sha256: impl Into<String>,
    spki_sha256: Option<String>,
) -> Result<ClientConfig, ServerError> {
    let verifier = match spki_sha256 {
        Some(spki) => PinnedServerCertVerifier::by_cert_and_spki(cert_sha256, spki),
        None => PinnedServerCertVerifier::by_cert_sha256(cert_sha256),
    };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| ServerError::Tls(format!("erro de protocolo TLS cliente: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Ok(cfg)
}

/// Extrai o elemento ASN.1 DER `subjectPublicKeyInfo` (6o campo de `TBSCertificate`
/// quando `version [0]` esta presente, ou 5o quando omitido) de um certificado X.509 v3 DER.
pub fn extract_spki_from_x509_der(cert_der: &[u8]) -> Option<&[u8]> {
    let (_, cert_body) = read_asn1_tlv(cert_der, 0x30)?;
    let (_, tbs_body) = read_asn1_tlv(cert_body, 0x30)?;

    let mut cursor = tbs_body;
    // version [0] EXPLICIT opcional
    if cursor.first().copied() == Some(0xa0) {
        let (_, rest) = skip_asn1_tlv(cursor)?;
        cursor = rest;
    }
    // serialNumber, signature, issuer, validity, subject (5 elementos ASN.1)
    for _ in 0..5 {
        let (_, rest) = skip_asn1_tlv(cursor)?;
        cursor = rest;
    }
    // O proximo elemento SEQUENCE (0x30) completo (incluindo tag+length) e o SubjectPublicKeyInfo
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

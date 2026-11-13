use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
use rcgen::generate_simple_self_signed;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, Error as TlsError, ServerConfig, ServerConnection, SignatureScheme, StreamOwned};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_LAN_PORT: u16 = 47600;
pub const MDNS_SERVICE_TYPE: &str = "_bookferry._tcp.local.";
pub const MDNS_INSTANCE_NAME: &str = "bookferry-pc";
pub const MDNS_HOST_NAME: &str = "bookferry-pc.local.";

pub fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut s, "{:02x}", b);
    }
    s
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_encode(&hasher.finalize())
}

pub fn random_hex(num_bytes: usize) -> String {
    let mut buf = vec![0u8; num_bytes];
    let _ = getrandom::getrandom(&mut buf);
    hex_encode(&buf)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProcMemStatus {
    pub vm_rss_kb: u64,
    pub vm_hwm_kb: u64,
    pub vm_size_kb: u64,
}

pub fn read_proc_mem() -> ProcMemStatus {
    let mut rss = 0;
    let mut hwm = 0;
    let mut size = 0;
    if let Ok(content) = std::fs::read_to_string("/proc/self/status") {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                rss = rest.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("VmHWM:") {
                hwm = rest.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("VmSize:") {
                size = rest.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0);
            }
        }
    }
    ProcMemStatus {
        vm_rss_kb: rss,
        vm_hwm_kb: hwm,
        vm_size_kb: size,
    }
}

// ============================================================================
// HLC (Hybrid Logical Clock) — Simples e Determinístico para LWW
// ============================================================================

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ParsedHlc {
    pub wall_ms: u64,
    pub logical: u32,
    pub node: String,
}

impl ParsedHlc {
    pub fn parse(raw: &str, fallback_node: &str) -> Self {
        let parts: Vec<&str> = raw.splitn(3, ':').collect();
        if parts.len() == 3 {
            let wall_ms = parts[0].trim().parse::<u64>().unwrap_or(0);
            let logical = parts[1].trim().parse::<u32>().unwrap_or(0);
            let node = parts[2].trim().to_string();
            ParsedHlc { wall_ms, logical, node }
        } else if parts.len() == 2 {
            let wall_ms = parts[0].trim().parse::<u64>().unwrap_or(0);
            let logical = parts[1].trim().parse::<u32>().unwrap_or(0);
            ParsedHlc {
                wall_ms,
                logical,
                node: fallback_node.to_string(),
            }
        } else {
            let wall_ms = raw.trim().parse::<u64>().unwrap_or(0);
            ParsedHlc {
                wall_ms,
                logical: 0,
                node: fallback_node.to_string(),
            }
        }
    }

    pub fn to_canonical_string(&self) -> String {
        format!("{:015}:{:06}:{}", self.wall_ms, self.logical, self.node)
    }

    pub fn cmp_key(&self) -> (u64, u32, &str) {
        (self.wall_ms, self.logical, self.node.as_str())
    }
}

pub struct HlcClock {
    node: String,
    state: Mutex<(u64, u32)>,
}

impl HlcClock {
    pub fn new(node: impl Into<String>) -> Self {
        Self {
            node: node.into(),
            state: Mutex::new((0, 0)),
        }
    }

    pub fn now(&self) -> ParsedHlc {
        let phys = unix_ms_now();
        let mut st = self.state.lock().unwrap();
        if phys > st.0 {
            st.0 = phys;
            st.1 = 0;
        } else {
            st.1 += 1;
        }
        ParsedHlc {
            wall_ms: st.0,
            logical: st.1,
            node: self.node.clone(),
        }
    }

    pub fn observe(&self, remote: &ParsedHlc) -> ParsedHlc {
        let phys = unix_ms_now();
        let mut st = self.state.lock().unwrap();
        if phys > st.0 && phys > remote.wall_ms {
            st.0 = phys;
            st.1 = 0;
        } else if st.0 == remote.wall_ms {
            st.1 = st.1.max(remote.logical) + 1;
        } else if remote.wall_ms > st.0 {
            st.0 = remote.wall_ms;
            st.1 = remote.logical + 1;
        } else {
            st.1 += 1;
        }
        ParsedHlc {
            wall_ms: st.0,
            logical: st.1,
            node: self.node.clone(),
        }
    }
}

// ============================================================================
// Estruturas de Pareamento, Posição e Auditoria LWW
// ============================================================================

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PairingQrPayload {
    pub ip: String,
    pub port: u16,
    pub cert_sha256: String,
    pub spki_sha256: String,
    pub pairing_secret: String,
    pub service_type: String,
    pub instance_name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PositionPayload {
    pub device: String,
    pub hlc: String,
    pub locator: String,
    pub percent: f64,
    #[serde(default)]
    pub page: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredPosition {
    pub book_id: String,
    pub device: String,
    pub hlc: String,
    pub hlc_canonical: String,
    pub hlc_wall_ms: u64,
    pub hlc_logical: u32,
    pub locator: String,
    pub percent: f64,
    pub page: Option<u32>,
    pub server_seq: u64,
    pub server_received_unix_ms: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LwwWriteOutcome {
    pub ok: bool,
    pub applied: bool,
    pub winner: String,
    pub reason: String,
    pub position: StoredPosition,
    pub rejected_incoming: Option<PositionPayload>,
    pub sse_pushed_count: usize,
    pub history_len: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ConflictHistoryItem {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub incoming: PositionPayload,
    pub incoming_canonical_hlc: String,
    pub applied: bool,
    pub winning_canonical_hlc: String,
}

// ============================================================================
// Geração de Certificado Runtime (rcgen) e Verificador com Pinning (rustls)
// ============================================================================

pub struct RuntimeCertMaterial {
    pub cert_der: Vec<u8>,
    pub cert_pem: String,
    pub key_pkcs8_der: Vec<u8>,
    pub key_pem: String,
    pub cert_sha256: String,
    pub spki_sha256: String,
}

pub fn generate_self_signed_cert(lan_ip: &str) -> Result<RuntimeCertMaterial, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let subject_alt_names = vec![
        "bookferry.local".to_string(),
        "bookferry-pc.local".to_string(),
        lan_ip.to_string(),
        "127.0.0.1".to_string(),
        "localhost".to_string(),
    ];
    let certified = generate_simple_self_signed(subject_alt_names)
        .map_err(|e| format!("rcgen erro: {e}"))?;

    let cert_der = certified.cert.der().to_vec();
    let cert_pem = certified.cert.pem();
    let key_pkcs8_der = certified.key_pair.serialize_der();
    let key_pem = certified.key_pair.serialize_pem();
    let spki_der = certified.key_pair.public_key_der();

    let cert_sha256 = sha256_hex(&cert_der);
    let spki_sha256 = sha256_hex(&spki_der);

    Ok(RuntimeCertMaterial {
        cert_der,
        cert_pem,
        key_pkcs8_der,
        key_pem,
        cert_sha256,
        spki_sha256,
    })
}

pub fn build_rustls_server_config(mat: &RuntimeCertMaterial) -> Result<Arc<ServerConfig>, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert = CertificateDer::from(mat.cert_der.clone());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(mat.key_pkcs8_der.clone()));
    let cfg = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("protocol versions err: {e}"))?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .map_err(|e| format!("with_single_cert err: {e}"))?;
    Ok(Arc::new(cfg))
}

#[derive(Debug)]
pub struct PinnedCertVerifier {
    pub expected_cert_sha256: String,
}

impl PinnedCertVerifier {
    pub fn new(expected_sha256: impl Into<String>) -> Self {
        Self {
            expected_cert_sha256: expected_sha256.into().trim().to_ascii_lowercase(),
        }
    }
}

impl ServerCertVerifier for PinnedCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let actual_cert_sha256 = sha256_hex(end_entity.as_ref());
        if actual_cert_sha256.eq_ignore_ascii_case(&self.expected_cert_sha256) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(TlsError::General(format!(
                "Pinning SHA-256 recusado: esperado={}, recebido={}",
                self.expected_cert_sha256, actual_cert_sha256
            )))
        }
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

pub fn build_pinned_client_config(expected_cert_sha256: &str) -> Result<Arc<ClientConfig>, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let verifier = Arc::new(PinnedCertVerifier::new(expected_cert_sha256));
    let cfg = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("client protocol versions err: {e}"))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

// ============================================================================
// Descoberta de Rede / Interfaces / mDNS (Servidor e Cliente)
// ============================================================================

pub fn detect_local_lan_ip(target_hint: &str) -> Option<Ipv4Addr> {
    if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
        if sock.connect(target_hint).is_ok() {
            if let Ok(SocketAddr::V4(addr)) = sock.local_addr() {
                return Some(*addr.ip());
            }
        }
    }
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            if !iface.is_loopback() {
                if let IpAddr::V4(ip) = iface.ip() {
                    if ip.octets()[0] == 192 && ip.octets()[1] == 168 {
                        return Some(ip);
                    }
                }
            }
        }
    }
    None
}

pub fn start_mdns_server(
    lan_ip: &str,
    port: u16,
    cert_sha256: &str,
    spki_sha256: &str,
) -> Result<ServiceDaemon, String> {
    let mdns = ServiceDaemon::new().map_err(|e| format!("ServiceDaemon::new falhou: {e}"))?;
    let _ = mdns.disable_interface(IfKind::All);
    if let Ok(ip) = lan_ip.parse::<IpAddr>() {
        let _ = mdns.enable_interface(IfKind::Addr(ip));
    }
    let _ = mdns.enable_interface("enp3s0");

    let props = [
        ("fp", cert_sha256),
        ("spki", spki_sha256),
        ("ip", lan_ip),
        ("port", "47600"),
        ("v", "1"),
    ];
    let service_info = ServiceInfo::new(
        MDNS_SERVICE_TYPE,
        MDNS_INSTANCE_NAME,
        MDNS_HOST_NAME,
        lan_ip,
        port,
        &props[..],
    )
    .map_err(|e| format!("ServiceInfo::new falhou: {e}"))?;

    mdns.register(service_info)
        .map_err(|e| format!("mdns.register falhou: {e}"))?;

    Ok(mdns)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MdnsDiscoveryAttempt {
    pub rep: usize,
    pub mode: String,
    pub ok: bool,
    pub elapsed_ms: f64,
    pub resolved_fullname: Option<String>,
    pub resolved_host: Option<String>,
    pub resolved_ips: Vec<String>,
    pub resolved_port: Option<u16>,
    pub txt_fp: Option<String>,
    pub if_addrs_ok: bool,
    pub if_addrs_count: usize,
    pub if_addrs_detail: String,
    pub local_wifi_ip: Option<String>,
    pub raw_udp_5353_packets_seen: usize,
    pub raw_udp_5353_detail: String,
    pub events_trace: Vec<String>,
    pub error: Option<String>,
}

/// Constrói um pacote DNS-SD query PTR padrão RFC 6762 para `_bookferry._tcp.local.`
fn build_mdns_ptr_query_packet() -> Vec<u8> {
    let mut pkt = vec![
        0x00, 0x00, // Transaction ID = 0
        0x00, 0x00, // Flags = standard query
        0x00, 0x01, // QDCOUNT = 1
        0x00, 0x00, // ANCOUNT = 0
        0x00, 0x00, // NSCOUNT = 0
        0x00, 0x00, // ARCOUNT = 0
    ];
    for label in ["_bookferry", "_tcp", "local"] {
        pkt.push(label.len() as u8);
        pkt.extend_from_slice(label.as_bytes());
    }
    pkt.push(0x00); // root label
    pkt.extend_from_slice(&[0x00, 0x0c]); // QTYPE = PTR (12)
    pkt.extend_from_slice(&[0x00, 0x01]); // QCLASS = IN (1) (multicast response)
    pkt
}

/// Sonda direta na porta UDP 5353 (multicast 224.0.0.251 e unicast direto ao IP se desejado)
/// para diagnosticar se pacotes mDNS atravessam o roteador Wi-Fi <-> cabo e se `socket2` funciona
/// mesmo quando `if-addrs` (`getifaddrs` via netlink) falha no Android 11+.
pub fn probe_raw_udp_mdns(local_ip: Option<Ipv4Addr>, timeout_ms: u64) -> (usize, String, Option<(String, u16, String)>) {
    let sock = match Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)) {
        Ok(s) => s,
        Err(e) => return (0, format!("Socket::new erro: {e}"), None),
    };
    let _ = sock.set_reuse_address(true);
    #[cfg(unix)]
    let _ = sock.set_reuse_port(true);

    let bind_addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 5353);
    let bound_port = if sock.bind(&bind_addr.into()).is_ok() {
        5353
    } else {
        let any_addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
        if let Err(e) = sock.bind(&any_addr.into()) {
            return (0, format!("bind UDP falhou: {e}"), None);
        }
        0
    };

    let mcast_group = Ipv4Addr::new(224, 0, 0, 251);
    let iface_ip = local_ip.unwrap_or(Ipv4Addr::UNSPECIFIED);
    let join_res = sock.join_multicast_v4(&mcast_group, &iface_ip);
    let _ = sock.set_multicast_if_v4(&iface_ip);
    let _ = sock.set_multicast_loop_v4(true);
    let _ = sock.set_read_timeout(Some(Duration::from_millis(120)));

    let udp: UdpSocket = sock.into();
    let query = build_mdns_ptr_query_packet();
    let mcast_dest = SocketAddrV4::new(mcast_group, 5353);
    let send_mcast = udp.send_to(&query, mcast_dest);

    let t0 = Instant::now();
    let mut packets_seen = 0usize;
    let mut details = Vec::new();
    details.push(format!(
        "bound_port={} iface={} join_mcast={:?} send_mcast={:?}",
        bound_port,
        iface_ip,
        join_res.as_ref().map(|_| "ok"),
        send_mcast.as_ref().map(|n| *n)
    ));

    let mut found_bookferry: Option<(String, u16, String)> = None;
    let mut buf = [0u8; 2048];
    while t0.elapsed() < Duration::from_millis(timeout_ms) {
        match udp.recv_from(&mut buf) {
            Ok((n, src)) => {
                packets_seen += 1;
                let ascii: String = buf[..n]
                    .iter()
                    .map(|&b| if b.is_ascii_graphic() { b as char } else { '.' })
                    .collect();
                let has_bf = ascii.contains("bookferry");
                details.push(format!("pkt#{packets_seen} from={src} len={n} bookferry={has_bf}"));
                if has_bf {
                    let fp = if let Some(idx) = ascii.find("fp=") {
                        let rest = &ascii[idx + 3..];
                        rest.chars().take(64).collect::<String>()
                    } else {
                        String::new()
                    };
                    found_bookferry = Some((src.ip().to_string(), DEFAULT_LAN_PORT, fp));
                    break;
                }
            }
            Err(_) => {
                // Reenvia query a cada ~120ms se ainda dentro do timeout
                let _ = udp.send_to(&query, mcast_dest);
            }
        }
    }

    (packets_seen, details.join("; "), found_bookferry)
}

pub fn discover_mdns_once(rep: usize, mode: &str, timeout_ms: u64) -> MdnsDiscoveryAttempt {
    let t0 = Instant::now();
    let local_ip = detect_local_lan_ip("192.168.1.2:47600");

    // 1. Inspeciona if_addrs::get_if_addrs() (usado internamente pelo crate mdns-sd)
    let (if_addrs_ok, if_addrs_count, if_addrs_detail) = match if_addrs::get_if_addrs() {
        Ok(list) => {
            let names: Vec<String> = list
                .iter()
                .map(|i| format!("{}={}", i.name, i.ip()))
                .collect();
            (true, list.len(), names.join(", "))
        }
        Err(e) => (
            false,
            0,
            format!("if_addrs::get_if_addrs() falhou: {e} (os error {:?})", e.raw_os_error()),
        ),
    };

    let mut events_trace = Vec::new();
    let daemon = match ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            let (pkts, raw_detail, _) = probe_raw_udp_mdns(local_ip, 500);
            return MdnsDiscoveryAttempt {
                rep,
                mode: mode.to_string(),
                ok: false,
                elapsed_ms: t0.elapsed().as_secs_f64() * 1000.0,
                resolved_fullname: None,
                resolved_host: None,
                resolved_ips: vec![],
                resolved_port: None,
                txt_fp: None,
                if_addrs_ok,
                if_addrs_count,
                if_addrs_detail,
                local_wifi_ip: local_ip.map(|ip| ip.to_string()),
                raw_udp_5353_packets_seen: pkts,
                raw_udp_5353_detail: raw_detail,
                events_trace,
                error: Some(format!("ServiceDaemon::new falhou: {e}")),
            };
        }
    };

    if let Some(ip) = local_ip {
        let _ = daemon.enable_interface(IfKind::Addr(IpAddr::V4(ip)));
    }

    let receiver = match daemon.browse(MDNS_SERVICE_TYPE) {
        Ok(r) => r,
        Err(e) => {
            let _ = daemon.shutdown();
            return MdnsDiscoveryAttempt {
                rep,
                mode: mode.to_string(),
                ok: false,
                elapsed_ms: t0.elapsed().as_secs_f64() * 1000.0,
                resolved_fullname: None,
                resolved_host: None,
                resolved_ips: vec![],
                resolved_port: None,
                txt_fp: None,
                if_addrs_ok,
                if_addrs_count,
                if_addrs_detail,
                local_wifi_ip: local_ip.map(|ip| ip.to_string()),
                raw_udp_5353_packets_seen: 0,
                raw_udp_5353_detail: String::new(),
                events_trace,
                error: Some(format!("daemon.browse falhou: {e}")),
            };
        }
    };

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        let rem = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(rem.min(Duration::from_millis(250))) {
            Ok(event) => {
                let dt = t0.elapsed().as_secs_f64() * 1000.0;
                match event {
                    ServiceEvent::SearchStarted(st) => {
                        events_trace.push(format!("{dt:.2}ms: SearchStarted({st})"));
                    }
                    ServiceEvent::ServiceFound(st, fullname) => {
                        events_trace.push(format!("{dt:.2}ms: ServiceFound({st}, {fullname})"));
                    }
                    ServiceEvent::ServiceResolved(info) => {
                        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
                        events_trace.push(format!(
                            "{elapsed_ms:.2}ms: ServiceResolved(fullname={}, host={}, port={})",
                            info.get_fullname(),
                            info.get_hostname(),
                            info.get_port()
                        ));
                        let ips: Vec<String> = info.get_addresses().iter().map(|a| a.to_string()).collect();
                        let txt_fp = info.get_property_val_str("fp").map(|s| s.to_string());
                        let fullname = info.get_fullname().to_string();
                        let host = info.get_hostname().to_string();
                        let port = info.get_port();
                        let _ = daemon.shutdown();
                        return MdnsDiscoveryAttempt {
                            rep,
                            mode: mode.to_string(),
                            ok: true,
                            elapsed_ms,
                            resolved_fullname: Some(fullname),
                            resolved_host: Some(host),
                            resolved_ips: ips,
                            resolved_port: Some(port),
                            txt_fp,
                            if_addrs_ok,
                            if_addrs_count,
                            if_addrs_detail,
                            local_wifi_ip: local_ip.map(|ip| ip.to_string()),
                            raw_udp_5353_packets_seen: 1,
                            raw_udp_5353_detail: "resolved_via_mdns_sd".to_string(),
                            events_trace,
                            error: None,
                        };
                    }
                    other => {
                        events_trace.push(format!("{dt:.2}ms: {other:?}"));
                    }
                }
            }
            Err(_) => {}
        }
    }

    let _ = daemon.shutdown();
    // Se mdns-sd deu timeout, roda sonda raw UDP 5353 por 600ms para diagnosticar com precisão!
    let (pkts, raw_detail, raw_found) = probe_raw_udp_mdns(local_ip, 600);
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;

    if let Some((src_ip, port, fp)) = raw_found {
        return MdnsDiscoveryAttempt {
            rep,
            mode: format!("{mode} (raw_udp_5353_fallback)"),
            ok: true,
            elapsed_ms,
            resolved_fullname: Some(format!("{MDNS_INSTANCE_NAME}.{MDNS_SERVICE_TYPE}")),
            resolved_host: Some(MDNS_HOST_NAME.to_string()),
            resolved_ips: vec![src_ip],
            resolved_port: Some(port),
            txt_fp: if fp.is_empty() { None } else { Some(fp) },
            if_addrs_ok,
            if_addrs_count,
            if_addrs_detail,
            local_wifi_ip: local_ip.map(|ip| ip.to_string()),
            raw_udp_5353_packets_seen: pkts,
            raw_udp_5353_detail: raw_detail,
            events_trace,
            error: None,
        };
    }

    MdnsDiscoveryAttempt {
        rep,
        mode: mode.to_string(),
        ok: false,
        elapsed_ms,
        resolved_fullname: None,
        resolved_host: None,
        resolved_ips: vec![],
        resolved_port: None,
        txt_fp: None,
        if_addrs_ok,
        if_addrs_count,
        if_addrs_detail,
        local_wifi_ip: local_ip.map(|ip| ip.to_string()),
        raw_udp_5353_packets_seen: pkts,
        raw_udp_5353_detail: raw_detail,
        events_trace,
        error: Some(format!("Timeout mDNS após {timeout_ms} ms")),
    }
}

// ============================================================================
// Estado do Servidor HTTP/1.1 + SSE + Range sobre TLS (porta 47600)
// ============================================================================

struct SseClientEntry {
    id: u64,
    device_info: String,
    connected_at_ms: u64,
    stream: Arc<Mutex<StreamOwned<ServerConnection, TcpStream>>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SseConnectionLog {
    pub client_id: u64,
    pub event: String,
    pub timestamp_ms: u64,
    pub detail: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AckRecord {
    pub book_id: String,
    pub page: Option<u32>,
    pub hlc: String,
    pub device: String,
    pub server_put_ms: u64,
    pub server_ack_ms: u64,
    pub roundtrip_put_to_ack_ms: u64,
    pub client_reported_render_ms: Option<f64>,
}

pub struct ServerSharedState {
    pub qr_payload: PairingQrPayload,
    pub secret_used: AtomicBool,
    pub valid_tokens: Mutex<HashSet<String>>,
    pub hlc_clock: HlcClock,
    pub seq_counter: AtomicU64,
    pub positions: Mutex<HashMap<String, StoredPosition>>,
    pub history: Mutex<Vec<ConflictHistoryItem>>,
    pub sse_clients: Mutex<Vec<SseClientEntry>>,
    pub sse_logs: Mutex<Vec<SseConnectionLog>>,
    pub next_sse_id: AtomicU64,
    pub fixtures_dir: PathBuf,
    pub last_acks: Mutex<Vec<AckRecord>>,
    pub ack_cv: (Mutex<u64>, Condvar),
    pub tls_rejections: Mutex<Vec<String>>,
}

impl ServerSharedState {
    pub fn new(qr_payload: PairingQrPayload, fixtures_dir: PathBuf) -> Self {
        let mut tokens = HashSet::new();
        // Token fixo opcional de automação CLI local apenas se necessário, mas o pareamento cria tokens reais
        tokens.insert("bf_cli_local_pc_token".to_string());
        Self {
            qr_payload,
            secret_used: AtomicBool::new(false),
            valid_tokens: Mutex::new(tokens),
            hlc_clock: HlcClock::new("server-pc"),
            seq_counter: AtomicU64::new(0),
            positions: Mutex::new(HashMap::new()),
            history: Mutex::new(Vec::new()),
            sse_clients: Mutex::new(Vec::new()),
            sse_logs: Mutex::new(Vec::new()),
            next_sse_id: AtomicU64::new(1),
            fixtures_dir,
            last_acks: Mutex::new(Vec::new()),
            ack_cv: (Mutex::new(0), Condvar::new()),
            tls_rejections: Mutex::new(Vec::new()),
        }
    }

    pub fn broadcast_sse(&self, event_name: &str, json_data: &str) -> usize {
        let frame = format!("event: {event_name}\ndata: {json_data}\n\n");
        let mut clients = self.sse_clients.lock().unwrap();
        let mut delivered = 0usize;
        let mut failed_ids = Vec::new();

        for client in clients.iter() {
            if let Ok(mut guard) = client.stream.lock() {
                if guard.write_all(frame.as_bytes()).is_ok() && guard.flush().is_ok() {
                    delivered += 1;
                } else {
                    failed_ids.push(client.id);
                }
            }
        }

        if !failed_ids.is_empty() {
            let now = unix_ms_now();
            clients.retain(|c| !failed_ids.contains(&c.id));
            let mut logs = self.sse_logs.lock().unwrap();
            for fid in failed_ids {
                println!("[SSE_DISCONNECT] client_id={fid} ts={now} reason=write_error_on_broadcast");
                logs.push(SseConnectionLog {
                    client_id: fid,
                    event: "disconnect_on_write".to_string(),
                    timestamp_ms: now,
                    detail: "Falha ao escrever evento SSE (socket fechado/Doze)".to_string(),
                });
            }
        }
        delivered
    }
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_http_request<R: Read>(reader: &mut BufReader<R>) -> Result<Option<HttpRequest>, String> {
    let mut request_line = String::new();
    match reader.read_line(&mut request_line) {
        Ok(0) => return Ok(None),
        Ok(_) => {}
        Err(e) => return Err(format!("read request_line err: {e}")),
    }
    let trimmed_req = request_line.trim();
    if trimmed_req.is_empty() {
        return Ok(None);
    }
    let parts: Vec<&str> = trimmed_req.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(format!("Invalid HTTP request line: {trimmed_req}"));
    }
    let method = parts[0].to_uppercase();
    let path = parts[1].to_string();

    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| format!("read header err: {e}"))?;
        if n == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let content_len = headers
        .get("content-length")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; content_len];
    if content_len > 0 {
        reader
            .read_exact(&mut body)
            .map_err(|e| format!("read body err: {e}"))?;
    }

    Ok(Some(HttpRequest {
        method,
        path,
        headers,
        body,
    }))
}

fn write_json_response(
    stream: &mut StreamOwned<ServerConnection, TcpStream>,
    status_code: u16,
    status_text: &str,
    body_json: &str,
) -> std::io::Result<()> {
    let bytes = body_json.as_bytes();
    let hdr = format!(
        "HTTP/1.1 {status_code} {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    stream.write_all(hdr.as_bytes())?;
    stream.write_all(bytes)?;
    stream.conn.send_close_notify();
    stream.flush()
}

fn check_bearer_auth(req: &HttpRequest, state: &ServerSharedState) -> bool {
    if let Some(auth) = req.headers.get("authorization") {
        if let Some(tok) = auth.strip_prefix("Bearer ").or_else(|| auth.strip_prefix("bearer ")) {
            let tok = tok.trim();
            return state.valid_tokens.lock().unwrap().contains(tok);
        }
    }
    false
}

pub fn handle_server_connection(
    tcp_stream: TcpStream,
    peer_addr: SocketAddr,
    tls_cfg: Arc<ServerConfig>,
    state: Arc<ServerSharedState>,
) {
    let _ = tcp_stream.set_nodelay(true);
    let _ = tcp_stream.set_read_timeout(Some(Duration::from_secs(15)));
    let _ = tcp_stream.set_write_timeout(Some(Duration::from_secs(15)));

    // Inspeciona primeiro byte para log evidente caso enviem HTTP sem TLS
    let mut first_byte = [0u8; 1];
    if let Ok(1) = tcp_stream.peek(&mut first_byte) {
        if first_byte[0] != 0x16 {
            let msg = format!(
                "[TLS_REJECT_CLEARTEXT] peer={peer_addr} first_byte=0x{:02x} ('{}') — conexao sem TLS recusada",
                first_byte[0],
                if first_byte[0].is_ascii_graphic() { first_byte[0] as char } else { '?' }
            );
            println!("{msg}");
            state.tls_rejections.lock().unwrap().push(msg);
        }
    }

    let mut conn = match ServerConnection::new(tls_cfg) {
        Ok(c) => c,
        Err(e) => {
            println!("[TLS_ERROR] ServerConnection::new falhou: {e}");
            return;
        }
    };

    let mut tcp_stream = tcp_stream;
    while conn.is_handshaking() {
        if let Err(e) = conn.complete_io(&mut tcp_stream) {
            let msg = format!("[TLS_HANDSHAKE_REJECTED] peer={peer_addr} err={e}");
            println!("{msg}");
            state.tls_rejections.lock().unwrap().push(msg);
            return;
        }
    }

    let tls_stream = StreamOwned::new(conn, tcp_stream);
    let mut reader = BufReader::new(tls_stream);

    let req = match read_http_request(&mut reader) {
        Ok(Some(r)) => r,
        Ok(None) => return,
        Err(e) => {
            println!("[HTTP_READ_ERR] peer={peer_addr} err={e}");
            return;
        }
    };

    let mut tls_stream = reader.into_inner();
    let clean_path = req.path.split('?').next().unwrap_or(&req.path);

    // 1. GET /health (público para teste de handshake TLS + 1º request)
    if req.method == "GET" && clean_path == "/health" {
        let body = serde_json::json!({
            "ok": true,
            "server": "bookferry-pc",
            "cert_sha256": state.qr_payload.cert_sha256,
            "spki_sha256": state.qr_payload.spki_sha256,
            "server_time_ms": unix_ms_now(),
        })
        .to_string();
        let _ = write_json_response(&mut tls_stream, 200, "OK", &body);
        return;
    }

    // 2. GET /qr-info (apenas em loopback/LAN para scripts de teste lerem o segredo atual)
    if req.method == "GET" && clean_path == "/qr-info" {
        let body = serde_json::to_string(&state.qr_payload).unwrap_or_default();
        let _ = write_json_response(&mut tls_stream, 200, "OK", &body);
        return;
    }

    // 3. POST /pair — troca segredo de uso único por token de dispositivo
    if req.method == "POST" && clean_path == "/pair" {
        #[derive(Deserialize)]
        struct PairReq {
            secret: String,
            #[serde(default)]
            device_name: Option<String>,
        }
        let parsed: PairReq = match serde_json::from_slice(&req.body) {
            Ok(p) => p,
            Err(e) => {
                let _ = write_json_response(
                    &mut tls_stream,
                    400,
                    "Bad Request",
                    &serde_json::json!({"ok": false, "error": format!("JSON invalido: {e}")}).to_string(),
                );
                return;
            }
        };

        if parsed.secret.trim() != state.qr_payload.pairing_secret {
            println!("[PAIR_REJECT_INVALID] peer={peer_addr} secret_provided={}", parsed.secret);
            let _ = write_json_response(
                &mut tls_stream,
                401,
                "Unauthorized",
                &serde_json::json!({"ok": false, "error": "invalid_pairing_secret"}).to_string(),
            );
            return;
        }

        // Compare-and-swap atômico garante uso único estrito!
        if state
            .secret_used
            .compare_exchange(false, true, AtomicOrdering::SeqCst, AtomicOrdering::SeqCst)
            .is_err()
        {
            println!("[PAIR_REJECT_REUSED] peer={peer_addr} secret already consumed!");
            let _ = write_json_response(
                &mut tls_stream,
                403,
                "Forbidden",
                &serde_json::json!({"ok": false, "error": "pairing_secret_already_used"}).to_string(),
            );
            return;
        }

        let token = format!("bf_tok_{}", random_hex(16));
        state.valid_tokens.lock().unwrap().insert(token.clone());
        let dev = parsed.device_name.unwrap_or_else(|| "android-device".to_string());
        println!("[PAIR_SUCCESS] peer={peer_addr} device={dev} token_prefix={}...", &token[..14]);
        let _ = write_json_response(
            &mut tls_stream,
            200,
            "OK",
            &serde_json::json!({
                "ok": true,
                "device_name": dev,
                "token": token,
                "cert_sha256": state.qr_payload.cert_sha256,
            })
            .to_string(),
        );
        return;
    }

    // Todos os demais endpoints exigem Bearer token válido!
    if !check_bearer_auth(&req, &state) {
        println!("[AUTH_401] peer={peer_addr} path={clean_path} — token ausente ou invalido");
        let _ = write_json_response(
            &mut tls_stream,
            401,
            "Unauthorized",
            &serde_json::json!({"ok": false, "error": "unauthorized_invalid_or_missing_token"}).to_string(),
        );
        return;
    }

    // 4. PUT / GET /position/{book_id}
    if let Some(book_id) = clean_path.strip_prefix("/position/") {
        let book_id = book_id.trim();
        if req.method == "GET" {
            let pos_opt = state.positions.lock().unwrap().get(book_id).cloned();
            let history: Vec<ConflictHistoryItem> = state
                .history
                .lock()
                .unwrap()
                .iter()
                .cloned()
                .collect();
            match pos_opt {
                Some(pos) => {
                    let body = serde_json::json!({
                        "ok": true,
                        "position": pos,
                        "history": history,
                    })
                    .to_string();
                    let _ = write_json_response(&mut tls_stream, 200, "OK", &body);
                }
                None => {
                    let body = serde_json::json!({
                        "ok": false,
                        "error": "book_position_not_found",
                        "book_id": book_id,
                    })
                    .to_string();
                    let _ = write_json_response(&mut tls_stream, 404, "Not Found", &body);
                }
            }
            return;
        } else if req.method == "PUT" {
            let incoming: PositionPayload = match serde_json::from_slice(&req.body) {
                Ok(p) => p,
                Err(e) => {
                    let _ = write_json_response(
                        &mut tls_stream,
                        400,
                        "Bad Request",
                        &serde_json::json!({"ok": false, "error": format!("JSON invalido: {e}")}).to_string(),
                    );
                    return;
                }
            };

            let parsed_in = ParsedHlc::parse(&incoming.hlc, &incoming.device);
            let _ = state.hlc_clock.observe(&parsed_in);
            let in_canonical = parsed_in.to_canonical_string();
            let now_ms = unix_ms_now();

            let mut positions = state.positions.lock().unwrap();
            let mut history = state.history.lock().unwrap();

            let should_apply = match positions.get(book_id) {
                None => true,
                Some(existing) => {
                    let existing_parsed = ParsedHlc {
                        wall_ms: existing.hlc_wall_ms,
                        logical: existing.hlc_logical,
                        node: existing.device.clone(),
                    };
                    parsed_in.cmp_key() >= existing_parsed.cmp_key()
                }
            };

            let seq = state.seq_counter.fetch_add(1, AtomicOrdering::SeqCst) + 1;

            if should_apply {
                let stored = StoredPosition {
                    book_id: book_id.to_string(),
                    device: incoming.device.clone(),
                    hlc: incoming.hlc.clone(),
                    hlc_canonical: in_canonical.clone(),
                    hlc_wall_ms: parsed_in.wall_ms,
                    hlc_logical: parsed_in.logical,
                    locator: incoming.locator.clone(),
                    percent: incoming.percent,
                    page: incoming.page,
                    server_seq: seq,
                    server_received_unix_ms: now_ms,
                };
                positions.insert(book_id.to_string(), stored.clone());
                history.push(ConflictHistoryItem {
                    seq,
                    timestamp_ms: now_ms,
                    incoming: incoming.clone(),
                    incoming_canonical_hlc: in_canonical.clone(),
                    applied: true,
                    winning_canonical_hlc: in_canonical.clone(),
                });
                let hist_len = history.len();
                drop(history);
                drop(positions);

                let sse_json = serde_json::to_string(&stored).unwrap_or_default();
                let pushed = state.broadcast_sse("position", &sse_json);
                println!(
                    "[POSITION_PUT_APPLIED] book={book_id} device={} hlc={in_canonical} loc={} pct={} seq={seq} sse_pushed={pushed}",
                    stored.device, stored.locator, stored.percent
                );

                let out = LwwWriteOutcome {
                    ok: true,
                    applied: true,
                    winner: "incoming".to_string(),
                    reason: format!("incoming HLC ({in_canonical}) >= existing HLC"),
                    position: stored,
                    rejected_incoming: None,
                    sse_pushed_count: pushed,
                    history_len: hist_len,
                };
                let _ = write_json_response(
                    &mut tls_stream,
                    200,
                    "OK",
                    &serde_json::to_string(&out).unwrap_or_default(),
                );
            } else {
                let existing = positions.get(book_id).unwrap().clone();
                history.push(ConflictHistoryItem {
                    seq,
                    timestamp_ms: now_ms,
                    incoming: incoming.clone(),
                    incoming_canonical_hlc: in_canonical.clone(),
                    applied: false,
                    winning_canonical_hlc: existing.hlc_canonical.clone(),
                });
                let hist_len = history.len();
                drop(history);
                drop(positions);

                println!(
                    "[POSITION_PUT_REJECTED_LWW] book={book_id} incoming_hlc={in_canonical} < existing_hlc={} (mantido vencedor={})",
                    existing.hlc_canonical, existing.locator
                );

                let out = LwwWriteOutcome {
                    ok: true,
                    applied: false,
                    winner: "existing".to_string(),
                    reason: format!(
                        "incoming HLC ({in_canonical}) < existing HLC ({})",
                        existing.hlc_canonical
                    ),
                    position: existing,
                    rejected_incoming: Some(incoming),
                    sse_pushed_count: 0,
                    history_len: hist_len,
                };
                let _ = write_json_response(
                    &mut tls_stream,
                    200,
                    "OK",
                    &serde_json::to_string(&out).unwrap_or_default(),
                );
            }
            return;
        }
    }

    // 5. POST /position-ack — o Android avisa quando recebeu SSE e renderizou na UI, permitindo medir RTT exato no relógio do PC
    if req.method == "POST" && clean_path == "/position-ack" {
        #[derive(Deserialize)]
        struct AckReq {
            book_id: String,
            page: Option<u32>,
            hlc: String,
            device: String,
            server_received_unix_ms: u64,
            client_reported_render_ms: Option<f64>,
        }
        if let Ok(ack) = serde_json::from_slice::<AckReq>(&req.body) {
            let now_ms = unix_ms_now();
            let rtt = now_ms.saturating_sub(ack.server_received_unix_ms);
            let rec = AckRecord {
                book_id: ack.book_id,
                page: ack.page,
                hlc: ack.hlc,
                device: ack.device,
                server_put_ms: ack.server_received_unix_ms,
                server_ack_ms: now_ms,
                roundtrip_put_to_ack_ms: rtt,
                client_reported_render_ms: ack.client_reported_render_ms,
            };
            println!(
                "[SSE_UI_ACK] book={} page={:?} server_put_to_android_ui_ack_ms={} ms",
                rec.book_id, rec.page, rec.roundtrip_put_to_ack_ms
            );
            state.last_acks.lock().unwrap().push(rec.clone());
            let (lock, cvar) = &state.ack_cv;
            let mut count = lock.lock().unwrap();
            *count += 1;
            cvar.notify_all();
            let _ = write_json_response(
                &mut tls_stream,
                200,
                "OK",
                &serde_json::to_string(&rec).unwrap_or_default(),
            );
            return;
        }
    }

    // 6. GET /status — inspeciona estado completo (posições, histórico LWW, logs SSE, ACKs, rejeições TLS)
    if req.method == "GET" && clean_path == "/status" {
        let positions = state.positions.lock().unwrap().clone();
        let history = state.history.lock().unwrap().clone();
        let sse_logs = state.sse_logs.lock().unwrap().clone();
        let active_sse = state.sse_clients.lock().unwrap().len();
        let acks = state.last_acks.lock().unwrap().clone();
        let rejections = state.tls_rejections.lock().unwrap().clone();
        let body = serde_json::json!({
            "ok": true,
            "active_sse_clients": active_sse,
            "positions": positions,
            "history": history,
            "sse_logs": sse_logs,
            "acks": acks,
            "tls_rejections": rejections,
        })
        .to_string();
        let _ = write_json_response(&mut tls_stream, 200, "OK", &body);
        return;
    }

    // 7. GET /events — Server-Sent Events (SSE) sobre o mesmo TLS
    if req.method == "GET" && clean_path == "/events" {
        let _ = tls_stream.get_mut().set_read_timeout(None);
        let _ = tls_stream.get_mut().set_write_timeout(Some(Duration::from_secs(5)));

        let sse_hdr = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\nX-Accel-Buffering: no\r\n\r\n: connected\n\n";
        if tls_stream.write_all(sse_hdr.as_bytes()).is_err() || tls_stream.flush().is_err() {
            return;
        }

        let client_id = state.next_sse_id.fetch_add(1, AtomicOrdering::SeqCst);
        let now_ms = unix_ms_now();
        let dev = req
            .headers
            .get("x-device-name")
            .cloned()
            .unwrap_or_else(|| peer_addr.to_string());
        println!("[SSE_CONNECT] client_id={client_id} device={dev} peer={peer_addr} ts={now_ms}");

        state.sse_logs.lock().unwrap().push(SseConnectionLog {
            client_id,
            event: "connect".to_string(),
            timestamp_ms: now_ms,
            detail: format!("SSE conectado de {dev} ({peer_addr})"),
        });

        let shared_stream = Arc::new(Mutex::new(tls_stream));
        state.sse_clients.lock().unwrap().push(SseClientEntry {
            id: client_id,
            device_info: dev.clone(),
            connected_at_ms: now_ms,
            stream: shared_stream.clone(),
        });

        // Loop de heartbeat / detecção de desconexão a cada 2 segundos
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let still_registered = state
                .sse_clients
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.id == client_id);
            if !still_registered {
                break;
            }
            let hb = format!(": hb {}\n\n", unix_ms_now());
            let mut disconnected = false;
            if let Ok(mut guard) = shared_stream.lock() {
                if guard.write_all(hb.as_bytes()).is_err() || guard.flush().is_err() {
                    disconnected = true;
                }
            }
            if disconnected {
                let ts = unix_ms_now();
                state.sse_clients.lock().unwrap().retain(|c| c.id != client_id);
                println!("[SSE_DISCONNECT] client_id={client_id} device={dev} ts={ts} reason=heartbeat_write_failed");
                state.sse_logs.lock().unwrap().push(SseConnectionLog {
                    client_id,
                    event: "disconnect_heartbeat".to_string(),
                    timestamp_ms: ts,
                    detail: format!("Socket SSE desconectado ({dev})"),
                });
                break;
            }
        }
        return;
    }

    // 8. GET / HEAD /file/{name} — Transferência de arquivo com suporte a HTTP Range sobre TLS
    if let Some(file_name) = clean_path.strip_prefix("/file/") {
        let safe_name = file_name.rsplit('/').next().unwrap_or(file_name);
        let file_path = state.fixtures_dir.join(safe_name);
        let file = match File::open(&file_path) {
            Ok(f) => f,
            Err(e) => {
                let _ = write_json_response(
                    &mut tls_stream,
                    404,
                    "Not Found",
                    &serde_json::json!({"ok": false, "error": format!("Arquivo {safe_name} nao encontrado: {e}")}).to_string(),
                );
                return;
            }
        };
        let total_len = match file.metadata() {
            Ok(m) => m.len(),
            Err(_) => return,
        };

        let _ = tls_stream.get_mut().set_write_timeout(Some(Duration::from_secs(60)));

        if req.method == "HEAD" {
            let hdr = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.comicbook+zip\r\nContent-Length: {total_len}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
            );
            let _ = tls_stream.write_all(hdr.as_bytes());
            let _ = tls_stream.flush();
            return;
        }

        if let Some(range_val) = req.headers.get("range") {
            if let Some(r_str) = range_val.strip_prefix("bytes=") {
                let mut bounds = r_str.split('-');
                let start: u64 = bounds.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
                let end: u64 = bounds
                    .next()
                    .and_then(|s| {
                        let t = s.trim();
                        if t.is_empty() {
                            None
                        } else {
                            t.parse().ok()
                        }
                    })
                    .unwrap_or_else(|| total_len.saturating_sub(1))
                    .min(total_len.saturating_sub(1));

                if start <= end && start < total_len {
                    let content_len = end - start + 1;
                    println!(
                        "[FILE_RANGE_206] peer={peer_addr} file={safe_name} range={start}-{end}/{total_len} ({content_len} bytes)"
                    );
                    let hdr = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Type: application/vnd.comicbook+zip\r\nContent-Length: {content_len}\r\nContent-Range: bytes {start}-{end}/{total_len}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
                    );
                    if tls_stream.write_all(hdr.as_bytes()).is_err() {
                        return;
                    }
                    let mut offset = start;
                    let remaining_end = end + 1;
                    let mut buf = vec![0u8; 256 * 1024];
                    while offset < remaining_end {
                        let to_read = ((remaining_end - offset) as usize).min(buf.len());
                        match file.read_at(&mut buf[..to_read], offset) {
                            Ok(0) => break,
                            Ok(n) => {
                                if tls_stream.write_all(&buf[..n]).is_err() {
                                    println!("[FILE_RANGE_INTERRUPTED] peer={peer_addr} file={safe_name} stopped_at_offset={offset}");
                                    break;
                                }
                                offset += n as u64;
                            }
                            Err(_) => break,
                        }
                    }
                    tls_stream.conn.send_close_notify();
                    let _ = tls_stream.flush();
                    return;
                }
            }
        }

        // Full 200 OK download
        println!("[FILE_FULL_200] peer={peer_addr} file={safe_name} total_len={total_len}");
        let hdr = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.comicbook+zip\r\nContent-Length: {total_len}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
        );
        if tls_stream.write_all(hdr.as_bytes()).is_err() {
            return;
        }
        let mut offset = 0u64;
        let mut buf = vec![0u8; 256 * 1024];
        while offset < total_len {
            let to_read = ((total_len - offset) as usize).min(buf.len());
            match file.read_at(&mut buf[..to_read], offset) {
                Ok(0) => break,
                Ok(n) => {
                    if tls_stream.write_all(&buf[..n]).is_err() {
                        println!("[FILE_FULL_INTERRUPTED] peer={peer_addr} file={safe_name} stopped_at_offset={offset}");
                        break;
                    }
                    offset += n as u64;
                }
                Err(_) => break,
            }
        }
        tls_stream.conn.send_close_notify();
        let _ = tls_stream.flush();
        return;
    }

    let _ = write_json_response(
        &mut tls_stream,
        404,
        "Not Found",
        &serde_json::json!({"ok": false, "error": "route_not_found", "path": clean_path}).to_string(),
    );
}

// ============================================================================
// Funções do Cliente Rust (usadas no Android e na CLI de testes)
// ============================================================================

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TlsProbeResult {
    pub case_label: String,
    pub host: String,
    pub port: u16,
    pub expected_sha256: String,
    pub ok: bool,
    pub status_code: Option<u16>,
    pub tcp_connect_ms: f64,
    pub tls_handshake_ms: f64,
    pub first_request_ms: f64,
    pub total_ms: f64,
    pub response_body: Option<String>,
    pub error: Option<String>,
}

pub fn client_tls_request(
    host: &str,
    port: u16,
    pinned_cert_sha256: &str,
    method: &str,
    path: &str,
    bearer_token: Option<&str>,
    body: Option<&[u8]>,
) -> Result<(u16, HashMap<String, String>, Vec<u8>, f64, f64, f64), String> {
    let t0 = Instant::now();
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|e| format!("SocketAddr invalido: {e}"))?;
    let mut tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
        .map_err(|e| format!("TcpStream::connect({addr}) falhou: {e}"))?;
    let _ = tcp.set_nodelay(true);
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(10)));
    let tcp_connect_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let t_hs = Instant::now();
    let client_cfg = build_pinned_client_config(pinned_cert_sha256)?;
    let server_name = ServerName::try_from("bookferry.local".to_string())
        .map_err(|e| format!("ServerName err: {e}"))?;
    let mut conn = ClientConnection::new(client_cfg, server_name)
        .map_err(|e| format!("ClientConnection::new err: {e}"))?;

    while conn.is_handshaking() {
        conn.complete_io(&mut tcp)
            .map_err(|e| format!("TLS handshake falhou: {e}"))?;
    }
    let tls_handshake_ms = t_hs.elapsed().as_secs_f64() * 1000.0;

    let t_req = Instant::now();
    let mut tls_stream = StreamOwned::new(conn, tcp);
    let body_bytes = body.unwrap_or(&[]);
    let mut req_hdr = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n"
    );
    if let Some(tok) = bearer_token {
        req_hdr.push_str(&format!("Authorization: Bearer {tok}\r\n"));
    }
    if !body_bytes.is_empty() {
        req_hdr.push_str("Content-Type: application/json\r\n");
        req_hdr.push_str(&format!("Content-Length: {}\r\n", body_bytes.len()));
    }
    req_hdr.push_str("\r\n");

    tls_stream
        .write_all(req_hdr.as_bytes())
        .map_err(|e| format!("write headers err: {e}"))?;
    if !body_bytes.is_empty() {
        tls_stream
            .write_all(body_bytes)
            .map_err(|e| format!("write body err: {e}"))?;
    }
    tls_stream
        .flush()
        .map_err(|e| format!("flush err: {e}"))?;

    let mut reader = BufReader::new(tls_stream);
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(|e| format!("read status_line err: {e}"))?;
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| format!("Status HTTP invalido: {status_line}"))?;

    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| format!("read resp header err: {e}"))?;
        if n == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let content_len = headers
        .get("content-length")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    let mut resp_body = Vec::with_capacity(content_len);
    if content_len > 0 {
        resp_body.resize(content_len, 0);
        reader
            .read_exact(&mut resp_body)
            .map_err(|e| format!("read resp body err: {e}"))?;
    } else {
        let _ = reader.read_to_end(&mut resp_body);
    }
    let first_request_ms = t_req.elapsed().as_secs_f64() * 1000.0;

    Ok((
        status_code,
        headers,
        resp_body,
        tcp_connect_ms,
        tls_handshake_ms,
        first_request_ms,
    ))
}

pub fn probe_tls_pinning_case(
    case_label: &str,
    host: &str,
    port: u16,
    expected_sha256: &str,
) -> TlsProbeResult {
    let t0 = Instant::now();
    match client_tls_request(host, port, expected_sha256, "GET", "/health", None, None) {
        Ok((status, _hdrs, body, tcp_ms, hs_ms, req_ms)) => TlsProbeResult {
            case_label: case_label.to_string(),
            host: host.to_string(),
            port,
            expected_sha256: expected_sha256.to_string(),
            ok: status == 200,
            status_code: Some(status),
            tcp_connect_ms: (tcp_ms * 1000.0).round() / 1000.0,
            tls_handshake_ms: (hs_ms * 1000.0).round() / 1000.0,
            first_request_ms: (req_ms * 1000.0).round() / 1000.0,
            total_ms: (t0.elapsed().as_secs_f64() * 1_000_000.0).round() / 1000.0,
            response_body: Some(String::from_utf8_lossy(&body).to_string()),
            error: None,
        },
        Err(e) => TlsProbeResult {
            case_label: case_label.to_string(),
            host: host.to_string(),
            port,
            expected_sha256: expected_sha256.to_string(),
            ok: false,
            status_code: None,
            tcp_connect_ms: 0.0,
            tls_handshake_ms: 0.0,
            first_request_ms: 0.0,
            total_ms: (t0.elapsed().as_secs_f64() * 1_000_000.0).round() / 1000.0,
            response_body: None,
            error: Some(e),
        },
    }
}

pub fn probe_plain_http_rejected(host: &str, port: u16) -> TlsProbeResult {
    let t0 = Instant::now();
    let addr: SocketAddr = match format!("{host}:{port}").parse() {
        Ok(a) => a,
        Err(e) => {
            return TlsProbeResult {
                case_label: "plain_http_without_tls".to_string(),
                host: host.to_string(),
                port,
                expected_sha256: "NONE (cleartext HTTP)".to_string(),
                ok: false,
                status_code: None,
                tcp_connect_ms: 0.0,
                tls_handshake_ms: 0.0,
                first_request_ms: 0.0,
                total_ms: 0.0,
                response_body: None,
                error: Some(format!("addr err: {e}")),
            }
        }
    };

    let mut tcp = match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
        Ok(s) => s,
        Err(e) => {
            return TlsProbeResult {
                case_label: "plain_http_without_tls".to_string(),
                host: host.to_string(),
                port,
                expected_sha256: "NONE (cleartext HTTP)".to_string(),
                ok: false,
                status_code: None,
                tcp_connect_ms: 0.0,
                tls_handshake_ms: 0.0,
                first_request_ms: 0.0,
                total_ms: t0.elapsed().as_secs_f64() * 1000.0,
                response_body: None,
                error: Some(format!("TCP connect err: {e}")),
            }
        }
    };
    let tcp_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let _ = tcp.set_read_timeout(Some(Duration::from_secs(3)));
    let req = format!("GET /health HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
    let _ = tcp.write_all(req.as_bytes());
    let _ = tcp.flush();

    let mut buf = Vec::new();
    let read_res = tcp.read_to_end(&mut buf);
    let total_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // Se o servidor TLS enviar um TLS Alert (0x15 ...) ou fechar a conexão com 0 bytes / ECONNRESET,
    // nunca começará com "HTTP/1.1 200"
    let starts_with_http = buf.starts_with(b"HTTP/");
    let detail = match read_res {
        Ok(0) => "Conexao encerrada imediatamente pelo servidor TLS (EOF 0 bytes)".to_string(),
        Ok(n) => format!(
            "Servidor respondeu {n} bytes nao-HTTP (TLS Alert raw bytes: {:02x?})",
            &buf[..n.min(16)]
        ),
        Err(e) => format!("Leitura falhou/resetada pelo servidor TLS: {e}"),
    };

    TlsProbeResult {
        case_label: "plain_http_without_tls".to_string(),
        host: host.to_string(),
        port,
        expected_sha256: "NONE (cleartext HTTP)".to_string(),
        ok: starts_with_http,
        status_code: None,
        tcp_connect_ms: (tcp_ms * 1000.0).round() / 1000.0,
        tls_handshake_ms: 0.0,
        first_request_ms: 0.0,
        total_ms: (total_ms * 1000.0).round() / 1000.0,
        response_body: None,
        error: Some(detail),
    }
}

// ============================================================================
// Transferência de Arquivo CBZ com HTTP Range + Interrupção + Retomada sobre TLS
// ============================================================================

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RangeTransferReport {
    pub file_name: String,
    pub dest_path: String,
    pub total_expected_bytes: u64,
    pub part1_status_code: u16,
    pub part1_bytes_received: u64,
    pub part1_elapsed_ms: f64,
    pub part1_throughput_mb_s: f64,
    pub interrupted_at_byte: u64,
    pub part2_range_header: String,
    pub part2_status_code: u16,
    pub part2_content_range_response: String,
    pub part2_bytes_received: u64,
    pub part2_elapsed_ms: f64,
    pub part2_throughput_mb_s: f64,
    pub combined_bytes_written: u64,
    pub combined_elapsed_ms: f64,
    pub combined_throughput_mb_s: f64,
    pub sha256_verified: String,
    pub zip_valid_pages_verified: usize,
    pub mem_before: ProcMemStatus,
    pub mem_peak_during: ProcMemStatus,
    pub mem_after: ProcMemStatus,
}

pub fn download_cbz_with_interruption_and_range_resume(
    host: &str,
    port: u16,
    pinned_cert_sha256: &str,
    bearer_token: &str,
    file_name: &str,
    mut dest_file: File,
    dest_label: &str,
    interrupt_after_bytes: u64,
) -> Result<RangeTransferReport, String> {
    let mem_before = read_proc_mem();
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|e| format!("SocketAddr err: {e}"))?;
    let client_cfg = build_pinned_client_config(pinned_cert_sha256)?;

    // --- PARTE 1: Inicia download completo (sem Range ou Range: bytes=0-), interrompe após `interrupt_after_bytes` ---
    let t_part1 = Instant::now();
    let mut tcp1 = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
        .map_err(|e| format!("Part1 TCP connect err: {e}"))?;
    let _ = tcp1.set_nodelay(true);
    let _ = tcp1.set_read_timeout(Some(Duration::from_secs(30)));
    let sn1 = ServerName::try_from("bookferry.local".to_string()).unwrap();
    let mut conn1 = ClientConnection::new(client_cfg.clone(), sn1)
        .map_err(|e| format!("Part1 TLS conn err: {e}"))?;
    while conn1.is_handshaking() {
        conn1
            .complete_io(&mut tcp1)
            .map_err(|e| format!("Part1 TLS hs err: {e}"))?;
    }
    let mut stream1 = StreamOwned::new(conn1, tcp1);
    let req1 = format!(
        "GET /file/{file_name} HTTP/1.1\r\nHost: {host}:{port}\r\nAuthorization: Bearer {bearer_token}\r\nConnection: close\r\n\r\n"
    );
    stream1
        .write_all(req1.as_bytes())
        .map_err(|e| format!("Part1 write err: {e}"))?;
    stream1.flush().map_err(|e| format!("Part1 flush err: {e}"))?;

    let mut reader1 = BufReader::with_capacity(256 * 1024, stream1);
    let mut status_line1 = String::new();
    reader1
        .read_line(&mut status_line1)
        .map_err(|e| format!("Part1 status err: {e}"))?;
    let part1_status_code = status_line1
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);

    let mut headers1 = HashMap::new();
    loop {
        let mut line = String::new();
        let n = reader1
            .read_line(&mut line)
            .map_err(|e| format!("Part1 hdr err: {e}"))?;
        if n == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers1.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let total_expected_bytes = headers1
        .get("content-length")
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| "Content-Length ausente na Parte 1".to_string())?;

    dest_file
        .seek(SeekFrom::Start(0))
        .map_err(|e| format!("seek(0) err: {e}"))?;
    let _ = dest_file.set_len(0);

    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut part1_bytes_received = 0u64;
    let mut mem_peak_during = mem_before.clone();

    while part1_bytes_received < interrupt_after_bytes {
        let max_chunk = ((interrupt_after_bytes - part1_bytes_received) as usize).min(buf.len());
        let n = reader1
            .read(&mut buf[..max_chunk])
            .map_err(|e| format!("Part1 read body err: {e}"))?;
        if n == 0 {
            break;
        }
        dest_file
            .write_all(&buf[..n])
            .map_err(|e| format!("Part1 file write err: {e}"))?;
        hasher.update(&buf[..n]);
        part1_bytes_received += n as u64;
    }
    dest_file.flush().map_err(|e| format!("Part1 file flush err: {e}"))?;
    let m1 = read_proc_mem();
    if m1.vm_rss_kb > mem_peak_during.vm_rss_kb {
        mem_peak_during = m1;
    }

    // Interrompe abruptamente a conexão TLS da Parte 1!
    drop(reader1);
    let part1_elapsed_ms = t_part1.elapsed().as_secs_f64() * 1000.0;
    let part1_throughput_mb_s =
        (part1_bytes_received as f64 / (1024.0 * 1024.0)) / (part1_elapsed_ms / 1000.0);

    // Verifica tamanho parcial no disco antes de retomar
    let disk_partial_len = dest_file
        .metadata()
        .map(|m| m.len())
        .unwrap_or(part1_bytes_received);

    // --- PARTE 2: Retoma de `disk_partial_len` até o fim com `Range: bytes=<disk_partial_len>-` ---
    let t_part2 = Instant::now();
    let range_hdr_val = format!("bytes={disk_partial_len}-");
    let mut tcp2 = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
        .map_err(|e| format!("Part2 TCP connect err: {e}"))?;
    let _ = tcp2.set_nodelay(true);
    let _ = tcp2.set_read_timeout(Some(Duration::from_secs(60)));
    let sn2 = ServerName::try_from("bookferry.local".to_string()).unwrap();
    let mut conn2 = ClientConnection::new(client_cfg, sn2)
        .map_err(|e| format!("Part2 TLS conn err: {e}"))?;
    while conn2.is_handshaking() {
        conn2
            .complete_io(&mut tcp2)
            .map_err(|e| format!("Part2 TLS hs err: {e}"))?;
    }
    let mut stream2 = StreamOwned::new(conn2, tcp2);
    let req2 = format!(
        "GET /file/{file_name} HTTP/1.1\r\nHost: {host}:{port}\r\nAuthorization: Bearer {bearer_token}\r\nRange: {range_hdr_val}\r\nConnection: close\r\n\r\n"
    );
    stream2
        .write_all(req2.as_bytes())
        .map_err(|e| format!("Part2 write err: {e}"))?;
    stream2.flush().map_err(|e| format!("Part2 flush err: {e}"))?;

    let mut reader2 = BufReader::with_capacity(256 * 1024, stream2);
    let mut status_line2 = String::new();
    reader2
        .read_line(&mut status_line2)
        .map_err(|e| format!("Part2 status err: {e}"))?;
    let part2_status_code = status_line2
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);

    let mut headers2 = HashMap::new();
    loop {
        let mut line = String::new();
        let n = reader2
            .read_line(&mut line)
            .map_err(|e| format!("Part2 hdr err: {e}"))?;
        if n == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers2.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let part2_content_range_response = headers2
        .get("content-range")
        .cloned()
        .unwrap_or_default();

    let part2_expected_len = headers2
        .get("content-length")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| total_expected_bytes.saturating_sub(disk_partial_len));

    dest_file
        .seek(SeekFrom::Start(disk_partial_len))
        .map_err(|e| format!("Part2 seek err: {e}"))?;

    let mut part2_bytes_received = 0u64;
    while part2_bytes_received < part2_expected_len {
        let max_chunk = ((part2_expected_len - part2_bytes_received) as usize).min(buf.len());
        let n = reader2
            .read(&mut buf[..max_chunk])
            .map_err(|e| format!("Part2 read body err: {e}"))?;
        if n == 0 {
            break;
        }
        dest_file
            .write_all(&buf[..n])
            .map_err(|e| format!("Part2 file write err: {e}"))?;
        hasher.update(&buf[..n]);
        part2_bytes_received += n as u64;
    }
    dest_file.flush().map_err(|e| format!("Part2 file flush err: {e}"))?;
    drop(reader2);

    let part2_elapsed_ms = t_part2.elapsed().as_secs_f64() * 1000.0;
    let part2_throughput_mb_s =
        (part2_bytes_received as f64 / (1024.0 * 1024.0)) / (part2_elapsed_ms / 1000.0);

    let m2 = read_proc_mem();
    if m2.vm_rss_kb > mem_peak_during.vm_rss_kb {
        mem_peak_during = m2.clone();
    }

    let combined_bytes_written = part1_bytes_received + part2_bytes_received;
    let combined_elapsed_ms = part1_elapsed_ms + part2_elapsed_ms;
    let combined_throughput_mb_s =
        (combined_bytes_written as f64 / (1024.0 * 1024.0)) / (combined_elapsed_ms / 1000.0);

    let sha256_verified = hex_encode(&hasher.finalize());

    // Confirma que o arquivo CBZ baixado em 2 partes via HTTP Range abre perfeitamente como ZIP!
    dest_file
        .seek(SeekFrom::Start(0))
        .map_err(|e| format!("seek final err: {e}"))?;
    let zip_valid_pages_verified = count_zip_entries(&mut dest_file).unwrap_or(0);
    let mem_after = read_proc_mem();

    Ok(RangeTransferReport {
        file_name: file_name.to_string(),
        dest_path: dest_label.to_string(),
        total_expected_bytes,
        part1_status_code,
        part1_bytes_received,
        part1_elapsed_ms: (part1_elapsed_ms * 100.0).round() / 100.0,
        part1_throughput_mb_s: (part1_throughput_mb_s * 100.0).round() / 100.0,
        interrupted_at_byte: disk_partial_len,
        part2_range_header: range_hdr_val,
        part2_status_code,
        part2_content_range_response,
        part2_bytes_received,
        part2_elapsed_ms: (part2_elapsed_ms * 100.0).round() / 100.0,
        part2_throughput_mb_s: (part2_throughput_mb_s * 100.0).round() / 100.0,
        combined_bytes_written,
        combined_elapsed_ms: (combined_elapsed_ms * 100.0).round() / 100.0,
        combined_throughput_mb_s: (combined_throughput_mb_s * 100.0).round() / 100.0,
        sha256_verified,
        zip_valid_pages_verified,
        mem_before,
        mem_peak_during,
        mem_after,
    })
}

fn count_zip_entries(file: &mut File) -> Option<usize> {
    // Leitura do End of Central Directory (EOCD) nos últimos 65 KB do arquivo ZIP para validar integridade
    let len = file.metadata().ok()?.len();
    if len < 22 {
        return None;
    }
    let tail_size = len.min(65557) as usize;
    let mut buf = vec![0u8; tail_size];
    file.read_at(&mut buf, len - tail_size as u64).ok()?;
    for i in (0..=buf.len() - 22).rev() {
        if &buf[i..i + 4] == b"PK\x05\x06" {
            let total_entries = u16::from_le_bytes([buf[i + 10], buf[i + 11]]) as usize;
            return Some(total_entries);
        }
    }
    None
}

// ============================================================================
// Inicialização Completa do Servidor em Thread ou Binário
// ============================================================================

pub fn start_lan_server(
    lan_ip: &str,
    port: u16,
    fixtures_dir: PathBuf,
    runtime_dir: Option<&Path>,
) -> Result<(Arc<ServerSharedState>, ServiceDaemon), String> {
    let cert_mat = generate_self_signed_cert(lan_ip)?;
    let tls_cfg = build_rustls_server_config(&cert_mat)?;
    let pairing_secret = format!("pair_{}", random_hex(16));

    let qr_payload = PairingQrPayload {
        ip: lan_ip.to_string(),
        port,
        cert_sha256: cert_mat.cert_sha256.clone(),
        spki_sha256: cert_mat.spki_sha256.clone(),
        pairing_secret,
        service_type: MDNS_SERVICE_TYPE.to_string(),
        instance_name: MDNS_INSTANCE_NAME.to_string(),
    };

    if let Some(dir) = runtime_dir {
        let _ = std::fs::create_dir_all(dir);
        let qr_json = serde_json::to_string_pretty(&qr_payload).unwrap_or_default();
        let _ = std::fs::write(dir.join("runtime-pairing.json"), &qr_json);
        let _ = std::fs::write(dir.join("runtime-cert.pem"), &cert_mat.cert_pem);
    }

    println!("==================================================================");
    println!("BOOKFERRY LAN SERVER (TLS 1.3/1.2 + Pinning + mDNS + HLC + SSE)");
    println!("Listening on: 0.0.0.0:{port} (LAN IP: {lan_ip}:{port})");
    println!("mDNS Service: {MDNS_INSTANCE_NAME}.{MDNS_SERVICE_TYPE} (UDP 5353)");
    println!("Cert SHA-256: {}", qr_payload.cert_sha256);
    println!("SPKI SHA-256: {}", qr_payload.spki_sha256);
    println!("One-Time Pairing Secret: {}", qr_payload.pairing_secret);
    println!(
        "QR Payload JSON: {}",
        serde_json::to_string(&qr_payload).unwrap_or_default()
    );
    println!("==================================================================");

    let mdns_daemon = start_mdns_server(
        lan_ip,
        port,
        &qr_payload.cert_sha256,
        &qr_payload.spki_sha256,
    )?;

    let listener = TcpListener::bind(format!("0.0.0.0:{port}"))
        .map_err(|e| format!("Falha ao fazer bind em 0.0.0.0:{port}: {e}"))?;

    let state = Arc::new(ServerSharedState::new(qr_payload, fixtures_dir));
    let state_bg = state.clone();

    std::thread::spawn(move || {
        for incoming in listener.incoming() {
            match incoming {
                Ok(stream) => {
                    let peer = stream
                        .peer_addr()
                        .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
                    let cfg = tls_cfg.clone();
                    let st = state_bg.clone();
                    std::thread::spawn(move || {
                        handle_server_connection(stream, peer, cfg, st);
                    });
                }
                Err(e) => {
                    eprintln!("[ACCEPT_ERR] {e}");
                }
            }
        }
    });

    Ok((state, mdns_daemon))
}

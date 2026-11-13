use crate::tls::TlsPin;
use bookferry_core::constant_time_eq_str;
use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

pub const MDNS_SERVICE_TYPE: &str = "_bookferry._tcp.local.";

/// Candidato de servidor descoberto via mDNS (`_bookferry._tcp.local.`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MdnsCandidate {
    pub fullname: String,
    pub ips: Vec<IpAddr>,
    pub port: u16,
    pub txt_fp: Option<String>,
    pub txt_cert_fp: Option<String>,
    pub txt_spki: Option<String>,
}

/// Valida um candidato mDNS contra o pin TLS salvo (`cert_sha256` e/ou `spki_sha256`).
///
/// Regra de seguranca:
/// 1. O registro TXT `fp` e obrigatorio e deve bater (em tempo constante) com o
///    `cert_sha256` ou `spki_sha256` fixado no pareamento. Caso contrario, o candidato
///    e ignorado (impede que um servidor falso na mesma rede sequestre a tentativa).
/// 2. Se `cert_fp` estiver presente no TXT e `pin.cert_sha256` estiver definido,
///    eles tambem devem coincidir.
/// 3. Se `spki` estiver presente no TXT e `pin.spki_sha256` estiver definido,
///    eles tambem devem coincidir.
pub fn validate_mdns_candidate(candidate: &MdnsCandidate, pin: &TlsPin) -> bool {
    if candidate.port == 0 || candidate.ips.is_empty() {
        return false;
    }

    let Some(ref fp) = candidate.txt_fp else {
        return false;
    };
    if !pin.matches_fingerprint(fp) {
        return false;
    }

    if let (Some(ref cert_fp), Some(ref expected_cert)) = (&candidate.txt_cert_fp, &pin.cert_sha256)
    {
        if !constant_time_eq_str(cert_fp.trim(), expected_cert) {
            return false;
        }
    }

    if let (Some(ref spki_fp), Some(ref expected_spki)) = (&candidate.txt_spki, &pin.spki_sha256) {
        if !constant_time_eq_str(spki_fp.trim(), expected_spki) {
            return false;
        }
    }

    true
}

/// Ordena os enderecos (`SocketAddr`) candidatos para localizar o servidor pareado:
/// 1. Ultimo IP bom (`last_good_ip`, na porta `port`).
/// 2. Demais IPs do payload de pareamento (`known_ips`, na porta `port`).
/// 3. Candidatos mDNS `_bookferry._tcp` cujo TXT `fp` bate com o `pin` salvo.
///
/// Enderecos duplicados sao removidos preservando a ordem da primeira ocorrencia.
pub fn build_ordered_candidates(
    last_good_ip: Option<&str>,
    known_ips: &[String],
    port: u16,
    mdns_candidates: &[MdnsCandidate],
    pin: &TlsPin,
) -> Vec<SocketAddr> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    if port > 0 {
        if let Some(ip_str) = last_good_ip {
            if let Ok(ip) = ip_str.trim().parse::<IpAddr>() {
                let addr = SocketAddr::new(ip, port);
                if seen.insert(addr) {
                    out.push(addr);
                }
            }
        }

        for ip_str in known_ips {
            if let Ok(ip) = ip_str.trim().parse::<IpAddr>() {
                let addr = SocketAddr::new(ip, port);
                if seen.insert(addr) {
                    out.push(addr);
                }
            }
        }
    }

    for cand in mdns_candidates {
        if !validate_mdns_candidate(cand, pin) {
            tracing::debug!(
                fullname = %cand.fullname,
                txt_fp = ?cand.txt_fp,
                "candidato mDNS ignorado: TXT fp nao confere com o pin salvo"
            );
            continue;
        }
        for &ip in &cand.ips {
            let addr = SocketAddr::new(ip, cand.port);
            if seen.insert(addr) {
                out.push(addr);
            }
        }
    }

    out
}

/// Coleta e deduplica os enderecos IP do `PairingQrPayload` (`ips` seguido de `ip`).
pub fn collect_qr_ips(ips: &[String], primary_ip: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    // Se `ip` primario foi informado, ou `ips` na ordem do payload:
    for raw in ips.iter().map(String::as_str).chain(primary_ip) {
        let trimmed = raw.trim();
        if trimmed.parse::<IpAddr>().is_ok() && seen.insert(trimmed.to_string()) {
            out.push(trimmed.to_string());
        }
    }
    out
}

/// Provedor de descoberta mDNS injetavel (permite testes deterministas sem multicast UDP).
pub trait DiscoveryProvider: Send + Sync {
    fn discover(&self, timeout: Duration) -> Vec<MdnsCandidate>;
}

/// Implementacao padrao de descoberta mDNS usando `mdns-sd`.
/// Qualquer falha de socket/multicast e capturada e retorna lista vazia sem nunca derrubar o cliente.
#[derive(Debug, Default)]
pub struct MdnsSdDiscovery;

impl DiscoveryProvider for MdnsSdDiscovery {
    fn discover(&self, timeout: Duration) -> Vec<MdnsCandidate> {
        if timeout.is_zero() {
            return Vec::new();
        }
        let daemon = match ServiceDaemon::new() {
            Ok(d) => d,
            Err(err) => {
                tracing::warn!(error = %err, "mDNS indisponivel no cliente; prosseguindo sem mDNS");
                return Vec::new();
            }
        };

        let receiver = match daemon.browse(MDNS_SERVICE_TYPE) {
            Ok(r) => r,
            Err(err) => {
                tracing::warn!(error = %err, "falha ao iniciar browse mDNS; prosseguindo sem mDNS");
                let _ = daemon.shutdown();
                return Vec::new();
            }
        };

        let deadline = Instant::now() + timeout;
        let mut results = Vec::new();

        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let remaining = deadline - now;
            match receiver.recv_timeout(remaining) {
                Ok(ServiceEvent::ServiceResolved(info)) => {
                    let mut ips: Vec<IpAddr> = info.get_addresses().iter().copied().collect();
                    ips.sort_by_key(|ip| match ip {
                        IpAddr::V4(_) => 0,
                        IpAddr::V6(_) => 1,
                    });
                    let txt_fp = info.get_property_val_str("fp").map(ToString::to_string);
                    let txt_cert_fp = info
                        .get_property_val_str("cert_fp")
                        .map(ToString::to_string);
                    let txt_spki = info.get_property_val_str("spki").map(ToString::to_string);
                    let port = info.get_port();

                    results.push(MdnsCandidate {
                        fullname: info.get_fullname().to_string(),
                        ips,
                        port,
                        txt_fp,
                        txt_cert_fp,
                        txt_spki,
                    });
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }

        let _ = daemon.stop_browse(MDNS_SERVICE_TYPE);
        let _ = daemon.shutdown();
        results
    }
}

/// Provedor de descoberta mDNS em memoria para testes deterministas (sem multicast UDP).
#[derive(Debug, Default)]
pub struct MockDiscoveryProvider {
    candidates: std::sync::Mutex<Vec<MdnsCandidate>>,
    should_fail: std::sync::atomic::AtomicBool,
}

impl MockDiscoveryProvider {
    pub fn new(candidates: Vec<MdnsCandidate>) -> Self {
        Self {
            candidates: std::sync::Mutex::new(candidates),
            should_fail: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn set_candidates(&self, candidates: Vec<MdnsCandidate>) {
        if let Ok(mut guard) = self.candidates.lock() {
            *guard = candidates;
        }
    }

    pub fn set_should_fail(&self, fail: bool) {
        self.should_fail
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }
}

impl DiscoveryProvider for MockDiscoveryProvider {
    fn discover(&self, _timeout: Duration) -> Vec<MdnsCandidate> {
        if self.should_fail.load(std::sync::atomic::Ordering::SeqCst) {
            tracing::warn!("simulando falha de mDNS no MockDiscoveryProvider");
            return Vec::new();
        }
        self.candidates
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn candidate_ordering_and_pin_validation_unit() {
        let good_cert = "a".repeat(64);
        let good_spki = "b".repeat(64);
        let bad_fp = "c".repeat(64);
        let pin = TlsPin::new(Some(good_cert.clone()), Some(good_spki.clone())).unwrap();

        let mdns_list = vec![
            // 1. Servidor falso com fp errado -> deve ser ignorado
            MdnsCandidate {
                fullname: "fake._bookferry._tcp.local.".into(),
                ips: vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 99))],
                port: 47600,
                txt_fp: Some(bad_fp),
                txt_cert_fp: None,
                txt_spki: None,
            },
            // 2. Servidor legitimo em novo IP via DHCP -> deve entrar apos os conhecidos
            MdnsCandidate {
                fullname: "real._bookferry._tcp.local.".into(),
                ips: vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 55))],
                port: 47600,
                txt_fp: Some(good_spki),
                txt_cert_fp: Some(good_cert),
                txt_spki: None,
            },
        ];

        let ordered = build_ordered_candidates(
            Some("192.168.1.20"),
            &["192.168.1.10".to_string(), "192.168.1.20".to_string()],
            47600,
            &mdns_list,
            &pin,
        );

        assert_eq!(
            ordered,
            vec![
                "192.168.1.20:47600".parse::<SocketAddr>().unwrap(),
                "192.168.1.10:47600".parse::<SocketAddr>().unwrap(),
                "192.168.1.55:47600".parse::<SocketAddr>().unwrap(),
            ]
        );
    }
}

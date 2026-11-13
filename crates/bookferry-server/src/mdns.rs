use bookferry_core::API_VERSION;
use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo};
use std::net::IpAddr;

pub const MDNS_SERVICE_TYPE: &str = "_bookferry._tcp.local.";

pub struct MdnsHandle {
    daemon: ServiceDaemon,
    fullname: String,
}

impl MdnsHandle {
    /// Inicia o anuncio mDNS (`_bookferry._tcp.local.`) restrito exclusivamente aos IPs e
    /// interfaces configurados. Qualquer erro de rede/multicast apenas emite `tracing::warn!`
    /// e retorna `None`, sem nunca derrubar o servidor.
    pub fn try_start(
        server_name: &str,
        port: u16,
        announce_ips: &[IpAddr],
        announce_interfaces: &[String],
        spki_sha256: &str,
        cert_sha256: &str,
    ) -> Option<Self> {
        if announce_ips.is_empty() && announce_interfaces.is_empty() {
            tracing::debug!("mDNS nao iniciado: nenhum IP ou interface LAN configurado em Config");
            return None;
        }

        let daemon = match ServiceDaemon::new() {
            Ok(d) => d,
            Err(err) => {
                tracing::warn!(error = %err, "falha ao criar ServiceDaemon mDNS; continuando sem mDNS");
                return None;
            }
        };

        // Desabilita todas as interfaces primeiro para nunca anunciar bridges Docker (docker0, br-*)
        if let Err(err) = daemon.disable_interface(IfKind::All) {
            tracing::warn!(error = %err, "falha ao desabilitar interfaces padrao no mDNS");
        }

        for &ip in announce_ips {
            if let Err(err) = daemon.enable_interface(IfKind::Addr(ip)) {
                tracing::warn!(ip = %ip, error = %err, "falha ao habilitar IP no mDNS");
            }
        }

        for iface in announce_interfaces {
            if let Err(err) = daemon.enable_interface(iface.as_str()) {
                tracing::warn!(interface = %iface, error = %err, "falha ao habilitar interface no mDNS");
            }
        }

        let sanitized_instance: String = server_name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let instance_name = if sanitized_instance.is_empty() {
            "bookferry".to_string()
        } else {
            sanitized_instance
        };
        let host_name = format!("{instance_name}.local.");

        let port_str = port.to_string();
        let properties = [
            ("v", API_VERSION),
            ("fp", spki_sha256),
            ("cert_fp", cert_sha256),
            ("port", port_str.as_str()),
        ];

        let ip_strings: Vec<String> = announce_ips.iter().map(ToString::to_string).collect();
        let ip_str_refs: Vec<&str> = ip_strings.iter().map(String::as_str).collect();

        let service_info = match ServiceInfo::new(
            MDNS_SERVICE_TYPE,
            &instance_name,
            &host_name,
            ip_str_refs.as_slice(),
            port,
            &properties[..],
        ) {
            Ok(info) => info,
            Err(err) => {
                tracing::warn!(error = %err, "falha ao construir ServiceInfo mDNS; continuando sem mDNS");
                let _ = daemon.shutdown();
                return None;
            }
        };

        let fullname = service_info.get_fullname().to_string();
        if let Err(err) = daemon.register(service_info) {
            tracing::warn!(error = %err, "falha ao registrar servico mDNS; continuando sem mDNS");
            let _ = daemon.shutdown();
            return None;
        }

        tracing::info!(
            service = %fullname,
            port = port,
            ips = ?announce_ips,
            interfaces = ?announce_interfaces,
            "anuncio mDNS _bookferry._tcp ativo"
        );

        Some(Self { daemon, fullname })
    }

    pub fn shutdown(self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

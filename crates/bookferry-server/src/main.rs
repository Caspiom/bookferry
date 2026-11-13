#![forbid(unsafe_code)]

use bookferry_server::{start, Config};
use std::env;
use std::net::IpAddr;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let mut data_dir = env::var_os("BOOKFERRY_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("./bookferry-data"));
    let mut vault_dir = env::var_os("BOOKFERRY_VAULT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("./bookferry-vault"));
    let mut bind_addr: IpAddr = env::var("BOOKFERRY_BIND")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| "127.0.0.1".parse().expect("valid loopback ip"));
    let mut port: u16 = env::var("BOOKFERRY_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(47600);
    let mut announce_ips: Vec<IpAddr> = Vec::new();
    let mut announce_interfaces: Vec<String> = Vec::new();
    let mut enable_mdns = false;
    let mut server_name = env::var("BOOKFERRY_NAME").unwrap_or_else(|_| "bookferry-pc".to_string());

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data-dir" => {
                if let Some(val) = args.next() {
                    data_dir = PathBuf::from(val);
                }
            }
            "--vault-dir" => {
                if let Some(val) = args.next() {
                    vault_dir = PathBuf::from(val);
                }
            }
            "--bind" => {
                if let Some(val) = args.next() {
                    bind_addr = val.parse()?;
                }
            }
            "--port" => {
                if let Some(val) = args.next() {
                    port = val.parse()?;
                }
            }
            "--announce-ip" => {
                if let Some(val) = args.next() {
                    announce_ips.push(val.parse()?);
                    enable_mdns = true;
                }
            }
            "--interface" => {
                if let Some(val) = args.next() {
                    announce_interfaces.push(val);
                    enable_mdns = true;
                }
            }
            "--name" => {
                if let Some(val) = args.next() {
                    server_name = val;
                }
            }
            "--enable-mdns" => {
                enable_mdns = true;
            }
            _ => {}
        }
    }

    std::fs::create_dir_all(&data_dir)?;
    std::fs::create_dir_all(&vault_dir)?;

    let mut config = Config::new(data_dir, vault_dir);
    config.bind_addr = bind_addr;
    config.port = port;
    config.announce_ips = announce_ips;
    config.announce_interfaces = announce_interfaces;
    config.enable_mdns = enable_mdns;
    config.server_name = server_name;

    let handle = start(config).await?;
    let qr_json = serde_json::to_string(&handle.pairing_payload())?;
    tracing::info!(
        listen_addr = %handle.local_addr(),
        cert_sha256 = %handle.cert_sha256(),
        spki_sha256 = %handle.spki_sha256(),
        "bookferry-serverd iniciado"
    );
    println!("{qr_json}");

    tokio::signal::ctrl_c().await?;
    tracing::info!("sinal de encerramento recebido; desligando bookferry-serverd");
    handle.shutdown().await?;
    Ok(())
}

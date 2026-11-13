use bookferry_server::{
    client_tls_request, start_lan_server, unix_ms_now, PairingQrPayload, PositionPayload,
    DEFAULT_LAN_PORT,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("serve");

    let spike_root = PathBuf::from("/home/gappy/Documents/projetos/bookshelf/spike");
    let fixtures_dir = spike_root.join("fixtures");
    let runtime_dir = spike_root.join("server");

    match cmd {
        "serve" => {
            let lan_ip = std::env::var("BOOKFERRY_LAN_IP").unwrap_or_else(|_| "192.168.1.2".to_string());
            let port = std::env::var("BOOKFERRY_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_LAN_PORT);

            let (_state, _mdns) = start_lan_server(&lan_ip, port, fixtures_dir, Some(&runtime_dir))
                .expect("Falha ao iniciar servidor Bookferry LAN");

            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        "put-position" => {
            // Uso: bookferry-server put-position <book_id> <page> <percent> [hlc] [device]
            let book_id = args.get(2).map(|s| s.as_str()).unwrap_or("little-nemo");
            let page: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(50);
            let percent: f64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(11.88);
            let now_ms = unix_ms_now();
            let device = args.get(6).map(|s| s.as_str()).unwrap_or("pc-linux");
            let hlc = args
                .get(5)
                .cloned()
                .unwrap_or_else(|| format!("{now_ms}:000000:{device}"));

            let qr_str = std::fs::read_to_string(runtime_dir.join("runtime-pairing.json"))
                .expect("Falha ao ler runtime-pairing.json");
            let qr: PairingQrPayload = serde_json::from_str(&qr_str).expect("JSON invalido");

            let payload = PositionPayload {
                device: device.to_string(),
                hlc,
                locator: format!("page:{page}"),
                percent,
                page: Some(page),
            };
            let body_bytes = serde_json::to_vec(&payload).unwrap();

            let t0 = Instant::now();
            let (status, _hdrs, resp_body, tcp_ms, hs_ms, req_ms) = client_tls_request(
                &qr.ip,
                qr.port,
                &qr.cert_sha256,
                "PUT",
                &format!("/position/{book_id}"),
                Some("bf_cli_local_pc_token"),
                Some(&body_bytes),
            )
            .expect("Falha no PUT /position");
            let total_ms = t0.elapsed().as_secs_f64() * 1000.0;

            println!(
                "[CLI_PUT_RESULT] status={status} tcp_ms={tcp_ms:.3} hs_ms={hs_ms:.3} req_ms={req_ms:.3} total_ms={total_ms:.3}"
            );
            println!("{}", String::from_utf8_lossy(&resp_body));
        }
        "status" => {
            let qr_str = std::fs::read_to_string(runtime_dir.join("runtime-pairing.json"))
                .expect("Falha ao ler runtime-pairing.json");
            let qr: PairingQrPayload = serde_json::from_str(&qr_str).expect("JSON invalido");
            let (status, _hdrs, resp_body, _, _, _) = client_tls_request(
                &qr.ip,
                qr.port,
                &qr.cert_sha256,
                "GET",
                "/status",
                Some("bf_cli_local_pc_token"),
                None,
            )
            .expect("Falha no GET /status");
            println!("HTTP {status}");
            println!("{}", String::from_utf8_lossy(&resp_body));
        }
        other => {
            eprintln!("Comando desconhecido: {other}");
            std::process::exit(1);
        }
    }
}

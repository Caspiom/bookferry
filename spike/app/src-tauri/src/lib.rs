#[tauri::command]
fn ping(name: String) -> String {
    format!(
        "pong, {} (rust {} / {})",
        name,
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    )
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![ping])
        .run(tauri::generate_context!())
        .expect("erro ao iniciar o app Tauri");
}

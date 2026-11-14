#![forbid(unsafe_code)]

pub mod commands;
pub mod protocol;
pub mod reader_prefs;

use crate::commands::{AppSettings, AppState};
use crate::protocol::ProtocolHandler;
use bookferry_client::{Client, ClientConfig, ClientEvent};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex as StdMutex};
use tauri::{Emitter, Manager};
#[cfg(any(not(target_os = "android"), bookferry_e2e))]
use tokio::sync::Mutex as AsyncMutex;

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

fn resolve_app_data_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    #[cfg(bookferry_e2e)]
    if std::env::var("BOOKFERRY_E2E").as_deref() == Ok("1") {
        if let Ok(custom_dir) = std::env::var("BOOKFERRY_E2E_DATA_DIR") {
            let p = PathBuf::from(custom_dir);
            std::fs::create_dir_all(&p)
                .map_err(|e| format!("falha ao criar BOOKFERRY_E2E_DATA_DIR: {e}"))?;
            return Ok(p);
        }
    }

    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("falha ao resolver app_data_dir do Tauri: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("falha ao criar app_data_dir: {e}"))?;
    Ok(dir)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    let builder = tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("bookferry", |ctx, request, responder| {
            let Some(state) = ctx.app_handle().try_state::<Arc<AppState>>() else {
                let resp = tauri::http::Response::builder()
                    .status(tauri::http::StatusCode::SERVICE_UNAVAILABLE)
                    .body(b"AppState indisponivel".to_vec())
                    .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()));
                responder.respond(resp);
                return;
            };
            let protocol = state.protocol.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let response = protocol.handle_request(&request);
                responder.respond(response);
            });
        })
        .setup(|app| {
            let app_handle = app.handle().clone();
            let data_dir = resolve_app_data_dir(&app_handle)?;
            let books_dir = data_dir.join("books");
            std::fs::create_dir_all(&books_dir)?;

            #[cfg_attr(not(bookferry_e2e), allow(unused_mut))]
            let mut settings = AppSettings::load(&data_dir);

            #[cfg(bookferry_e2e)]
            if std::env::var("BOOKFERRY_E2E").as_deref() == Ok("1") {
                if let Ok(e2e_vault) = std::env::var("BOOKFERRY_E2E_VAULT") {
                    if !e2e_vault.trim().is_empty() {
                        settings.vault_dir = Some(e2e_vault.trim().to_string());
                        settings.client_only_mode = false;
                        let _ = settings.save(&data_dir);
                    }
                }
            }

            let device_name = if cfg!(target_os = "android") {
                "Bookferry Android"
            } else {
                "Bookferry Desktop"
            };
            let mut client_cfg = ClientConfig::new(&data_dir, device_name);
            client_cfg.enable_mdns = !cfg!(bookferry_e2e);

            let client = tauri::async_runtime::block_on(Client::open(client_cfg))
                .map_err(|e| format!("falha ao abrir bookferry-client: {e}"))?;

            let protocol = ProtocolHandler::new(&books_dir).with_covers_dir(data_dir.join("covers"));

            let state = Arc::new(AppState {
                data_dir: data_dir.clone(),
                client: client.clone(),
                protocol,
                connected: AtomicBool::new(false),
                settings: StdMutex::new(settings.clone()),
                active_downloads: StdMutex::new(HashMap::new()),
                reader_prefs_lock: StdMutex::new(()),
                #[cfg(not(target_os = "android"))]
                desktop_server: AsyncMutex::new(None),
                #[cfg(bookferry_e2e)]
                e2e_peer: AsyncMutex::new(None),
            });

            // Encaminha todos os `ClientEvent` para o WebView via `AppHandle::emit`
            let mut events_rx = client.subscribe();
            let emit_handle = app_handle.clone();
            let state_for_events = state.clone();
            tauri::async_runtime::spawn(async move {
                while let Ok(ev) = events_rx.recv().await {
                    match &ev {
                        ClientEvent::Connected { .. } => {
                            state_for_events
                                .connected
                                .store(true, AtomicOrdering::SeqCst);
                        }
                        ClientEvent::Disconnected { .. } | ClientEvent::Unpaired { .. } => {
                            state_for_events
                                .connected
                                .store(false, AtomicOrdering::SeqCst);
                        }
                        _ => {}
                    }
                    let _ = emit_handle.emit("bookferry-event", &ev);
                }
            });

            // No Desktop, se ja houver um `vault_dir` configurado, inicia o servidor embutido
            // e auto-pareia em loopback imediatamente.
            #[cfg(not(target_os = "android"))]
            if !settings.client_only_mode {
                if let Some(ref vault_str) = settings.vault_dir {
                    let vault_path = PathBuf::from(vault_str);
                    if vault_path.is_dir() {
                        if let Err(err) = tauri::async_runtime::block_on(
                            crate::commands::start_or_restart_desktop_server(&state, vault_path),
                        ) {
                            tracing::warn!(error = %err, "nao foi possivel iniciar servidor desktop na abertura");
                        }
                    }
                }
            }

            app.manage(state);
            Ok(())
        });

    #[cfg(not(bookferry_e2e))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        commands::status,
        commands::pair,
        commands::unpair,
        commands::library,
        commands::rescan,
        commands::download_book,
        commands::cancel_download,
        commands::remove_download,
        commands::set_position,
        commands::get_position,
        commands::decide_book_resume,
        commands::on_resume,
        commands::cbz_info,
        commands::save_settings,
        commands::configure_server_vault,
        commands::rotate_pairing_qr,
        commands::list_paired_devices,
        commands::revoke_paired_device,
        commands::get_reader_prefs,
        commands::save_reader_prefs,
        commands::reset_reader_prefs,
        commands::set_default_reader_prefs,
        commands::save_reader_preset,
        commands::delete_reader_preset,
        commands::save_progress_prefs,
        commands::set_fullscreen,
        commands::is_fullscreen,
        commands::set_display_title,
        commands::set_continue_hidden,
        commands::set_book_cover,
        commands::remove_book_cover,
        commands::ensure_cover,
        commands::list_books_without_cover,
    ]);

    #[cfg(bookferry_e2e)]
    let builder = builder.invoke_handler(tauri::generate_handler![
        commands::status,
        commands::pair,
        commands::unpair,
        commands::library,
        commands::rescan,
        commands::download_book,
        commands::cancel_download,
        commands::remove_download,
        commands::set_position,
        commands::get_position,
        commands::decide_book_resume,
        commands::on_resume,
        commands::cbz_info,
        commands::save_settings,
        commands::configure_server_vault,
        commands::rotate_pairing_qr,
        commands::list_paired_devices,
        commands::revoke_paired_device,
        commands::get_reader_prefs,
        commands::save_reader_prefs,
        commands::reset_reader_prefs,
        commands::set_default_reader_prefs,
        commands::save_reader_preset,
        commands::delete_reader_preset,
        commands::save_progress_prefs,
        commands::set_fullscreen,
        commands::is_fullscreen,
        commands::set_display_title,
        commands::set_continue_hidden,
        commands::set_book_cover,
        commands::remove_book_cover,
        commands::ensure_cover,
        commands::list_books_without_cover,
        commands::e2e_pair_second_client,
        commands::e2e_wait_peer_position,
        commands::e2e_peer_set_position,
        commands::e2e_capture_screenshot,
        commands::e2e_finish,
    ]);

    if let Err(err) = builder.run(tauri::generate_context!()) {
        tracing::error!(error = %err, "erro fatal ao executar aplicacao Tauri Bookferry");
    }
}

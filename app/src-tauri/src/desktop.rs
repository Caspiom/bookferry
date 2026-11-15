//! Ciclo de vida do app no desktop: roda em segundo plano com ícone na bandeja, abre sem janela
//! quando iniciado com o computador (`--background`) e mantém o servidor embutido ligado mesmo
//! com a janela fechada. Nada aqui existe no Android (o celular é só cliente).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

/// Rótulo da janela principal (o mesmo de `tauri.conf.json`).
pub const MAIN_WINDOW: &str = "main";

const TRAY_ID: &str = "bookferry-tray";
const MENU_OPEN: &str = "open";
const MENU_QUIT: &str = "quit";

/// Estado da bandeja, gerenciado pelo Tauri.
#[derive(Default)]
pub struct TrayState {
    /// `true` só depois que o ícone foi criado. Sem ele, fechar a janela encerra o app: esconder
    /// a janela sem ícone deixaria o servidor rodando sem nenhum jeito de voltar a ela.
    pub active: AtomicBool,
    status_item: Mutex<Option<MenuItem<Wry>>>,
}

/// O app foi iniciado para ficar em segundo plano (início com o computador)?
pub fn wants_background<S: AsRef<str>>(args: &[S]) -> bool {
    args.iter()
        .any(|a| matches!(a.as_ref(), "--background" | "--hidden"))
}

/// Decide o que fazer ao clicar no X da janela: esconder (segue na bandeja) ou encerrar.
pub fn should_hide_on_close(close_to_tray: bool, tray_active: bool) -> bool {
    close_to_tray && tray_active
}

/// Linha de status do menu da bandeja.
pub fn server_status_text(port: Option<u16>) -> String {
    match port {
        Some(p) => format!("Servidor ativo (porta {p})"),
        None => "Servidor desligado: escolha a pasta de livros".to_string(),
    }
}

/// Mostra e foca a janela principal (recria o foco mesmo se estava minimizada ou escondida).
pub fn show_main_window(app: &AppHandle) {
    match app.get_webview_window(MAIN_WINDOW) {
        Some(win) => {
            let _ = win.unminimize();
            if let Err(err) = win.show() {
                tracing::warn!(error = %err, "falha ao mostrar a janela principal");
            }
            let _ = win.set_focus();
        }
        None => tracing::warn!("janela principal nao encontrada ao tentar mostrar"),
    }
}

/// Cria o ícone da bandeja. Falhar aqui não derruba o app: sem ícone, `TrayState::active`
/// fica `false` e fechar a janela volta a encerrar o programa.
pub fn setup_tray(app: &AppHandle, server_port: Option<u16>) {
    let state = app.state::<TrayState>();
    match build_tray(app, server_port) {
        Ok(status_item) => {
            *state.status_item.lock().unwrap_or_else(|e| e.into_inner()) = Some(status_item);
            state.active.store(true, Ordering::SeqCst);
        }
        Err(err) => {
            tracing::warn!(error = %err, "nao foi possivel criar o icone da bandeja; fechar a janela encerra o app");
        }
    }
}

fn build_tray(app: &AppHandle, server_port: Option<u16>) -> tauri::Result<MenuItem<Wry>> {
    let status_item = MenuItem::with_id(
        app,
        "status",
        server_status_text(server_port),
        false,
        None::<&str>,
    )?;
    let open_item = MenuItem::with_id(app, MENU_OPEN, "Abrir Bookferry", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, MENU_QUIT, "Sair", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status_item,
            &PredefinedMenuItem::separator(app)?,
            &open_item,
            &quit_item,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Bookferry")
        .menu(&menu)
        // O clique esquerdo abre a janela (Windows/macOS); no Linux o menu é o único caminho.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_OPEN => show_main_window(app),
            MENU_QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(status_item)
}

/// Atualiza a linha de status do menu (ex.: depois de configurar a pasta de livros).
pub fn refresh_tray_status(app: &AppHandle, server_port: Option<u16>) {
    let state = app.state::<TrayState>();
    let guard = state.status_item.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(item) = guard.as_ref() {
        let _ = item.set_text(server_status_text(server_port));
    }
}

// ---------------------------------------------------------------------------
// Iniciar com o computador
// ---------------------------------------------------------------------------

/// Nome do arquivo em `~/.config/autostart/` (o mesmo identificador do app).
pub const AUTOSTART_FILE: &str = "dev.gaspari.bookferry.desktop";

/// Onde fica a entrada de autostart do XDG: `$XDG_CONFIG_HOME/autostart` ou `~/.config/autostart`.
pub fn autostart_file_path(xdg_config_home: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    let base = match xdg_config_home {
        Some(dir) if dir.is_absolute() => dir.to_path_buf(),
        _ => home?.join(".config"),
    };
    Some(base.join("autostart").join(AUTOSTART_FILE))
}

/// Comando gravado no `Exec=`. No NixOS o executável real fica no store e muda a cada rebuild
/// (e o binário embrulhado perde o ambiente do webview), então usa o nome `bookferry`, resolvido
/// pelo PATH da sessão. No AppImage o caminho do próprio arquivo é o único que vale.
pub fn autostart_exec(appimage: Option<&str>) -> String {
    let program = match appimage {
        Some(path) if !path.is_empty() => quote_exec_arg(path),
        _ => "bookferry".to_string(),
    };
    format!("{program} --background")
}

/// Aspas do `Exec=` da especificação Desktop Entry. Caminho com caractere que exigiria escape
/// duplo (aspas, barra invertida, `$`, crase) é recusado e cai no nome simples.
fn quote_exec_arg(path: &str) -> String {
    if path.contains(['"', '\\', '$', '`', '\n']) {
        return "bookferry".to_string();
    }
    if path.contains(' ') {
        format!("\"{path}\"")
    } else {
        path.to_string()
    }
}

/// Conteúdo do `.desktop` de autostart.
pub fn autostart_entry(exec: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Bookferry\n\
         Comment=Servidor de sincronizacao do Bookferry em segundo plano\n\
         Exec={exec}\n\
         Icon=bookferry\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n"
    )
}

/// O sistema oferece "iniciar com o computador" por aqui? (Hoje só no Linux; Windows e macOS
/// ainda não têm esta parte implementada.)
pub const fn autostart_supported() -> bool {
    cfg!(target_os = "linux")
}

#[cfg(target_os = "linux")]
fn current_autostart_path() -> Result<PathBuf, String> {
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    autostart_file_path(xdg.as_deref(), home.as_deref())
        .ok_or_else(|| "nao foi possivel descobrir a pasta de configuracao do usuario".to_string())
}

/// Início com o computador está ligado?
pub fn autostart_enabled() -> bool {
    #[cfg(target_os = "linux")]
    {
        current_autostart_path().is_ok_and(|p| p.is_file())
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Liga ou desliga o início com o computador.
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let path = current_autostart_path()?;
        if enabled {
            let dir = path.parent().ok_or("caminho de autostart invalido")?;
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("erro ao criar a pasta de autostart: {e}"))?;
            let appimage = std::env::var("APPIMAGE").ok();
            let entry = autostart_entry(&autostart_exec(appimage.as_deref()));
            std::fs::write(&path, entry).map_err(|e| format!("erro ao gravar o autostart: {e}"))
        } else {
            match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("erro ao remover o autostart: {e}")),
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = enabled;
        Err("Iniciar com o computador ainda so esta disponivel no Linux".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_so_com_a_flag_exata() {
        assert!(wants_background(&["bookferry", "--background"]));
        assert!(wants_background(&["bookferry", "--hidden"]));
        assert!(!wants_background(&["bookferry"]));
        // nome parecido nao conta (ex.: arquivo aberto com o app)
        assert!(!wants_background(&[
            "bookferry",
            "--background-x",
            "livro.epub"
        ]));
    }

    #[test]
    fn fechar_so_esconde_com_bandeja_viva_e_preferencia_ligada() {
        assert!(should_hide_on_close(true, true));
        // sem icone, esconder deixaria o app inalcancavel
        assert!(!should_hide_on_close(true, false));
        assert!(!should_hide_on_close(false, true));
        assert!(!should_hide_on_close(false, false));
    }

    #[test]
    fn status_mostra_a_porta_ou_o_motivo() {
        assert_eq!(
            server_status_text(Some(47600)),
            "Servidor ativo (porta 47600)"
        );
        assert!(server_status_text(None).contains("desligado"));
    }

    #[test]
    fn autostart_usa_xdg_config_home_quando_absoluto() {
        let p = autostart_file_path(Some(Path::new("/cfg")), Some(Path::new("/home/u"))).unwrap();
        assert_eq!(p, Path::new("/cfg/autostart/dev.gaspari.bookferry.desktop"));
    }

    #[test]
    fn autostart_cai_em_home_config_se_xdg_ausente_ou_relativo() {
        let esperado = Path::new("/home/u/.config/autostart/dev.gaspari.bookferry.desktop");
        assert_eq!(
            autostart_file_path(None, Some(Path::new("/home/u"))).unwrap(),
            esperado
        );
        // a especificacao XDG manda ignorar caminho relativo
        assert_eq!(
            autostart_file_path(Some(Path::new("relativo")), Some(Path::new("/home/u"))).unwrap(),
            esperado
        );
        assert!(autostart_file_path(None, None).is_none());
    }

    #[test]
    fn exec_padrao_e_o_nome_no_path() {
        assert_eq!(autostart_exec(None), "bookferry --background");
        assert_eq!(autostart_exec(Some("")), "bookferry --background");
    }

    #[test]
    fn exec_de_appimage_usa_o_caminho_com_aspas_se_tiver_espaco() {
        assert_eq!(
            autostart_exec(Some("/opt/Bookferry.AppImage")),
            "/opt/Bookferry.AppImage --background"
        );
        assert_eq!(
            autostart_exec(Some("/home/u/Meus Apps/Bookferry.AppImage")),
            "\"/home/u/Meus Apps/Bookferry.AppImage\" --background"
        );
    }

    #[test]
    fn exec_recusa_caminho_que_exigiria_escape_duplo() {
        for ruim in ["/a\"b", "/a\\b", "/a$b", "/a`b", "/a\nb"] {
            assert_eq!(
                autostart_exec(Some(ruim)),
                "bookferry --background",
                "{ruim:?}"
            );
        }
    }

    #[test]
    fn entrada_de_autostart_e_um_desktop_entry_valido() {
        let e = autostart_entry("bookferry --background");
        assert!(e.starts_with("[Desktop Entry]\n"));
        for linha in [
            "Type=Application",
            "Exec=bookferry --background",
            "Terminal=false",
            "X-GNOME-Autostart-enabled=true",
        ] {
            assert!(e.lines().any(|l| l == linha), "falta {linha}");
        }
        assert!(e.ends_with('\n'));
    }
}

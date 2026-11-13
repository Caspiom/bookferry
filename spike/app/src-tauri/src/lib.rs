use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::FileExt;
use std::os::unix::io::FromRawFd;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

static REGISTERED_FILES: OnceLock<Mutex<HashMap<String, Arc<File>>>> = OnceLock::new();
static SERVER_PORT: OnceLock<u16> = OnceLock::new();

fn files_map() -> &'static Mutex<HashMap<String, Arc<File>>> {
    REGISTERED_FILES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ia = a.chars().peekable();
    let mut ib = b.chars().peekable();

    loop {
        match (ia.peek(), ib.peek()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(&ca), Some(&cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let mut sa = String::new();
                    let mut sb = String::new();
                    while let Some(&c) = ia.peek() {
                        if c.is_ascii_digit() {
                            sa.push(c);
                            ia.next();
                        } else {
                            break;
                        }
                    }
                    while let Some(&c) = ib.peek() {
                        if c.is_ascii_digit() {
                            sb.push(c);
                            ib.next();
                        } else {
                            break;
                        }
                    }
                    let sa_trim = sa.trim_start_matches('0');
                    let sb_trim = sb.trim_start_matches('0');
                    match sa_trim.len().cmp(&sb_trim.len()) {
                        Ordering::Equal => match sa_trim.cmp(sb_trim) {
                            Ordering::Equal => match sa.len().cmp(&sb.len()) {
                                Ordering::Equal => continue,
                                other => return other,
                            },
                            other => return other,
                        },
                        other => return other,
                    }
                } else {
                    let la = ca.to_ascii_lowercase();
                    let lb = cb.to_ascii_lowercase();
                    if la != lb {
                        return la.cmp(&lb);
                    }
                    ia.next();
                    ib.next();
                }
            }
        }
    }
}

fn is_valid_comic_page(entry_name: &str) -> bool {
    if entry_name.ends_with('/') {
        return false;
    }
    if entry_name.starts_with("__MACOSX/") || entry_name.contains("/__MACOSX/") {
        return false;
    }
    let base = entry_name.rsplit('/').next().unwrap_or(entry_name);
    if base.starts_with('.') {
        return false;
    }
    let lower = base.to_ascii_lowercase();
    lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".png")
        || lower.ends_with(".webp")
        || lower.ends_with(".gif")
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProcMemStatus {
    pub vm_rss_kb: u64,
    pub vm_hwm_kb: u64,
    pub vm_size_kb: u64,
}

fn read_proc_mem() -> ProcMemStatus {
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

#[derive(Serialize)]
pub struct SafCbzResult {
    pub file_name: String,
    pub file_size_bytes: u64,
    pub total_zip_entries: usize,
    pub ignored_macosx_entries: usize,
    pub valid_page_count: usize,
    pub first_5_pages: Vec<String>,
    pub last_5_pages: Vec<String>,
    pub target_page_index: usize,
    pub target_page_name: String,
    pub target_page_bytes: usize,
    pub zip_open_ms: f64,
    pub list_natural_sort_ms: f64,
    pub read_single_page_ms: f64,
    pub base64_encode_ms: f64,
    pub total_rust_ms: f64,
    pub copied_to_cache: bool,
    pub mem_before: ProcMemStatus,
    pub mem_after: ProcMemStatus,
    pub data_url: String,
}

#[tauri::command]
fn ping(name: String) -> String {
    format!(
        "pong, {} (rust {} / {})",
        name,
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    )
}

#[tauri::command]
fn saf_inspect_and_read_cbz(
    fd: i32,
    name: String,
    page_index: Option<usize>,
    target_name: Option<String>,
) -> Result<SafCbzResult, String> {
    let mem_before = read_proc_mem();
    let t_total = Instant::now();

    let file = unsafe { File::from_raw_fd(fd) };
    let file_size_bytes = file.metadata().map(|m| m.len()).unwrap_or(0);

    let t_zip = Instant::now();
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("ZipArchive::new erro: {e}"))?;
    let zip_open_ms = t_zip.elapsed().as_secs_f64() * 1000.0;

    let total_zip_entries = archive.len();
    let mut pages: Vec<String> = Vec::with_capacity(total_zip_entries);
    let mut ignored_macosx_entries = 0usize;

    for raw_name in archive.file_names() {
        if is_valid_comic_page(raw_name) {
            pages.push(raw_name.to_string());
        } else {
            ignored_macosx_entries += 1;
        }
    }

    pages.sort_by(|a, b| natural_cmp(a, b));
    let list_natural_sort_ms = t_total.elapsed().as_secs_f64() * 1000.0;

    if pages.is_empty() {
        return Err("Nenhuma pagina valida encontrada no CBZ".to_string());
    }

    let chosen_idx = if let Some(ref req_name) = target_name {
        pages.iter().position(|p| p == req_name || p.ends_with(req_name)).unwrap_or(0)
    } else {
        page_index.unwrap_or(0).min(pages.len() - 1)
    };
    let chosen_name = pages[chosen_idx].clone();

    let t_read = Instant::now();
    let mut buf = Vec::new();
    {
        let mut entry = archive
            .by_name(&chosen_name)
            .map_err(|e| format!("by_name({chosen_name}) erro: {e}"))?;
        buf.reserve_exact(entry.size() as usize);
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("read_to_end erro: {e}"))?;
    }
    let read_single_page_ms = t_read.elapsed().as_secs_f64() * 1000.0;

    let t_b64 = Instant::now();
    let b64 = BASE64.encode(&buf);
    let data_url = format!("data:image/jpeg;base64,{b64}");
    let base64_encode_ms = t_b64.elapsed().as_secs_f64() * 1000.0;

    let total_rust_ms = t_total.elapsed().as_secs_f64() * 1000.0;

    let inner_file = archive.into_inner();
    files_map()
        .lock()
        .unwrap()
        .insert(name.clone(), Arc::new(inner_file));

    let mem_after = read_proc_mem();

    let first_5_pages = pages.iter().take(5).cloned().collect();
    let last_5_pages = pages
        .iter()
        .rev()
        .take(5)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    Ok(SafCbzResult {
        file_name: name,
        file_size_bytes,
        total_zip_entries,
        ignored_macosx_entries,
        valid_page_count: pages.len(),
        first_5_pages,
        last_5_pages,
        target_page_index: chosen_idx,
        target_page_name: chosen_name,
        target_page_bytes: buf.len(),
        zip_open_ms,
        list_natural_sort_ms,
        read_single_page_ms,
        base64_encode_ms,
        total_rust_ms,
        copied_to_cache: false,
        mem_before,
        mem_after,
        data_url,
    })
}

#[derive(Serialize)]
pub struct RegisteredFileInfo {
    pub name: String,
    pub size: u64,
    pub http_url: String,
}

#[tauri::command]
fn register_saf_file(fd: i32, name: String) -> Result<RegisteredFileInfo, String> {
    let file = unsafe { File::from_raw_fd(fd) };
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    files_map()
        .lock()
        .unwrap()
        .insert(name.clone(), Arc::new(file));
    let port = *SERVER_PORT.get().unwrap_or(&0);
    Ok(RegisteredFileInfo {
        name: name.clone(),
        size,
        http_url: format!("http://127.0.0.1:{port}/file/{name}"),
    })
}

#[tauri::command]
fn try_open_direct_path(path: String, register_as: Option<String>) -> Result<RegisteredFileInfo, String> {
    let file = File::open(&path).map_err(|e| format!("File::open({path}) falhou: {e} (os error {:?})", e.raw_os_error()))?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    let name = register_as.unwrap_or_else(|| {
        path.rsplit('/').next().unwrap_or("file").to_string()
    });
    files_map()
        .lock()
        .unwrap()
        .insert(name.clone(), Arc::new(file));
    let port = *SERVER_PORT.get().unwrap_or(&0);
    Ok(RegisteredFileInfo {
        name: name.clone(),
        size,
        http_url: format!("http://127.0.0.1:{port}/file/{name}"),
    })
}

#[derive(Serialize)]
pub struct EnvInfo {
    pub os: String,
    pub arch: String,
    pub server_port: u16,
    pub autorun: Option<String>,
    pub out_file: Option<String>,
}

#[tauri::command]
fn get_env_info() -> EnvInfo {
    EnvInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        server_port: *SERVER_PORT.get().unwrap_or(&0),
        autorun: std::env::var("BOOKFERRY_AUTORUN").ok(),
        out_file: std::env::var("BOOKFERRY_OUT").ok(),
    }
}

#[tauri::command]
fn log_result(tag: String, payload: String) {
    println!("[BOOKFERRY_RESULT][{}] {}", tag, payload);
    if let Ok(out_path) = std::env::var("BOOKFERRY_OUT") {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(out_path)
        {
            let _ = writeln!(f, "{}", payload);
        }
    }
    if tag == "AUTORUN_DONE" && std::env::var("BOOKFERRY_AUTORUN").is_ok() {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(300));
            std::process::exit(0);
        });
    }
}

fn resolve_file(name: &str) -> Option<Arc<File>> {
    if let Some(f) = files_map().lock().unwrap().get(name).cloned() {
        return Some(f);
    }
    // Fallback candidates on Linux or Android if accessible
    let candidates = [
        PathBuf::from(format!("../../fixtures/{name}")),
        PathBuf::from(format!("../fixtures/{name}")),
        PathBuf::from(format!("fixtures/{name}")),
        PathBuf::from(format!("/home/gappy/Documents/projetos/bookshelf/spike/fixtures/{name}")),
        PathBuf::from(format!("/sdcard/Download/bookferry-fixtures/{name}")),
    ];
    for p in candidates {
        if let Ok(f) = File::open(&p) {
            let arc = Arc::new(f);
            files_map()
                .lock()
                .unwrap()
                .insert(name.to_string(), arc.clone());
            return Some(arc);
        }
    }
    None
}

fn guess_mime(name: &str) -> &'static str {
    if name.ends_with(".pdf") {
        "application/pdf"
    } else if name.ends_with(".epub") {
        "application/epub+zip"
    } else if name.ends_with(".cbz") {
        "application/vnd.comicbook+zip"
    } else if name.ends_with(".jpeg") || name.ends_with(".jpg") {
        "image/jpeg"
    } else if name.ends_with(".json") {
        "application/json"
    } else {
        "application/octet-stream"
    }
}

fn handle_http_client(mut stream: TcpStream) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return;
    }
    let method = parts[0];
    let raw_path = parts[1];

    let mut range_header: Option<String> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if trimmed.len() > 6 && trimmed[..6].eq_ignore_ascii_case("range:") {
            range_header = Some(trimmed[6..].trim().to_string());
        }
    }

    if method == "OPTIONS" {
        let resp = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, HEAD, OPTIONS\r\nAccess-Control-Allow-Headers: *\r\nAccess-Control-Expose-Headers: Content-Range, Content-Length, Accept-Ranges\r\n\r\n";
        let _ = stream.write_all(resp.as_bytes());
        return;
    }

    let path = raw_path.split('?').next().unwrap_or(raw_path);
    if let Some(name) = path.strip_prefix("/file/") {
        if let Some(file) = resolve_file(name) {
            let total_len = match file.metadata() {
                Ok(m) => m.len(),
                Err(_) => return,
            };
            let mime = guess_mime(name);

            if method == "HEAD" {
                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {total_len}\r\nAccept-Ranges: bytes\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Expose-Headers: Content-Range, Content-Length, Accept-Ranges\r\n\r\n"
                );
                let _ = stream.write_all(hdr.as_bytes());
                return;
            }

            if let Some(r_str) = range_header.and_then(|s| s.strip_prefix("bytes=").map(|x| x.to_string())) {
                let mut bounds = r_str.split('-');
                let start: u64 = bounds.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                let end: u64 = bounds
                    .next()
                    .and_then(|s| if s.is_empty() { None } else { s.parse().ok() })
                    .unwrap_or_else(|| total_len.saturating_sub(1))
                    .min(total_len.saturating_sub(1));
                if start <= end && start < total_len {
                    let chunk_len = (end - start + 1) as usize;
                    let mut buf = vec![0u8; chunk_len];
                    let _ = file.read_at(&mut buf, start);
                    let hdr = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Type: {mime}\r\nContent-Length: {chunk_len}\r\nContent-Range: bytes {start}-{end}/{total_len}\r\nAccept-Ranges: bytes\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Expose-Headers: Content-Range, Content-Length, Accept-Ranges\r\n\r\n"
                    );
                    let _ = stream.write_all(hdr.as_bytes());
                    let _ = stream.write_all(&buf);
                    return;
                }
            }

            // Full file response in 256 KB chunks using read_at
            let hdr = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {total_len}\r\nAccept-Ranges: bytes\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Expose-Headers: Content-Range, Content-Length, Accept-Ranges\r\n\r\n"
            );
            if stream.write_all(hdr.as_bytes()).is_err() {
                return;
            }
            let mut offset = 0u64;
            let mut buf = vec![0u8; 256 * 1024];
            while offset < total_len {
                let to_read = ((total_len - offset) as usize).min(buf.len());
                match file.read_at(&mut buf[..to_read], offset) {
                    Ok(0) => break,
                    Ok(n) => {
                        if stream.write_all(&buf[..n]).is_err() {
                            break;
                        }
                        offset += n as u64;
                    }
                    Err(_) => break,
                }
            }
            return;
        }
    } else if let Some(rest) = path.strip_prefix("/cbz-page/") {
        // /cbz-page/<cbz_name>/<page_name>
        if let Some((cbz_name, page_name)) = rest.split_once('/') {
            if let Some(file) = resolve_file(cbz_name) {
                if let Ok(dup_file) = file.try_clone() {
                    let mut dup_file = dup_file;
                    let _ = dup_file.seek(SeekFrom::Start(0));
                    if let Ok(mut archive) = zip::ZipArchive::new(dup_file) {
                        if let Ok(mut entry) = archive.by_name(page_name) {
                            let mut buf = Vec::with_capacity(entry.size() as usize);
                            if entry.read_to_end(&mut buf).is_ok() {
                                let hdr = format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
                                    buf.len()
                                );
                                let _ = stream.write_all(hdr.as_bytes());
                                let _ = stream.write_all(&buf);
                                return;
                            }
                        }
                    }
                }
            }
        }
    }

    let not_found = "HTTP/1.1 404 Not Found\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 9\r\n\r\nNot Found";
    let _ = stream.write_all(not_found.as_bytes());
}

fn start_local_http_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:18421")
        .or_else(|_| TcpListener::bind("127.0.0.1:0"))
        .expect("Falha ao abrir TcpListener local");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || handle_http_client(stream));
        }
    });
    port
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let port = start_local_http_server();
    let _ = SERVER_PORT.set(port);

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            ping,
            saf_inspect_and_read_cbz,
            register_saf_file,
            try_open_direct_path,
            get_env_info,
            log_result
        ])
        .run(tauri::generate_context!())
        .expect("erro ao iniciar o app Tauri");
}

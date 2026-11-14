use bookferry_core::{BookFormat, BookId};
use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;
use tauri::http::{header, Method, Request, Response, StatusCode, Uri};

/// Limite padrao de tamanho descomprimido por entrada de imagem no CBZ (50 MiB),
/// prevenindo zip bombs de consumir a memoria do processo.
pub const DEFAULT_MAX_CBZ_ENTRY_BYTES: u64 = 50 * 1024 * 1024;

/// Limite maximo de um unico bloco servido via HTTP Range (64 MiB).
pub const MAX_RANGE_CHUNK_BYTES: u64 = 64 * 1024 * 1024;

/// Capacidade maxima de paginas CBZ descomprimidas retidas no cache em memoria.
pub const CBZ_PAGE_CACHE_MAX_ITEMS: usize = 16;

/// Metadados de uma pagina de imagem valida dentro de um arquivo CBZ.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CbzPageEntry {
    /// Indice original dentro do `ZipArchive` (usado com `by_index`, nunca caminho de disco).
    pub zip_index: usize,
    /// Nome normalizado da entrada dentro do ZIP (apenas para ordenacao natural).
    pub entry_name: String,
    /// `Content-Type` determinado pela extensao da imagem.
    pub content_type: &'static str,
    /// Tamanho descomprimido declarado no cabecalho ZIP.
    pub declared_size: u64,
}

#[derive(Clone, Debug)]
struct CachedCbzIndex {
    file_len: u64,
    mtime_ms: u64,
    pages: Arc<Vec<CbzPageEntry>>,
}

#[derive(Clone, Debug)]
struct CachedCbzPage {
    book_id: String,
    page_index: usize,
    file_len: u64,
    mtime_ms: u64,
    content_type: &'static str,
    bytes: Arc<Vec<u8>>,
}

/// Estado compartilhado do protocolo customizado `bookferry://` (e `http://bookferry.localhost`).
#[derive(Clone)]
pub struct ProtocolHandler {
    books_dir: PathBuf,
    /// Pasta das capas baixadas (`<book_id>-<versao>`); `None` desliga a rota `/covers/...`.
    covers_dir: Option<PathBuf>,
    max_cbz_entry_bytes: u64,
    cbz_index_cache: Arc<Mutex<HashMap<String, CachedCbzIndex>>>,
    cbz_page_cache: Arc<Mutex<VecDeque<CachedCbzPage>>>,
}

impl ProtocolHandler {
    pub fn new(books_dir: impl Into<PathBuf>) -> Self {
        Self::with_max_cbz_entry_bytes(books_dir, DEFAULT_MAX_CBZ_ENTRY_BYTES)
    }

    pub fn with_max_cbz_entry_bytes(
        books_dir: impl Into<PathBuf>,
        max_cbz_entry_bytes: u64,
    ) -> Self {
        Self {
            books_dir: books_dir.into(),
            covers_dir: None,
            max_cbz_entry_bytes: max_cbz_entry_bytes.max(1024),
            cbz_index_cache: Arc::new(Mutex::new(HashMap::new())),
            cbz_page_cache: Arc::new(Mutex::new(VecDeque::with_capacity(
                CBZ_PAGE_CACHE_MAX_ITEMS,
            ))),
        }
    }

    /// Habilita `GET /covers/<book_id>/<versao>` servindo `dir/<book_id>-<versao>`.
    pub fn with_covers_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.covers_dir = Some(dir.into());
        self
    }

    pub fn books_dir(&self) -> &Path {
        &self.books_dir
    }

    /// Inspeciona e retorna a quantidade de paginas validas de um CBZ ja baixado em `books_dir`.
    pub fn inspect_cbz_page_count(&self, raw_book_id: &str) -> Result<usize, String> {
        let book_id = BookId::parse(raw_book_id).map_err(|e| format!("book_id invalido: {e}"))?;
        let path = self
            .resolve_safe_book_file(&book_id)
            .ok_or_else(|| "caminho de livro invalido".to_string())?;
        if !path.is_file() {
            return Err("arquivo CBZ nao encontrado localmente".to_string());
        }
        let pages = self.get_or_index_cbz(&book_id, &path)?;
        Ok(pages.len())
    }

    /// Processa uma requisicao do protocolo customizado `bookferry://` ou `http://bookferry.localhost`.
    pub fn handle_request(&self, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
        let method = req.method();
        if method == Method::OPTIONS {
            return build_cors_response(StatusCode::NO_CONTENT, "text/plain", Vec::new(), &[]);
        }
        if method != Method::GET && method != Method::HEAD {
            return build_json_error(
                StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
                "Apenas GET, HEAD e OPTIONS sao permitidos",
                &[],
            );
        }

        let norm_path = normalize_protocol_path(req.uri());
        let trimmed = norm_path.trim_matches('/');
        let segments: Vec<&str> = if trimmed.is_empty() {
            Vec::new()
        } else {
            trimmed.split('/').collect()
        };

        match segments.as_slice() {
            ["books", raw_book_id] | ["book", raw_book_id] => {
                self.serve_book_file(raw_book_id, method == Method::HEAD, req.headers())
            }
            ["covers", raw_book_id, raw_version] => {
                self.serve_cover(raw_book_id, raw_version, method == Method::HEAD)
            }
            ["cbz", raw_book_id, "page", raw_index] => {
                self.serve_cbz_page(raw_book_id, raw_index, method == Method::HEAD)
            }
            _ => build_json_error(
                StatusCode::NOT_FOUND,
                "route_not_found",
                "Rota do protocolo bookferry nao encontrada",
                &[],
            ),
        }
    }

    fn resolve_safe_book_file(&self, book_id: &BookId) -> Option<PathBuf> {
        let rel = Path::new(book_id.as_str());
        let mut comps = rel.components();
        match (comps.next(), comps.next()) {
            (Some(Component::Normal(_)), None) => Some(self.books_dir.join(book_id.as_str())),
            _ => None,
        }
    }

    /// Serve uma capa ja baixada pelo cliente. So `BookId` valido + versao numerica viram nome de
    /// arquivo (nada de `..`), e o tipo vem dos bytes (nunca da extensao).
    fn serve_cover(
        &self,
        raw_book_id: &str,
        raw_version: &str,
        is_head: bool,
    ) -> Response<Vec<u8>> {
        let (Ok(book_id), true) = (
            BookId::parse(raw_book_id),
            !raw_version.is_empty() && raw_version.bytes().all(|b| b.is_ascii_digit()),
        ) else {
            return build_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_cover",
                "Identificador de capa invalido",
                &[],
            );
        };
        let not_found = || {
            build_json_error(
                StatusCode::NOT_FOUND,
                "cover_not_found",
                "Capa nao encontrada neste aparelho",
                &[],
            )
        };
        let Some(dir) = self.covers_dir.as_ref() else {
            return not_found();
        };
        let Ok(bytes) = std::fs::read(dir.join(format!("{}-{raw_version}", book_id.as_str())))
        else {
            return not_found();
        };
        let content_type = match bytes.as_slice() {
            [0xFF, 0xD8, 0xFF, ..] => "image/jpeg",
            [0x89, b'P', b'N', b'G', ..] => "image/png",
            b if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" => "image/webp",
            _ => return not_found(),
        };
        let len = bytes.len().to_string();
        build_cors_response(
            StatusCode::OK,
            content_type,
            if is_head { Vec::new() } else { bytes },
            &[
                ("content-length", &len),
                // A versao esta na URL: o conteudo daquela URL nunca muda.
                ("cache-control", "private, max-age=31536000, immutable"),
                ("x-content-type-options", "nosniff"),
            ],
        )
    }

    fn serve_book_file(
        &self,
        raw_book_id: &str,
        is_head: bool,
        headers: &header::HeaderMap,
    ) -> Response<Vec<u8>> {
        let Ok(book_id) = BookId::parse(raw_book_id) else {
            return build_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_book_id",
                "Identificador de livro invalido",
                &[],
            );
        };

        let Some(file_path) = self.resolve_safe_book_file(&book_id) else {
            return build_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_book_id",
                "Caminho de livro invalido",
                &[],
            );
        };

        let Ok(mut file) = File::open(&file_path) else {
            return build_json_error(
                StatusCode::NOT_FOUND,
                "book_not_found",
                "Arquivo do livro nao encontrado no armazenamento local",
                &[],
            );
        };

        let Ok(meta) = file.metadata() else {
            return build_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "io_error",
                "Falha ao ler metadados do arquivo",
                &[],
            );
        };
        let total_size = meta.len();
        let content_type = detect_book_content_type(&mut file);

        if let Some(range_val) = headers.get(header::RANGE) {
            let Ok(range_str) = range_val.to_str() else {
                let cr = format!("bytes */{total_size}");
                return build_json_error(
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    "invalid_range",
                    "Cabecalho Range malformado",
                    &[("Content-Range", &cr)],
                );
            };

            let Some((start, end)) = parse_single_byte_range(range_str, total_size) else {
                let cr = format!("bytes */{total_size}");
                return build_json_error(
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    "invalid_range",
                    "Intervalo HTTP Range invalido ou fora dos limites",
                    &[("Content-Range", &cr)],
                );
            };

            let clamped_end = end.min(start.saturating_add(MAX_RANGE_CHUNK_BYTES - 1));
            let chunk_len = clamped_end - start + 1;

            let mut buf = Vec::new();
            if !is_head {
                if file.seek(SeekFrom::Start(start)).is_err() {
                    return build_json_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "io_error",
                        "Falha ao posicionar leitura do arquivo",
                        &[],
                    );
                }
                buf.reserve_exact(chunk_len as usize);
                if file.take(chunk_len).read_to_end(&mut buf).is_err() {
                    return build_json_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "io_error",
                        "Falha ao ler intervalo do arquivo",
                        &[],
                    );
                }
            }

            let content_range = format!("bytes {start}-{clamped_end}/{total_size}");
            let content_len = chunk_len.to_string();
            return build_cors_response(
                StatusCode::PARTIAL_CONTENT,
                content_type,
                buf,
                &[
                    ("Content-Range", &content_range),
                    ("Content-Length", &content_len),
                ],
            );
        }

        let mut buf = Vec::new();
        if !is_head
            && (file.seek(SeekFrom::Start(0)).is_err() || file.read_to_end(&mut buf).is_err())
        {
            return build_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "io_error",
                "Falha ao ler arquivo completo",
                &[],
            );
        }

        let content_len = total_size.to_string();
        build_cors_response(
            StatusCode::OK,
            content_type,
            buf,
            &[("Content-Length", &content_len)],
        )
    }

    fn serve_cbz_page(
        &self,
        raw_book_id: &str,
        raw_index: &str,
        is_head: bool,
    ) -> Response<Vec<u8>> {
        let Ok(book_id) = BookId::parse(raw_book_id) else {
            return build_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_book_id",
                "Identificador de livro invalido",
                &[],
            );
        };

        if raw_index.is_empty() || !raw_index.bytes().all(|b| b.is_ascii_digit()) {
            return build_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_page_index",
                "Indice de pagina CBZ invalido",
                &[],
            );
        }
        let Ok(page_index) = raw_index.parse::<usize>() else {
            return build_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_page_index",
                "Indice de pagina CBZ invalido",
                &[],
            );
        };

        let Some(file_path) = self.resolve_safe_book_file(&book_id) else {
            return build_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_book_id",
                "Caminho de livro invalido",
                &[],
            );
        };

        let Ok(meta) = std::fs::metadata(&file_path) else {
            return build_json_error(
                StatusCode::NOT_FOUND,
                "book_not_found",
                "Arquivo CBZ nao encontrado localmente",
                &[],
            );
        };
        let file_len = meta.len();
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as u64);

        // Verifica cache de pagina descomprimida em memoria
        {
            let mut cache = self
                .cbz_page_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(pos) = cache.iter().position(|item| {
                item.book_id == book_id.as_str()
                    && item.page_index == page_index
                    && item.file_len == file_len
                    && item.mtime_ms == mtime_ms
            }) {
                if let Some(hit) = cache.remove(pos) {
                    let bytes = if is_head {
                        Vec::new()
                    } else {
                        (*hit.bytes).clone()
                    };
                    let len_str = hit.bytes.len().to_string();
                    let content_type = hit.content_type;
                    cache.push_back(hit);
                    return build_cors_response(
                        StatusCode::OK,
                        content_type,
                        bytes,
                        &[
                            ("Content-Length", &len_str),
                            ("Cache-Control", "private, max-age=3600, immutable"),
                        ],
                    );
                }
            }
        }

        let pages = match self.get_or_index_cbz(&book_id, &file_path) {
            Ok(p) => p,
            Err(err_msg) => {
                return build_json_error(StatusCode::BAD_REQUEST, "invalid_cbz", &err_msg, &[]);
            }
        };

        let Some(page_meta) = pages.get(page_index) else {
            return build_json_error(
                StatusCode::NOT_FOUND,
                "cbz_page_out_of_range",
                "Indice de pagina fora do intervalo do CBZ",
                &[],
            );
        };

        if page_meta.declared_size > self.max_cbz_entry_bytes {
            return build_json_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "cbz_entry_too_large",
                "Entrada do CBZ excede o limite maximo de seguranca (protecao zip bomb)",
                &[],
            );
        }

        let Ok(file) = File::open(&file_path) else {
            return build_json_error(
                StatusCode::NOT_FOUND,
                "book_not_found",
                "Falha ao abrir arquivo CBZ",
                &[],
            );
        };
        let Ok(mut archive) = zip::ZipArchive::new(file) else {
            return build_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_cbz",
                "Arquivo CBZ corrompido ou formato ZIP invalido",
                &[],
            );
        };

        // Le exclusivamente por `by_index(page_meta.zip_index)`: o nome da entrada NUNCA vira caminho do disco!
        let Ok(entry) = archive.by_index(page_meta.zip_index) else {
            return build_json_error(
                StatusCode::NOT_FOUND,
                "cbz_entry_missing",
                "Entrada da pagina nao encontrada no arquivo ZIP",
                &[],
            );
        };

        if entry.size() > self.max_cbz_entry_bytes {
            return build_json_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "cbz_entry_too_large",
                "Entrada do CBZ excede o limite maximo de seguranca",
                &[],
            );
        }

        let initial_cap = (entry.size().min(4 * 1024 * 1024)) as usize;
        let mut buf = Vec::with_capacity(initial_cap);
        let limit = self.max_cbz_entry_bytes.saturating_add(1);
        if entry.take(limit).read_to_end(&mut buf).is_err() {
            return build_json_error(
                StatusCode::BAD_REQUEST,
                "cbz_read_error",
                "Erro ao descomprimir pagina do CBZ",
                &[],
            );
        }

        if (buf.len() as u64) > self.max_cbz_entry_bytes {
            return build_json_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "cbz_entry_too_large",
                "Stream descomprimido excedeu o limite maximo permitido (zip bomb detectada)",
                &[],
            );
        }

        let len_str = buf.len().to_string();
        let arc_bytes = Arc::new(buf);

        {
            let mut cache = self
                .cbz_page_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            while cache.len() >= CBZ_PAGE_CACHE_MAX_ITEMS {
                cache.pop_front();
            }
            cache.push_back(CachedCbzPage {
                book_id: book_id.as_str().to_string(),
                page_index,
                file_len,
                mtime_ms,
                content_type: page_meta.content_type,
                bytes: arc_bytes.clone(),
            });
        }

        let resp_body = if is_head {
            Vec::new()
        } else {
            (*arc_bytes).clone()
        };

        build_cors_response(
            StatusCode::OK,
            page_meta.content_type,
            resp_body,
            &[
                ("Content-Length", &len_str),
                ("Cache-Control", "private, max-age=3600, immutable"),
            ],
        )
    }

    fn get_or_index_cbz(
        &self,
        book_id: &BookId,
        file_path: &Path,
    ) -> Result<Arc<Vec<CbzPageEntry>>, String> {
        let meta = std::fs::metadata(file_path).map_err(|e| format!("erro de metadados: {e}"))?;
        let file_len = meta.len();
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as u64);

        {
            let cache = self
                .cbz_index_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(cached) = cache.get(book_id.as_str()) {
                if cached.file_len == file_len && cached.mtime_ms == mtime_ms {
                    return Ok(cached.pages.clone());
                }
            }
        }

        let file = File::open(file_path).map_err(|e| format!("falha ao abrir CBZ: {e}"))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("arquivo ZIP invalido: {e}"))?;

        let mut pages = Vec::new();
        for idx in 0..archive.len() {
            let Ok(entry) = archive.by_index_raw(idx) else {
                continue;
            };
            if entry.is_dir() {
                continue;
            }
            let raw_name = entry.name();
            if let Some((normalized_name, content_type)) = classify_cbz_image_entry(raw_name) {
                pages.push(CbzPageEntry {
                    zip_index: idx,
                    entry_name: normalized_name,
                    content_type,
                    declared_size: entry.size(),
                });
            }
        }

        pages.sort_by(|a, b| natural_cmp(&a.entry_name, &b.entry_name));
        let arc_pages = Arc::new(pages);

        {
            let mut cache = self
                .cbz_index_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            cache.insert(
                book_id.as_str().to_string(),
                CachedCbzIndex {
                    file_len,
                    mtime_ms,
                    pages: arc_pages.clone(),
                },
            );
        }

        Ok(arc_pages)
    }
}

/// Normaliza o caminho da URI recebida no protocolo customizado, tratando tanto
/// `bookferry://localhost/books/<id>`, `bookferry://books/<id>` (Linux/macOS)
/// quanto `http://bookferry.localhost/books/<id>` (Android/Windows).
pub fn normalize_protocol_path(uri: &Uri) -> String {
    let raw_path = uri.path();
    if let Some(host) = uri.host() {
        let lower_host = host.to_ascii_lowercase();
        if lower_host != "localhost"
            && lower_host != "127.0.0.1"
            && !lower_host.ends_with(".localhost")
        {
            if raw_path.is_empty() || raw_path == "/" {
                return format!("/{host}");
            }
            return format!("/{host}{raw_path}");
        }
    }
    raw_path.to_string()
}

/// Valida e classifica o nome de uma entrada ZIP para leitura CBZ:
/// - Rejeita diretorios, NUL bytes, caminhos absolutos e segmentos `..` (path traversal).
/// - Ignora entradas em `__MACOSX` e dotfiles (qualquer segmento iniciado por `.`).
/// - Aceita exclusivamente extensoes `.jpg`, `.jpeg`, `.png`, `.webp`, `.gif`, `.avif`.
/// - Nunca converte o nome em caminho do sistema de arquivos.
pub fn classify_cbz_image_entry(raw_name: &str) -> Option<(String, &'static str)> {
    if raw_name.is_empty()
        || raw_name.contains('\0')
        || raw_name.ends_with('/')
        || raw_name.ends_with('\\')
    {
        return None;
    }

    let normalized = raw_name.replace('\\', "/");
    if normalized.starts_with('/') {
        return None;
    }

    for segment in normalized.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." || segment == "__MACOSX" {
            return None;
        }
        if segment.starts_with('.') {
            return None;
        }
    }

    let file_name = normalized.rsplit('/').next()?;
    let (_, ext) = file_name.rsplit_once('.')?;
    let content_type = match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "avif" => "image/avif",
        _ => return None,
    };

    Some((normalized, content_type))
}

/// Ordenacao natural deterministica de nomes de paginas (`page2.jpg` < `page10.jpg`).
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

/// Analisa um cabecalho `Range: bytes=...` de intervalo unico (RFC 9110),
/// limitando `end` a `total_size - 1`.
pub fn parse_single_byte_range(header: &str, total_size: u64) -> Option<(u64, u64)> {
    if total_size == 0 {
        return None;
    }
    let spec = header.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start_str, end_str) = spec.split_once('-')?;
    let start_str = start_str.trim();
    let end_str = end_str.trim();

    if start_str.is_empty() {
        let suffix_len = end_str.parse::<u64>().ok()?;
        if suffix_len == 0 {
            return None;
        }
        let start = total_size.saturating_sub(suffix_len);
        Some((start, total_size - 1))
    } else {
        let start = start_str.parse::<u64>().ok()?;
        if start >= total_size {
            return None;
        }
        if end_str.is_empty() {
            Some((start, total_size - 1))
        } else {
            let end = end_str.parse::<u64>().ok()?;
            if start > end {
                return None;
            }
            Some((start, end.min(total_size - 1)))
        }
    }
}

fn detect_book_content_type(file: &mut File) -> &'static str {
    let mut magic = [0u8; 64];
    if file.seek(SeekFrom::Start(0)).is_ok() {
        let n = file.read(&mut magic).unwrap_or(0);
        let slice = &magic[..n];
        if slice.starts_with(b"%PDF-") {
            return BookFormat::Pdf.content_type();
        }
        if slice.starts_with(b"PK\x03\x04") {
            if slice.windows(8).any(|w| w == b"mimetype") {
                return BookFormat::Epub.content_type();
            }
            return BookFormat::Cbz.content_type();
        }
    }
    "application/octet-stream"
}

fn build_cors_response(
    status: StatusCode,
    content_type: &str,
    body: Vec<u8>,
    extra_headers: &[(&str, &str)],
) -> Response<Vec<u8>> {
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCESS_CONTROL_ALLOW_METHODS, "GET, HEAD, OPTIONS")
        .header(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            "Range, If-Range, Content-Type, Accept",
        )
        .header(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            "Content-Range, Content-Length, Accept-Ranges, ETag",
        );

    for &(k, v) in extra_headers {
        builder = builder.header(k, v);
    }

    builder
        .body(body)
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

fn build_json_error(
    status: StatusCode,
    code: &str,
    message: &str,
    extra_headers: &[(&str, &str)],
) -> Response<Vec<u8>> {
    let body = serde_json::to_vec(&serde_json::json!({
        "error": code,
        "message": message,
    }))
    .unwrap_or_default();
    build_cors_response(status, "application/json", body, extra_headers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;

    fn write_book_file(dir: &Path, data: &[u8]) -> BookId {
        let mut cursor = std::io::Cursor::new(data);
        let (book_id, _) = BookId::from_reader(&mut cursor, data.len() as u64).unwrap();
        let path = dir.join(book_id.as_str());
        std::fs::write(path, data).unwrap();
        book_id
    }

    #[test]
    fn http_range_200_206_clamp_end_and_416_validation() {
        let tmp = TempDir::new().unwrap();
        let mut pdf_bytes = b"%PDF-1.7\n".to_vec();
        for i in 0..10_000u32 {
            pdf_bytes.extend_from_slice(&i.to_be_bytes());
        }
        let total_len = pdf_bytes.len();
        let book_id = write_book_file(tmp.path(), &pdf_bytes);
        let handler = ProtocolHandler::new(tmp.path());

        // 1. GET completo (200 OK + Accept-Ranges: bytes)
        let req_full = Request::builder()
            .method(Method::GET)
            .uri(format!("bookferry://localhost/books/{book_id}"))
            .body(Vec::new())
            .unwrap();
        let resp_full = handler.handle_request(&req_full);
        assert_eq!(resp_full.status(), StatusCode::OK);
        assert_eq!(
            resp_full.headers().get(header::ACCEPT_RANGES).unwrap(),
            "bytes"
        );
        assert_eq!(
            resp_full.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/pdf"
        );
        assert_eq!(resp_full.body().len(), total_len);
        assert_eq!(resp_full.body().as_slice(), pdf_bytes.as_slice());

        // 2. Range parcial 100..=299 via scheme Android http://bookferry.localhost (206 Partial Content)
        let req_range = Request::builder()
            .method(Method::GET)
            .uri(format!("http://bookferry.localhost/books/{book_id}"))
            .header(header::RANGE, "bytes=100-299")
            .body(Vec::new())
            .unwrap();
        let resp_range = handler.handle_request(&req_range);
        assert_eq!(resp_range.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            resp_range.headers().get(header::CONTENT_RANGE).unwrap(),
            &format!("bytes 100-299/{total_len}")
        );
        assert_eq!(resp_range.body(), &pdf_bytes[100..=299]);

        // 3. Range com fim alem do tamanho do arquivo -> limitado a total_len - 1 (206 Partial Content)
        let start = total_len - 50;
        let req_clamp = Request::builder()
            .method(Method::GET)
            .uri(format!("bookferry://localhost/books/{book_id}"))
            .header(header::RANGE, format!("bytes={start}-999999999"))
            .body(Vec::new())
            .unwrap();
        let resp_clamp = handler.handle_request(&req_clamp);
        assert_eq!(resp_clamp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            resp_clamp.headers().get(header::CONTENT_RANGE).unwrap(),
            &format!("bytes {start}-{}/{total_len}", total_len - 1)
        );
        assert_eq!(resp_clamp.body(), &pdf_bytes[start..]);

        // 4. Range invalido (start >= total_len, multiplos ranges, start > end) -> 416
        for bad_range in [
            format!("bytes={total_len}-{}", total_len + 10),
            "bytes=200-100".to_string(),
            "bytes=0-10,20-30".to_string(),
            "items=0-10".to_string(),
        ] {
            let req_bad = Request::builder()
                .method(Method::GET)
                .uri(format!("bookferry://localhost/books/{book_id}"))
                .header(header::RANGE, bad_range)
                .body(Vec::new())
                .unwrap();
            let resp_bad = handler.handle_request(&req_bad);
            assert_eq!(resp_bad.status(), StatusCode::RANGE_NOT_SATISFIABLE);
            assert_eq!(
                resp_bad.headers().get(header::CONTENT_RANGE).unwrap(),
                &format!("bytes */{total_len}")
            );
        }
    }

    #[test]
    fn path_traversal_and_invalid_book_id_rejected() {
        let tmp = TempDir::new().unwrap();
        let handler = ProtocolHandler::new(tmp.path());

        for malicious_uri in [
            "bookferry://localhost/books/../../etc/passwd",
            "bookferry://localhost/books/..%2f..%2fetc%2fpasswd",
            "bookferry://localhost/books/d41d8cd98f00b204e9800998ecf8427e-100/../secret",
            "http://bookferry.localhost/cbz/../../etc/passwd/page/0",
            "http://bookferry.localhost/cbz/ZZZZ8cd98f00b204e9800998ecf8427e-100/page/0",
        ] {
            let req = Request::builder()
                .method(Method::GET)
                .uri(malicious_uri)
                .body(Vec::new())
                .unwrap();
            let resp = handler.handle_request(&req);
            assert!(
                resp.status() == StatusCode::BAD_REQUEST || resp.status() == StatusCode::NOT_FOUND,
                "URI maliciosa deveria ser rejeitada ({malicious_uri}): status={}",
                resp.status()
            );
        }
    }

    #[test]
    fn cbz_natural_order_hostile_entries_invalid_index_and_zip_bomb_protection() {
        let tmp = TempDir::new().unwrap();

        let mut zip_buf = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut zip_buf);
            let mut zw = zip::ZipWriter::new(cursor);
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

            // Entradas hostis e ignoradas:
            zw.add_directory("__MACOSX/", opts).unwrap();
            zw.start_file("__MACOSX/._page1.png", opts).unwrap();
            zw.write_all(b"macosx_junk").unwrap();

            zw.start_file(".DS_Store", opts).unwrap();
            zw.write_all(b"ds_store_junk").unwrap();

            zw.start_file("chapter/.hidden.jpg", opts).unwrap();
            zw.write_all(b"dotfile_junk").unwrap();

            zw.start_file("../../evil_traversal.png", opts).unwrap();
            zw.write_all(b"traversal_attempt").unwrap();

            zw.start_file("ComicInfo.xml", opts).unwrap();
            zw.write_all(b"<ComicInfo/>").unwrap();

            zw.start_file("script.sh", opts).unwrap();
            zw.write_all(b"#!/bin/sh").unwrap();

            // Paginas validas fora de ordem lexicografica para provar ordenacao natural:
            zw.start_file("page10.webp", opts).unwrap();
            zw.write_all(b"WEBP_PAGE_10").unwrap();

            zw.start_file("page2.png", opts).unwrap();
            zw.write_all(b"PNG_PAGE_2").unwrap();

            zw.start_file("page1.jpg", opts).unwrap();
            zw.write_all(b"JPG_PAGE_1").unwrap();

            zw.start_file("page20.avif", opts).unwrap();
            zw.write_all(b"AVIF_PAGE_20").unwrap();

            // Entrada gigante ("zip bomb" acima do limite de teste de 4 KiB)
            zw.start_file("page99_bomb.gif", opts).unwrap();
            zw.write_all(&vec![b'Z'; 16 * 1024]).unwrap();

            zw.finish().unwrap();
        }

        let book_id = write_book_file(tmp.path(), &zip_buf);
        // Limite de 4 KiB para testar protecao contra zip bomb
        let handler = ProtocolHandler::with_max_cbz_entry_bytes(tmp.path(), 4096);

        // 5 paginas de imagem validas (page1.jpg, page2.png, page10.webp, page20.avif, page99_bomb.gif)
        assert_eq!(handler.inspect_cbz_page_count(book_id.as_str()).unwrap(), 5);

        // Indice 0 -> page1.jpg (image/jpeg)
        let req0 = Request::builder()
            .method(Method::GET)
            .uri(format!("bookferry://localhost/cbz/{book_id}/page/0"))
            .body(Vec::new())
            .unwrap();
        let resp0 = handler.handle_request(&req0);
        assert_eq!(resp0.status(), StatusCode::OK);
        assert_eq!(
            resp0.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/jpeg"
        );
        assert_eq!(resp0.body(), b"JPG_PAGE_1");

        // Indice 1 -> page2.png (image/png)
        let req1 = Request::builder()
            .method(Method::GET)
            .uri(format!("http://bookferry.localhost/cbz/{book_id}/page/1"))
            .body(Vec::new())
            .unwrap();
        let resp1 = handler.handle_request(&req1);
        assert_eq!(resp1.status(), StatusCode::OK);
        assert_eq!(
            resp1.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/png"
        );
        assert_eq!(resp1.body(), b"PNG_PAGE_2");

        // Indice 2 -> page10.webp (image/webp)
        let req2 = Request::builder()
            .method(Method::GET)
            .uri(format!("bookferry://localhost/cbz/{book_id}/page/2"))
            .body(Vec::new())
            .unwrap();
        let resp2 = handler.handle_request(&req2);
        assert_eq!(resp2.status(), StatusCode::OK);
        assert_eq!(
            resp2.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/webp"
        );
        assert_eq!(resp2.body(), b"WEBP_PAGE_10");

        // Indice 3 -> page20.avif (image/avif)
        let req3 = Request::builder()
            .method(Method::GET)
            .uri(format!("bookferry://localhost/cbz/{book_id}/page/3"))
            .body(Vec::new())
            .unwrap();
        let resp3 = handler.handle_request(&req3);
        assert_eq!(resp3.status(), StatusCode::OK);
        assert_eq!(
            resp3.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/avif"
        );
        assert_eq!(resp3.body(), b"AVIF_PAGE_20");

        // Indice 4 -> page99_bomb.gif (16 KiB > limite de 4 KiB) -> 413 Payload Too Large!
        let req_bomb = Request::builder()
            .method(Method::GET)
            .uri(format!("bookferry://localhost/cbz/{book_id}/page/4"))
            .body(Vec::new())
            .unwrap();
        let resp_bomb = handler.handle_request(&req_bomb);
        assert_eq!(resp_bomb.status(), StatusCode::PAYLOAD_TOO_LARGE);

        // Indice fora do intervalo (99) -> 404 Not Found
        let req_oob = Request::builder()
            .method(Method::GET)
            .uri(format!("bookferry://localhost/cbz/{book_id}/page/99"))
            .body(Vec::new())
            .unwrap();
        let resp_oob = handler.handle_request(&req_oob);
        assert_eq!(resp_oob.status(), StatusCode::NOT_FOUND);

        // Indice nao numerico ("-1", "abc") -> 400 Bad Request
        for bad_idx in ["-1", "abc", "1..2"] {
            let req_bad_idx = Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "bookferry://localhost/cbz/{book_id}/page/{bad_idx}"
                ))
                .body(Vec::new())
                .unwrap();
            let resp_bad_idx = handler.handle_request(&req_bad_idx);
            assert_eq!(resp_bad_idx.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[test]
    fn covers_are_served_by_id_and_version_with_type_from_bytes() {
        let books = TempDir::new().unwrap();
        let covers = TempDir::new().unwrap();
        let id = "d41d8cd98f00b204e9800998ecf8427e-100";
        std::fs::write(
            covers.path().join(format!("{id}-7")),
            [0xFF, 0xD8, 0xFF, 0xE0, 1, 2],
        )
        .unwrap();
        std::fs::write(
            covers.path().join(format!("{id}-8")),
            b"<svg onload=alert(1)/>",
        )
        .unwrap();
        let handler = ProtocolHandler::new(books.path()).with_covers_dir(covers.path());
        let get = |uri: &str| {
            let req = Request::builder()
                .method(Method::GET)
                .uri(uri)
                .body(Vec::new())
                .unwrap();
            handler.handle_request(&req)
        };

        let ok = get(&format!("bookferry://localhost/covers/{id}/7"));
        assert_eq!(ok.status(), StatusCode::OK);
        assert_eq!(
            ok.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/jpeg"
        );
        assert_eq!(ok.body(), &vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2]);
        // Arquivo que nao e imagem nunca e servido, mesmo existindo.
        assert_eq!(
            get(&format!("bookferry://localhost/covers/{id}/8")).status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get(&format!("bookferry://localhost/covers/{id}/9")).status(),
            StatusCode::NOT_FOUND
        );
        for bad in [
            "bookferry://localhost/covers/../../etc/passwd/1".to_string(),
            format!("bookferry://localhost/covers/{id}/7..%2f..%2fsecret"),
            format!("bookferry://localhost/covers/{id}/abc"),
            format!("bookferry://localhost/covers/{id}/"),
            "bookferry://localhost/covers/ZZZ/1".to_string(),
        ] {
            let st = get(&bad).status();
            assert!(
                st == StatusCode::BAD_REQUEST || st == StatusCode::NOT_FOUND,
                "{bad}: {st}"
            );
        }
        // Sem `covers_dir` configurado a rota responde 404.
        let plain = ProtocolHandler::new(books.path());
        let req = Request::builder()
            .method(Method::GET)
            .uri(format!("bookferry://localhost/covers/{id}/7"))
            .body(Vec::new())
            .unwrap();
        assert_eq!(plain.handle_request(&req).status(), StatusCode::NOT_FOUND);
    }
}

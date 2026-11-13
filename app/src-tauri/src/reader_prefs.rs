//! Preferencias de leitura (EPUB e CBZ) armazenadas localmente em
//! `<data_dir>/reader_prefs.json`. Nao sincronizam com o servidor.
//!
//! Seguranca: nenhum valor vindo do arquivo ou do WebView e usado "cru". Toda entrada passa
//! por sanitizacao campo a campo a partir de `serde_json::Value` (sem `Deserialize` estrito):
//! tipo errado, enum desconhecido ou valor fora da faixa viram clamp (numericos) ou o padrao
//! do campo (enums/bool); campos desconhecidos sao ignorados. Como os enums sao tipos Rust
//! fechados, o JS so recebe de volta valores de uma lista conhecida.

use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

pub const PREFS_FILENAME: &str = "reader_prefs.json";
const PREFS_TMP_FILENAME: &str = "reader_prefs.json.tmp";
const PREFS_VERSION: u64 = 1;
/// Maximo de livros com preferencias proprias.
pub const MAX_BOOKS: usize = 5000;
/// Maximo de presets do usuario.
pub const MAX_PRESETS: usize = 12;
/// Tamanho maximo (em caracteres) do nome de um preset.
pub const MAX_PRESET_NAME_CHARS: usize = 24;
const MAX_BOOK_ID_LEN: usize = 128;

// ---------------------------------------------------------------------------
// Enums fechados
// ---------------------------------------------------------------------------

macro_rules! str_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $s:literal),+ $(,)? } default $def:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
        pub enum $name {
            $( #[serde(rename = $s)] $variant, )+
        }

        impl Default for $name {
            fn default() -> Self {
                Self::$def
            }
        }

        impl $name {
            /// Converte a partir de um valor JSON; qualquer coisa fora da lista vira o padrao.
            fn from_value(v: Option<&Value>) -> Self {
                match v.and_then(Value::as_str) {
                    $( Some($s) => Self::$variant, )+
                    _ => Self::default(),
                }
            }
        }
    };
}

str_enum!(FontFamily { Book => "book", Serif => "serif", Sans => "sans", Mono => "mono" } default Book);
str_enum!(Align { Justify => "justify", Left => "left" } default Justify);
str_enum!(Theme { Auto => "auto", Light => "light", Sepia => "sepia", Dark => "dark", Black => "black" } default Auto);
str_enum!(Direction { Ltr => "ltr", Rtl => "rtl" } default Ltr);
str_enum!(SpreadMode { Single => "single", Double => "double", Auto => "auto" } default Single);
str_enum!(Fit { Page => "page", Width => "width", Height => "height", Original => "original" } default Page);
str_enum!(Background { Theme => "theme", Black => "black", Light => "light" } default Theme);

// ---------------------------------------------------------------------------
// Estruturas
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReaderPrefs {
    pub font_size: u32,
    pub font_family: FontFamily,
    pub line_height: f64,
    pub margin_pct: u32,
    pub align: Align,
    pub hyphenate: bool,
    pub theme: Theme,
    // CBZ
    pub direction: Direction,
    pub spread_mode: SpreadMode,
    pub cover_alone: bool,
    pub fit: Fit,
    pub zero_margin: bool,
    pub bg: Background,
}

impl Default for ReaderPrefs {
    fn default() -> Self {
        Self {
            font_size: 18,
            font_family: FontFamily::default(),
            line_height: 1.65,
            margin_pct: 6,
            align: Align::default(),
            hyphenate: false,
            theme: Theme::default(),
            direction: Direction::default(),
            spread_mode: SpreadMode::default(),
            cover_alone: true,
            fit: Fit::default(),
            zero_margin: false,
            bg: Background::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProgressPrefs {
    pub chapter: bool,
    pub book_percent: bool,
    pub chapter_percent: bool,
    pub pages_left: bool,
    pub page_format: u8,
}

impl Default for ProgressPrefs {
    fn default() -> Self {
        Self {
            chapter: true,
            book_percent: true,
            chapter_percent: true,
            pages_left: true,
            page_format: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Preset {
    pub name: String,
    pub prefs: ReaderPrefs,
}

/// Conteudo completo de `reader_prefs.json` (ja sanitizado).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrefsFile {
    pub defaults: ReaderPrefs,
    pub progress: ProgressPrefs,
    pub presets: Vec<Preset>,
    pub books: BTreeMap<String, ReaderPrefs>,
}

/// Visao devolvida por todos os comandos, calculada para um `book_id`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReaderPrefsView {
    /// Preferencias do livro, ou os `defaults` se o livro nao tem entrada propria.
    pub prefs: ReaderPrefs,
    /// `true` se o livro tem entrada propria.
    pub is_custom: bool,
    pub defaults: ReaderPrefs,
    pub presets: Vec<Preset>,
    pub progress: ProgressPrefs,
}

// ---------------------------------------------------------------------------
// Sanitizacao
// ---------------------------------------------------------------------------

fn bool_field(obj: &Map<String, Value>, key: &str, default: bool) -> bool {
    obj.get(key).and_then(Value::as_bool).unwrap_or(default)
}

/// NaN/infinito viram `default`; os demais sofrem clamp em `min..=max`.
fn finite_clamp(n: f64, default: f64, min: f64, max: f64) -> f64 {
    if n.is_finite() {
        n.clamp(min, max)
    } else {
        default
    }
}

/// Le um numero; tipo errado vira `default`. Faz clamp em `min..=max`.
fn f64_field(obj: &Map<String, Value>, key: &str, default: f64, min: f64, max: f64) -> f64 {
    match obj.get(key).and_then(Value::as_f64) {
        Some(n) => finite_clamp(n, default, min, max),
        None => default,
    }
}

fn u32_field(obj: &Map<String, Value>, key: &str, default: u32, min: u32, max: u32) -> u32 {
    f64_field(obj, key, f64::from(default), f64::from(min), f64::from(max)).round() as u32
}

/// Sanitiza um `ReaderPrefs` a partir de qualquer `Value` (nao-objeto => tudo padrao).
pub fn sanitize_prefs(v: &Value) -> ReaderPrefs {
    let empty = Map::new();
    let obj = v.as_object().unwrap_or(&empty);
    let d = ReaderPrefs::default();
    let line_height = f64_field(obj, "line_height", d.line_height, 1.2, 2.2);
    // Arredonda ao multiplo de 0.05 (sem ruido de ponto flutuante).
    let line_height = ((line_height * 20.0).round() / 20.0).clamp(1.2, 2.2);
    ReaderPrefs {
        font_size: u32_field(obj, "font_size", d.font_size, 12, 36),
        font_family: FontFamily::from_value(obj.get("font_family")),
        line_height,
        margin_pct: u32_field(obj, "margin_pct", d.margin_pct, 0, 20),
        align: Align::from_value(obj.get("align")),
        hyphenate: bool_field(obj, "hyphenate", d.hyphenate),
        theme: Theme::from_value(obj.get("theme")),
        direction: Direction::from_value(obj.get("direction")),
        spread_mode: SpreadMode::from_value(obj.get("spread_mode")),
        cover_alone: bool_field(obj, "cover_alone", d.cover_alone),
        fit: Fit::from_value(obj.get("fit")),
        zero_margin: bool_field(obj, "zero_margin", d.zero_margin),
        bg: Background::from_value(obj.get("bg")),
    }
}

pub fn sanitize_progress(v: &Value) -> ProgressPrefs {
    let empty = Map::new();
    let obj = v.as_object().unwrap_or(&empty);
    let d = ProgressPrefs::default();
    ProgressPrefs {
        chapter: bool_field(obj, "chapter", d.chapter),
        book_percent: bool_field(obj, "book_percent", d.book_percent),
        chapter_percent: bool_field(obj, "chapter_percent", d.chapter_percent),
        pages_left: bool_field(obj, "pages_left", d.pages_left),
        page_format: u32_field(obj, "page_format", u32::from(d.page_format), 0, 2) as u8,
    }
}

/// Valida o nome de um preset. Devolve o nome ja com `trim`.
pub fn validate_preset_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    let len = name.chars().count();
    if len == 0 {
        return Err("o nome do preset nao pode ser vazio".to_string());
    }
    if len > MAX_PRESET_NAME_CHARS {
        return Err(format!(
            "o nome do preset deve ter no maximo {MAX_PRESET_NAME_CHARS} caracteres"
        ));
    }
    if name.chars().any(|c| c.is_control() || c == '<' || c == '>') {
        return Err("o nome do preset contem caracteres nao permitidos".to_string());
    }
    Ok(name.to_string())
}

/// Valida um `book_id`: apenas `[0-9A-Za-z_-]{1,128}`.
pub fn validate_book_id(raw: &str) -> Result<&str, String> {
    let ok = !raw.is_empty()
        && raw.len() <= MAX_BOOK_ID_LEN
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if ok {
        Ok(raw)
    } else {
        Err("book_id invalido".to_string())
    }
}

/// Sanitiza o arquivo inteiro. Versao desconhecida ou formato inesperado => tudo padrao.
pub fn sanitize_file(v: &Value) -> PrefsFile {
    let Some(obj) = v.as_object() else {
        return PrefsFile::default();
    };
    if obj.get("version").and_then(Value::as_u64) != Some(PREFS_VERSION) {
        return PrefsFile::default();
    }
    let null = Value::Null;
    let defaults = sanitize_prefs(obj.get("defaults").unwrap_or(&null));
    let progress = sanitize_progress(obj.get("progress").unwrap_or(&null));

    let mut presets: Vec<Preset> = Vec::new();
    if let Some(list) = obj.get("presets").and_then(Value::as_array) {
        for item in list {
            if presets.len() >= MAX_PRESETS {
                break;
            }
            let Some(item) = item.as_object() else {
                continue;
            };
            let Some(name) = item
                .get("name")
                .and_then(Value::as_str)
                .and_then(|n| validate_preset_name(n).ok())
            else {
                continue;
            };
            if presets.iter().any(|p| p.name == name) {
                continue;
            }
            presets.push(Preset {
                name,
                prefs: sanitize_prefs(item.get("prefs").unwrap_or(&null)),
            });
        }
    }

    let mut books = BTreeMap::new();
    if let Some(map) = obj.get("books").and_then(Value::as_object) {
        for (id, prefs) in map {
            if books.len() >= MAX_BOOKS {
                break;
            }
            if validate_book_id(id).is_ok() {
                books.insert(id.clone(), sanitize_prefs(prefs));
            }
        }
    }

    PrefsFile {
        defaults,
        progress,
        presets,
        books,
    }
}

// ---------------------------------------------------------------------------
// Persistencia
// ---------------------------------------------------------------------------

/// Carrega o arquivo. Ausente, ilegivel, corrompido ou de versao desconhecida => padroes.
/// Nunca falha e nunca reescreve o arquivo (um arquivo corrompido so e substituido na
/// proxima escrita legitima).
pub fn load_file(data_dir: &Path) -> PrefsFile {
    let path = data_dir.join(PREFS_FILENAME);
    let Ok(raw) = std::fs::read(&path) else {
        return PrefsFile::default();
    };
    match serde_json::from_slice::<Value>(&raw) {
        Ok(v) => sanitize_file(&v),
        Err(_) => PrefsFile::default(),
    }
}

fn file_to_json(file: &PrefsFile) -> Result<Vec<u8>, String> {
    let to_val = |p: &ReaderPrefs| serde_json::to_value(p).map_err(|e| e.to_string());
    let mut books = Map::new();
    for (id, p) in &file.books {
        books.insert(id.clone(), to_val(p)?);
    }
    let doc = json!({
        "version": PREFS_VERSION,
        "defaults": to_val(&file.defaults)?,
        "progress": serde_json::to_value(&file.progress).map_err(|e| e.to_string())?,
        "presets": serde_json::to_value(&file.presets).map_err(|e| e.to_string())?,
        "books": Value::Object(books),
    });
    serde_json::to_vec_pretty(&doc).map_err(|e| format!("erro ao serializar preferencias: {e}"))
}

/// Escrita atomica: grava `reader_prefs.json.tmp` na mesma pasta, faz fsync e `rename`.
pub fn save_file(data_dir: &Path, file: &PrefsFile) -> Result<(), String> {
    std::fs::create_dir_all(data_dir).map_err(|e| format!("erro ao criar data_dir: {e}"))?;
    let bytes = file_to_json(file)?;
    let tmp = data_dir.join(PREFS_TMP_FILENAME);
    let dest = data_dir.join(PREFS_FILENAME);

    let write_tmp = || -> std::io::Result<()> {
        // `File::create` trunca um .tmp sujo deixado por uma execucao anterior.
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        Ok(())
    };
    if let Err(e) = write_tmp() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("erro ao gravar preferencias de leitura: {e}"));
    }
    if let Err(e) = std::fs::rename(&tmp, &dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("erro ao gravar preferencias de leitura: {e}"));
    }
    // Melhor esforco: persiste a entrada de diretorio do rename.
    #[cfg(unix)]
    if let Ok(dir) = std::fs::File::open(data_dir) {
        let _ = dir.sync_all();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Operacoes (a serializacao via mutex e feita pelo chamador, em commands.rs)
// ---------------------------------------------------------------------------

pub fn build_view(file: &PrefsFile, book_id: &str) -> ReaderPrefsView {
    let own = file.books.get(book_id);
    ReaderPrefsView {
        prefs: own.cloned().unwrap_or_else(|| file.defaults.clone()),
        is_custom: own.is_some(),
        defaults: file.defaults.clone(),
        presets: file.presets.clone(),
        progress: file.progress.clone(),
    }
}

pub fn op_get(data_dir: &Path, book_id: &str) -> Result<ReaderPrefsView, String> {
    let book_id = validate_book_id(book_id)?;
    Ok(build_view(&load_file(data_dir), book_id))
}

pub fn op_save_book(
    data_dir: &Path,
    book_id: &str,
    prefs: &Value,
) -> Result<ReaderPrefsView, String> {
    let book_id = validate_book_id(book_id)?;
    let mut file = load_file(data_dir);
    if !file.books.contains_key(book_id) && file.books.len() >= MAX_BOOKS {
        return Err(format!(
            "limite de {MAX_BOOKS} livros com preferencias proprias atingido"
        ));
    }
    file.books
        .insert(book_id.to_string(), sanitize_prefs(prefs));
    save_file(data_dir, &file)?;
    Ok(build_view(&file, book_id))
}

pub fn op_reset_book(data_dir: &Path, book_id: &str) -> Result<ReaderPrefsView, String> {
    let book_id = validate_book_id(book_id)?;
    let mut file = load_file(data_dir);
    if file.books.remove(book_id).is_some() {
        save_file(data_dir, &file)?;
    }
    Ok(build_view(&file, book_id))
}

pub fn op_set_default(
    data_dir: &Path,
    book_id: &str,
    prefs: &Value,
) -> Result<ReaderPrefsView, String> {
    let book_id = validate_book_id(book_id)?;
    let mut file = load_file(data_dir);
    file.defaults = sanitize_prefs(prefs);
    save_file(data_dir, &file)?;
    Ok(build_view(&file, book_id))
}

pub fn op_save_preset(
    data_dir: &Path,
    book_id: &str,
    name: &str,
    prefs: &Value,
) -> Result<ReaderPrefsView, String> {
    let book_id = validate_book_id(book_id)?;
    let name = validate_preset_name(name)?;
    let mut file = load_file(data_dir);
    let prefs = sanitize_prefs(prefs);
    if let Some(existing) = file.presets.iter_mut().find(|p| p.name == name) {
        existing.prefs = prefs;
    } else {
        if file.presets.len() >= MAX_PRESETS {
            return Err(format!("limite de {MAX_PRESETS} presets atingido"));
        }
        file.presets.push(Preset { name, prefs });
    }
    save_file(data_dir, &file)?;
    Ok(build_view(&file, book_id))
}

pub fn op_delete_preset(
    data_dir: &Path,
    book_id: &str,
    name: &str,
) -> Result<ReaderPrefsView, String> {
    let book_id = validate_book_id(book_id)?;
    let name = name.trim();
    let mut file = load_file(data_dir);
    let before = file.presets.len();
    file.presets.retain(|p| p.name != name);
    if file.presets.len() != before {
        save_file(data_dir, &file)?;
    }
    Ok(build_view(&file, book_id))
}

pub fn op_save_progress(
    data_dir: &Path,
    book_id: &str,
    progress: &Value,
) -> Result<ReaderPrefsView, String> {
    let book_id = validate_book_id(book_id)?;
    let mut file = load_file(data_dir);
    file.progress = sanitize_progress(progress);
    save_file(data_dir, &file)?;
    Ok(build_view(&file, book_id))
}

// ---------------------------------------------------------------------------
// Testes
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BOOK_A: &str = "0123456789abcdef0123456789abcdef-1000";
    const BOOK_B: &str = "fedcba9876543210fedcba9876543210-2000";

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn padroes_conforme_contrato() {
        let p = sanitize_prefs(&json!({}));
        assert_eq!(p, ReaderPrefs::default());
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(
            v,
            json!({
                "font_size": 18, "font_family": "book", "line_height": 1.65,
                "margin_pct": 6, "align": "justify", "hyphenate": false,
                "theme": "auto", "direction": "ltr", "spread_mode": "single",
                "cover_alone": true, "fit": "page", "zero_margin": false, "bg": "theme"
            })
        );
        let pr = serde_json::to_value(ProgressPrefs::default()).unwrap();
        assert_eq!(
            pr,
            json!({"chapter": true, "book_percent": true, "chapter_percent": true,
                   "pages_left": true, "page_format": 0})
        );
        // Nao-objeto => padrao
        assert_eq!(sanitize_prefs(&json!("x")), ReaderPrefs::default());
        assert_eq!(sanitize_prefs(&Value::Null), ReaderPrefs::default());
    }

    #[test]
    fn valores_fora_da_faixa_sofrem_clamp() {
        let p = sanitize_prefs(&json!({
            "font_size": 999, "margin_pct": 500, "line_height": 9.0
        }));
        assert_eq!(p.font_size, 36);
        assert_eq!(p.margin_pct, 20);
        assert_eq!(p.line_height, 2.2);
        let p = sanitize_prefs(&json!({
            "font_size": -5, "margin_pct": -1, "line_height": 0.1
        }));
        assert_eq!(p.font_size, 12);
        assert_eq!(p.margin_pct, 0);
        assert_eq!(p.line_height, 1.2);
        let p = sanitize_prefs(&json!({"font_size": 1e300, "margin_pct": -1e300}));
        assert_eq!(p.font_size, 36);
        assert_eq!(p.margin_pct, 0);
        let pr = sanitize_progress(&json!({"page_format": 7}));
        assert_eq!(pr.page_format, 2);
        let pr = sanitize_progress(&json!({"page_format": -3}));
        assert_eq!(pr.page_format, 0);
    }

    #[test]
    fn line_height_arredonda_para_multiplo_de_005() {
        let p = sanitize_prefs(&json!({"line_height": 1.67}));
        assert_eq!(p.line_height, 1.65);
        let p = sanitize_prefs(&json!({"line_height": 1.68}));
        assert_eq!(p.line_height, 1.7);
        let p = sanitize_prefs(&json!({"line_height": 1.3}));
        assert_eq!(p.line_height, 1.3);
        let p = sanitize_prefs(&json!({"font_size": 17.6}));
        assert_eq!(p.font_size, 18);
    }

    #[test]
    fn enum_invalido_e_tipo_errado_viram_padrao() {
        let p = sanitize_prefs(&json!({
            "font_family": "Comic Sans; } body { display:none",
            "align": 3,
            "theme": "Dark",          // diferenca de caixa nao e aceita
            "direction": null,
            "spread_mode": ["double"],
            "fit": "cover",
            "bg": "url(javascript:alert(1))",
            "hyphenate": "true",
            "cover_alone": 0,
            "zero_margin": "yes",
            "font_size": "20",
            "line_height": "1.5",
            "margin_pct": true
        }));
        assert_eq!(p, ReaderPrefs::default());
    }

    #[test]
    fn enums_validos_sao_aceitos() {
        let p = sanitize_prefs(&json!({
            "font_family": "mono", "align": "left", "theme": "black", "direction": "rtl",
            "spread_mode": "auto", "fit": "original", "bg": "light",
            "hyphenate": true, "cover_alone": false, "zero_margin": true
        }));
        assert_eq!(p.font_family, FontFamily::Mono);
        assert_eq!(p.align, Align::Left);
        assert_eq!(p.theme, Theme::Black);
        assert_eq!(p.direction, Direction::Rtl);
        assert_eq!(p.spread_mode, SpreadMode::Auto);
        assert_eq!(p.fit, Fit::Original);
        assert_eq!(p.bg, Background::Light);
        assert!(p.hyphenate && !p.cover_alone && p.zero_margin);
    }

    #[test]
    fn nan_e_infinito_viram_padrao() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(finite_clamp(bad, 7.0, 0.0, 10.0), 7.0);
        }
        assert_eq!(finite_clamp(11.0, 7.0, 0.0, 10.0), 10.0);
        assert_eq!(finite_clamp(-1.0, 7.0, 0.0, 10.0), 0.0);
        // Numero que estoura f64 no parse nunca produz valor fora da faixa.
        let v: Value = serde_json::from_str(r#"{"font_size": 1e999}"#).unwrap_or(Value::Null);
        let p = sanitize_prefs(&v);
        assert!((12..=36).contains(&p.font_size));
    }

    #[test]
    fn campos_desconhecidos_sao_ignorados() {
        let p = sanitize_prefs(&json!({
            "font_size": 20, "evil": "<script>", "__proto__": {"x": 1}
        }));
        assert_eq!(p.font_size, 20);
        let v = serde_json::to_value(&p).unwrap();
        assert!(v.get("evil").is_none());
        assert_eq!(v.as_object().unwrap().len(), 13);
    }

    #[test]
    fn nome_de_preset_invalido() {
        assert!(validate_preset_name("").is_err());
        assert!(validate_preset_name("   ").is_err());
        assert!(validate_preset_name(&"a".repeat(25)).is_err());
        assert!(validate_preset_name("a<b").is_err());
        assert!(validate_preset_name("a>b").is_err());
        assert!(validate_preset_name("a\nb").is_err());
        assert!(validate_preset_name("a\u{0}b").is_err());
        assert!(validate_preset_name("a\tb").is_err());
        // Limites validos (chars, nao bytes) e trim
        assert_eq!(
            validate_preset_name(&"a".repeat(24)).unwrap(),
            "a".repeat(24)
        );
        assert!(validate_preset_name(&"é".repeat(24)).is_ok());
        assert_eq!(validate_preset_name("  Noite  ").unwrap(), "Noite");
        // Operacao tambem rejeita
        let d = dir();
        assert!(op_save_preset(d.path(), BOOK_A, "<b>", &json!({})).is_err());
        assert!(!d.path().join(PREFS_FILENAME).exists());
    }

    #[test]
    fn limite_de_12_presets_e_substituicao() {
        let d = dir();
        for i in 0..12 {
            op_save_preset(
                d.path(),
                BOOK_A,
                &format!("p{i}"),
                &json!({"font_size": 20}),
            )
            .unwrap();
        }
        // 13o distinto => Err
        assert!(op_save_preset(d.path(), BOOK_A, "p12", &json!({})).is_err());
        assert_eq!(op_get(d.path(), BOOK_A).unwrap().presets.len(), 12);
        // Substituir existente nao conta
        let v = op_save_preset(d.path(), BOOK_A, " p3 ", &json!({"font_size": 30})).unwrap();
        assert_eq!(v.presets.len(), 12);
        let p3 = v.presets.iter().find(|p| p.name == "p3").unwrap();
        assert_eq!(p3.prefs.font_size, 30);
        // Apagar libera espaco
        let v = op_delete_preset(d.path(), BOOK_A, "p0").unwrap();
        assert_eq!(v.presets.len(), 11);
        assert!(op_save_preset(d.path(), BOOK_A, "p12", &json!({})).is_ok());
    }

    #[test]
    fn apagar_preset_inexistente_e_ok() {
        let d = dir();
        let v = op_delete_preset(d.path(), BOOK_A, "nao-existe").unwrap();
        assert!(v.presets.is_empty());
        // Nao criou arquivo desnecessariamente
        assert!(!d.path().join(PREFS_FILENAME).exists());
    }

    #[test]
    fn json_corrompido_vira_padrao_sem_sobrescrever() {
        let d = dir();
        let path = d.path().join(PREFS_FILENAME);
        std::fs::write(&path, b"{ isto nao e json").unwrap();
        let v = op_get(d.path(), BOOK_A).unwrap();
        assert_eq!(v.prefs, ReaderPrefs::default());
        assert!(!v.is_custom);
        assert_eq!(v.defaults, ReaderPrefs::default());
        assert!(v.presets.is_empty());
        // Leitura nao reescreve o arquivo corrompido
        assert_eq!(std::fs::read(&path).unwrap(), b"{ isto nao e json");
        // Bytes nao-UTF8
        std::fs::write(&path, [0xff, 0xfe, 0x00, 0x01]).unwrap();
        assert_eq!(
            op_get(d.path(), BOOK_A).unwrap().prefs,
            ReaderPrefs::default()
        );
        // Escrita legitima substitui
        op_save_book(d.path(), BOOK_A, &json!({"font_size": 22})).unwrap();
        assert_eq!(op_get(d.path(), BOOK_A).unwrap().prefs.font_size, 22);
    }

    #[test]
    fn versao_desconhecida_ou_formato_errado_vira_padrao() {
        let d = dir();
        let path = d.path().join(PREFS_FILENAME);
        let body = json!({"version": 2, "defaults": {"font_size": 30}});
        std::fs::write(&path, body.to_string()).unwrap();
        assert_eq!(op_get(d.path(), BOOK_A).unwrap().defaults.font_size, 18);
        std::fs::write(&path, json!({"defaults": {"font_size": 30}}).to_string()).unwrap();
        assert_eq!(op_get(d.path(), BOOK_A).unwrap().defaults.font_size, 18);
        std::fs::write(&path, "[1,2,3]").unwrap();
        assert_eq!(op_get(d.path(), BOOK_A).unwrap().defaults.font_size, 18);
        std::fs::write(&path, "").unwrap();
        assert_eq!(op_get(d.path(), BOOK_A).unwrap().defaults.font_size, 18);
    }

    #[test]
    fn arquivo_adulterado_e_sanitizado_na_leitura() {
        let d = dir();
        let body = json!({
            "version": 1,
            "defaults": {"font_size": 500, "theme": "<script>"},
            "progress": {"page_format": 99, "chapter": "x"},
            "presets": [
                {"name": "<x>", "prefs": {}},
                {"name": "ok", "prefs": {"fit": "width", "extra": 1}},
                {"name": "ok", "prefs": {"fit": "height"}},
                "lixo", {"prefs": {}}
            ],
            "books": {
                "../../etc/passwd": {"font_size": 20},
                BOOK_A: {"font_size": 0, "bg": "evil"},
                BOOK_B: 42
            }
        });
        std::fs::write(d.path().join(PREFS_FILENAME), body.to_string()).unwrap();
        let v = op_get(d.path(), BOOK_A).unwrap();
        assert_eq!(v.defaults.font_size, 36);
        assert_eq!(v.defaults.theme, Theme::Auto);
        assert_eq!(v.progress.page_format, 2);
        assert!(v.progress.chapter);
        assert_eq!(v.presets.len(), 1);
        assert_eq!(v.presets[0].name, "ok");
        assert_eq!(v.presets[0].prefs.fit, Fit::Width);
        assert!(v.is_custom);
        assert_eq!(v.prefs.font_size, 12);
        assert_eq!(v.prefs.bg, Background::Theme);
        let file = load_file(d.path());
        assert_eq!(file.books.len(), 2);
        assert!(!file.books.contains_key("../../etc/passwd"));
    }

    #[test]
    fn escrita_atomica_sem_tmp_residual_e_roundtrip() {
        let d = dir();
        let tmp = d.path().join(PREFS_TMP_FILENAME);
        let path = d.path().join(PREFS_FILENAME);
        op_save_book(
            d.path(),
            BOOK_A,
            &json!({"font_size": 24, "theme": "sepia"}),
        )
        .unwrap();
        assert!(!tmp.exists());
        assert!(path.exists());
        let raw: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw["version"], 1);
        assert_eq!(raw["books"][BOOK_A]["font_size"], 24);
        assert_eq!(raw["books"][BOOK_A]["theme"], "sepia");
        let v = op_get(d.path(), BOOK_A).unwrap();
        assert_eq!(v.prefs.font_size, 24);
        assert_eq!(v.prefs.theme, Theme::Sepia);
        // Roundtrip completo do arquivo
        let file = load_file(d.path());
        save_file(d.path(), &file).unwrap();
        assert_eq!(load_file(d.path()), file);
        assert!(!tmp.exists());
    }

    #[test]
    fn tmp_sujo_nao_afeta_arquivo_anterior_nem_a_proxima_escrita() {
        let d = dir();
        let tmp = d.path().join(PREFS_TMP_FILENAME);
        op_save_book(d.path(), BOOK_A, &json!({"font_size": 20})).unwrap();
        // Simula queda no meio de uma escrita: tmp truncado/sujo
        std::fs::write(&tmp, b"{\"version\": 1, \"books\": {trunc").unwrap();
        // O arquivo anterior continua integro e e o que se le
        assert_eq!(op_get(d.path(), BOOK_A).unwrap().prefs.font_size, 20);
        // A proxima escrita ignora o lixo, e nao sobra .tmp
        op_save_book(d.path(), BOOK_A, &json!({"font_size": 26})).unwrap();
        assert!(!tmp.exists());
        assert_eq!(op_get(d.path(), BOOK_A).unwrap().prefs.font_size, 26);
    }

    #[test]
    fn falha_na_escrita_preserva_arquivo_anterior() {
        let d = dir();
        let path = d.path().join(PREFS_FILENAME);
        op_save_book(d.path(), BOOK_A, &json!({"font_size": 20})).unwrap();
        let before = std::fs::read(&path).unwrap();
        // Um diretorio no lugar do .tmp faz a criacao falhar
        let tmp = d.path().join(PREFS_TMP_FILENAME);
        std::fs::create_dir(&tmp).unwrap();
        assert!(op_save_book(d.path(), BOOK_A, &json!({"font_size": 30})).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn prefs_de_um_livro_nao_vazam_para_outro() {
        let d = dir();
        op_save_book(d.path(), BOOK_A, &json!({"font_size": 30, "theme": "dark"})).unwrap();
        let b = op_get(d.path(), BOOK_B).unwrap();
        assert!(!b.is_custom);
        assert_eq!(b.prefs, ReaderPrefs::default());
        let a = op_get(d.path(), BOOK_A).unwrap();
        assert!(a.is_custom);
        assert_eq!(a.prefs.font_size, 30);
        op_save_book(d.path(), BOOK_B, &json!({"font_size": 14})).unwrap();
        assert_eq!(op_get(d.path(), BOOK_A).unwrap().prefs.font_size, 30);
        assert_eq!(op_get(d.path(), BOOK_B).unwrap().prefs.font_size, 14);
        // O campo defaults da visao nao e contaminado pelo livro
        assert_eq!(op_get(d.path(), BOOK_A).unwrap().defaults.font_size, 18);
    }

    #[test]
    fn defaults_aplicados_a_livro_sem_entrada() {
        let d = dir();
        let v =
            op_set_default(d.path(), BOOK_A, &json!({"font_size": 28, "fit": "width"})).unwrap();
        // Livro A nao tem entrada: prefs == defaults, nao custom
        assert!(!v.is_custom);
        assert_eq!(v.prefs, v.defaults);
        assert_eq!(v.prefs.font_size, 28);
        let b = op_get(d.path(), BOOK_B).unwrap();
        assert_eq!(b.prefs.font_size, 28);
        assert_eq!(b.prefs.fit, Fit::Width);
        assert!(!b.is_custom);
    }

    #[test]
    fn set_default_nao_altera_livros_salvos() {
        let d = dir();
        op_save_book(d.path(), BOOK_A, &json!({"font_size": 30})).unwrap();
        op_set_default(d.path(), BOOK_B, &json!({"font_size": 14})).unwrap();
        let a = op_get(d.path(), BOOK_A).unwrap();
        assert!(a.is_custom);
        assert_eq!(a.prefs.font_size, 30);
        assert_eq!(a.defaults.font_size, 14);
        assert_eq!(op_get(d.path(), BOOK_B).unwrap().prefs.font_size, 14);
    }

    #[test]
    fn reset_remove_entrada_do_livro() {
        let d = dir();
        op_set_default(d.path(), BOOK_A, &json!({"font_size": 22})).unwrap();
        op_save_book(d.path(), BOOK_A, &json!({"font_size": 30})).unwrap();
        op_save_book(d.path(), BOOK_B, &json!({"font_size": 31})).unwrap();
        let v = op_reset_book(d.path(), BOOK_A).unwrap();
        assert!(!v.is_custom);
        assert_eq!(v.prefs.font_size, 22);
        // Outro livro intacto
        assert_eq!(op_get(d.path(), BOOK_B).unwrap().prefs.font_size, 31);
        // Reset repetido e Ok
        assert!(op_reset_book(d.path(), BOOK_A).is_ok());
    }

    #[test]
    fn book_id_invalido_e_rejeitado() {
        let d = dir();
        let bad = [
            "",
            "../x",
            "a/b",
            "a\\b",
            "a b",
            "a%2e",
            "a.b",
            "é",
            &"a".repeat(129),
        ];
        for id in bad {
            assert!(op_get(d.path(), id).is_err(), "{id:?}");
            assert!(op_save_book(d.path(), id, &json!({})).is_err(), "{id:?}");
            assert!(op_reset_book(d.path(), id).is_err(), "{id:?}");
            assert!(op_set_default(d.path(), id, &json!({})).is_err(), "{id:?}");
            assert!(
                op_save_preset(d.path(), id, "n", &json!({})).is_err(),
                "{id:?}"
            );
            assert!(op_delete_preset(d.path(), id, "n").is_err(), "{id:?}");
            assert!(
                op_save_progress(d.path(), id, &json!({})).is_err(),
                "{id:?}"
            );
        }
        assert!(!d.path().join(PREFS_FILENAME).exists());
        assert!(op_get(d.path(), &"a".repeat(128)).is_ok());
        assert!(op_get(d.path(), "abc_DEF-123").is_ok());
    }

    #[test]
    fn limite_de_livros() {
        let d = dir();
        let mut file = PrefsFile::default();
        for i in 0..MAX_BOOKS {
            file.books.insert(format!("b{i}"), ReaderPrefs::default());
        }
        save_file(d.path(), &file).unwrap();
        // Livro novo acima do limite => Err
        assert!(op_save_book(d.path(), "novo", &json!({})).is_err());
        // Livro existente continua podendo ser atualizado
        let v = op_save_book(d.path(), "b7", &json!({"font_size": 20})).unwrap();
        assert_eq!(v.prefs.font_size, 20);
        assert_eq!(load_file(d.path()).books.len(), MAX_BOOKS);
    }

    #[test]
    fn progress_prefs_sao_globais_e_sanitizadas() {
        let d = dir();
        let v = op_save_progress(
            d.path(),
            BOOK_A,
            &json!({"chapter": false, "pages_left": false, "page_format": 1, "x": 1}),
        )
        .unwrap();
        assert!(!v.progress.chapter);
        assert!(v.progress.book_percent);
        assert!(!v.progress.pages_left);
        assert_eq!(v.progress.page_format, 1);
        // Visivel de outro livro (e global)
        assert_eq!(op_get(d.path(), BOOK_B).unwrap().progress, v.progress);
    }

    #[test]
    fn save_book_sanitiza_e_devolve_valores_limpos() {
        let d = dir();
        let v = op_save_book(
            d.path(),
            BOOK_A,
            &json!({"font_size": 1000, "font_family": "evil", "line_height": 1.234}),
        )
        .unwrap();
        assert_eq!(v.prefs.font_size, 36);
        assert_eq!(v.prefs.font_family, FontFamily::Book);
        assert_eq!(v.prefs.line_height, 1.25);
        let persisted = op_get(d.path(), BOOK_A).unwrap();
        assert_eq!(persisted.prefs, v.prefs);
    }
}

#![forbid(unsafe_code)]

use md5::Md5;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use thiserror::Error;

/// Versao estavel da API HTTP do Bookferry.
pub const API_VERSION: &str = "1";

/// Versao do formato JSON do QR Code de pareamento.
pub const PAIRING_QR_VERSION: u32 = 1;

/// Deriva maxima permitida no futuro para o `wall_ms` de um HLC (24 horas em ms).
pub const DEFAULT_MAX_FUTURE_DRIFT_MS: u64 = 24 * 60 * 60 * 1000;

/// Limite maximo de caracteres Unicode no campo opcional `snippet`.
pub const MAX_SNIPPET_CHARS: usize = 120;

/// Limite maximo de bytes no campo `locator`.
pub const MAX_LOCATOR_BYTES: usize = 4096;

/// Quantidade maxima de entradas no historico de posicoes por livro.
pub const DEFAULT_POSITION_HISTORY_LIMIT: usize = 20;

/// Valor maximo de `wall_ms` representavel em 15 digitos decimais.
pub const MAX_HLC_WALL_MS: u64 = 999_999_999_999_999;

/// Valor maximo de `logical` representavel em 6 digitos decimais.
pub const MAX_HLC_LOGICAL: u32 = 999_999;

/// Erros de dominio e validacao pura do `bookferry-core`.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum CoreError {
    #[error("formato de HLC invalido: {0}")]
    InvalidHlcFormat(String),

    #[error("node_id de HLC invalido: {0}")]
    InvalidHlcNode(String),

    #[error("contador logico do HLC excedeu o limite ({MAX_HLC_LOGICAL})")]
    HlcLogicalOverflow,

    #[error("HLC muito no futuro: wall_ms={wall_ms} excede now_ms={now_ms} + max_drift_ms={max_drift_ms}")]
    HlcTooFarInFuture {
        wall_ms: u64,
        now_ms: u64,
        max_drift_ms: u64,
    },

    #[error("identificador de livro (book_id) invalido: {0}")]
    InvalidBookId(String),

    #[error("device_id invalido: nao pode ser vazio nem exceder 128 caracteres")]
    InvalidDeviceId,

    #[error("locator invalido: nao pode ser vazio nem exceder {MAX_LOCATOR_BYTES} bytes")]
    InvalidLocator,

    #[error("snippet excede o limite de {max} caracteres (recebido: {len})")]
    SnippetTooLong { len: usize, max: usize },

    #[error("percent invalido ({0}): deve ser finito e estar entre 0.0 e 100.0")]
    InvalidPercent(f64),

    #[error("falha ao gerar bytes aleatorios do sistema")]
    RandomGenerationFailed,
}

impl CoreError {
    /// Codigo de erro estavel para respostas da API HTTP.
    pub fn error_code(&self) -> &'static str {
        match self {
            CoreError::InvalidHlcFormat(_) | CoreError::InvalidHlcNode(_) => "invalid_hlc",
            CoreError::HlcLogicalOverflow => "hlc_logical_overflow",
            CoreError::HlcTooFarInFuture { .. } => "hlc_too_far_in_future",
            CoreError::InvalidBookId(_) => "invalid_book_id",
            CoreError::InvalidDeviceId => "invalid_device_id",
            CoreError::InvalidLocator => "invalid_locator",
            CoreError::SnippetTooLong { .. } => "snippet_too_long",
            CoreError::InvalidPercent(_) => "invalid_percent",
            CoreError::RandomGenerationFailed => "rng_failure",
        }
    }
}

// ============================================================================
// Primitivas Criptograficas e Redacao
// ============================================================================

/// Codifica bytes como string hexadecimal lowercase.
pub fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// Calcula o hash SHA-256 e retorna como string hexadecimal lowercase (64 chars).
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_encode(&hasher.finalize())
}

/// Gera `num_bytes` criptograficamente seguros (CSPRNG) e codifica em hex lowercase.
pub fn generate_random_hex(num_bytes: usize) -> Result<String, CoreError> {
    let mut buf = vec![0u8; num_bytes];
    getrandom::getrandom(&mut buf).map_err(|_| CoreError::RandomGenerationFailed)?;
    Ok(hex_encode(&buf))
}

/// Compara duas strings em tempo constante usando `subtle::ConstantTimeEq`.
///
/// Compara tanto os digests SHA-256 (32 bytes fixos) quanto os comprimentos
/// em tempo constante para evitar vazamento por canal lateral de tempo.
pub fn constant_time_eq_str(a: &str, b: &str) -> bool {
    let hash_a = Sha256::digest(a.as_bytes());
    let hash_b = Sha256::digest(b.as_bytes());
    let hashes_eq = hash_a.as_slice().ct_eq(hash_b.as_slice());
    let len_eq = a.len().ct_eq(&b.len());
    bool::from(hashes_eq & len_eq)
}

// ============================================================================
// Relogio Injetavel e HLC (Hybrid Logical Clock)
// ============================================================================

/// Abstrai a leitura de tempo fisico (Unix epoch em milissegundos) para permitir
/// testes deterministas de expiracao, rate-limiting e HLC.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// Relogio fisico real do sistema operacional.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

/// Relogio controlado manualmente para testes automatizados.
#[derive(Debug)]
pub struct ManualClock {
    now_ms: AtomicU64,
}

impl ManualClock {
    pub fn new(initial_ms: u64) -> Self {
        Self {
            now_ms: AtomicU64::new(initial_ms),
        }
    }

    pub fn set_ms(&self, ms: u64) {
        self.now_ms.store(ms, AtomicOrdering::SeqCst);
    }

    pub fn advance_ms(&self, delta_ms: u64) -> u64 {
        self.now_ms.fetch_add(delta_ms, AtomicOrdering::SeqCst) + delta_ms
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(AtomicOrdering::SeqCst)
    }
}

impl<T: Clock + ?Sized> Clock for Arc<T> {
    fn now_ms(&self) -> u64 {
        (**self).now_ms()
    }
}

/// Carimbo de tempo de Relogio Logico Hibrido (HLC).
///
/// Ordem total: `(wall_ms, logical, node)`.
/// Codificacao textual canonica ordenavel lexicograficamente:
/// `{wall_ms:015}:{logical:06}:{node}`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Hlc {
    pub wall_ms: u64,
    pub logical: u32,
    pub node: String,
}

impl Hlc {
    pub fn new(wall_ms: u64, logical: u32, node: impl Into<String>) -> Result<Self, CoreError> {
        let node = node.into();
        Self::validate_components(wall_ms, logical, &node)?;
        Ok(Self {
            wall_ms,
            logical,
            node,
        })
    }

    fn validate_components(wall_ms: u64, logical: u32, node: &str) -> Result<(), CoreError> {
        if wall_ms > MAX_HLC_WALL_MS {
            return Err(CoreError::InvalidHlcFormat(format!(
                "wall_ms ({wall_ms}) excede {MAX_HLC_WALL_MS}"
            )));
        }
        if logical > MAX_HLC_LOGICAL {
            return Err(CoreError::HlcLogicalOverflow);
        }
        if node.is_empty()
            || node.len() > 64
            || node.contains(':')
            || !node
                .chars()
                .all(|c| c.is_ascii_graphic() && !c.is_ascii_control())
        {
            return Err(CoreError::InvalidHlcNode(node.to_string()));
        }
        Ok(())
    }

    /// Analisa uma string HLC no formato `<wall_ms>:<logical>:<node>`.
    pub fn parse(raw: &str) -> Result<Self, CoreError> {
        let raw = raw.trim();
        let mut parts = raw.splitn(3, ':');
        let wall_str = parts
            .next()
            .ok_or_else(|| CoreError::InvalidHlcFormat(raw.to_string()))?;
        let logical_str = parts
            .next()
            .ok_or_else(|| CoreError::InvalidHlcFormat(raw.to_string()))?;
        let node_str = parts
            .next()
            .ok_or_else(|| CoreError::InvalidHlcFormat(raw.to_string()))?;

        if wall_str.is_empty() || !wall_str.bytes().all(|b| b.is_ascii_digit()) {
            return Err(CoreError::InvalidHlcFormat(raw.to_string()));
        }
        if logical_str.is_empty() || !logical_str.bytes().all(|b| b.is_ascii_digit()) {
            return Err(CoreError::InvalidHlcFormat(raw.to_string()));
        }

        let wall_ms = wall_str
            .parse::<u64>()
            .map_err(|_| CoreError::InvalidHlcFormat(raw.to_string()))?;
        let logical = logical_str
            .parse::<u32>()
            .map_err(|_| CoreError::InvalidHlcFormat(raw.to_string()))?;

        Self::new(wall_ms, logical, node_str)
    }

    /// Codificacao textual canonica de largura fixa, preservando a ordem total por comparacao
    /// lexicografica ASCII/UTF-8 (`{wall_ms:015}:{logical:06}:{node}`).
    pub fn to_canonical_string(&self) -> String {
        format!("{:015}:{:06}:{}", self.wall_ms, self.logical, self.node)
    }

    /// Verifica se `wall_ms` nao esta excessivamente no futuro em relacao a `now_ms`.
    pub fn validate_drift(&self, now_ms: u64, max_drift_ms: u64) -> Result<(), CoreError> {
        let max_allowed = now_ms.saturating_add(max_drift_ms);
        if self.wall_ms > max_allowed {
            return Err(CoreError::HlcTooFarInFuture {
                wall_ms: self.wall_ms,
                now_ms,
                max_drift_ms,
            });
        }
        Ok(())
    }
}

impl fmt::Display for Hlc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_canonical_string())
    }
}

impl FromStr for Hlc {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Ord for Hlc {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.wall_ms, self.logical, self.node.as_str()).cmp(&(
            other.wall_ms,
            other.logical,
            other.node.as_str(),
        ))
    }
}

impl PartialOrd for Hlc {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Gerador e receptor de carimbos HLC seguindo o algoritmo padrao de Kulkarni et al.
pub struct HlcClock<C: Clock = SystemClock> {
    node: String,
    clock: C,
    state: Mutex<(u64, u32)>,
}

impl HlcClock<SystemClock> {
    pub fn new_system(node: impl Into<String>) -> Result<Self, CoreError> {
        Self::with_clock(node, SystemClock)
    }
}

impl<C: Clock> HlcClock<C> {
    pub fn with_clock(node: impl Into<String>, clock: C) -> Result<Self, CoreError> {
        let node = node.into();
        Hlc::validate_components(0, 0, &node)?;
        Ok(Self {
            node,
            clock,
            state: Mutex::new((0, 0)),
        })
    }

    pub fn node(&self) -> &str {
        &self.node
    }

    pub fn clock(&self) -> &C {
        &self.clock
    }

    /// Regra HLC de envio / evento local:
    /// - `l' = max(l, pt)`
    /// - Se `l' == l`, `c' = c + 1`, senao `c' = 0`.
    pub fn now(&self) -> Result<Hlc, CoreError> {
        let pt = self.clock.now_ms();
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (l_prev, c_prev) = *st;
        let (l_new, c_new) = if pt > l_prev {
            (pt, 0)
        } else {
            let next_c = c_prev.checked_add(1).ok_or(CoreError::HlcLogicalOverflow)?;
            if next_c > MAX_HLC_LOGICAL {
                return Err(CoreError::HlcLogicalOverflow);
            }
            (l_prev, next_c)
        };
        *st = (l_new, c_new);
        Hlc::new(l_new, c_new, self.node.clone())
    }

    /// Regra HLC de recebimento de mensagem remota `remote`:
    /// 1. Valida que `remote.wall_ms <= pt + max_drift_ms`.
    /// 2. `l' = max(l, remote.wall_ms, pt)`
    /// 3. Atualiza o contador logico conforme qual componente dominou `l'`.
    pub fn receive(&self, remote: &Hlc, max_drift_ms: u64) -> Result<Hlc, CoreError> {
        let pt = self.clock.now_ms();
        remote.validate_drift(pt, max_drift_ms)?;

        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (l_prev, c_prev) = *st;
        let l_new = l_prev.max(remote.wall_ms).max(pt);

        let c_new = if l_new == l_prev && l_new == remote.wall_ms {
            c_prev
                .max(remote.logical)
                .checked_add(1)
                .ok_or(CoreError::HlcLogicalOverflow)?
        } else if l_new == l_prev {
            c_prev.checked_add(1).ok_or(CoreError::HlcLogicalOverflow)?
        } else if l_new == remote.wall_ms {
            remote
                .logical
                .checked_add(1)
                .ok_or(CoreError::HlcLogicalOverflow)?
        } else {
            0
        };

        if c_new > MAX_HLC_LOGICAL {
            return Err(CoreError::HlcLogicalOverflow);
        }

        *st = (l_new, c_new);
        Hlc::new(l_new, c_new, self.node.clone())
    }
}

// ============================================================================
// BookId e Partial MD5 estilo KOReader
// ============================================================================

/// Formatos de livro suportados na Fatia 1 do MVP.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BookFormat {
    Epub,
    Pdf,
    Cbz,
}

impl BookFormat {
    /// Detecta o formato a partir da extensao do arquivo (case-insensitive).
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?;
        Self::from_extension(ext)
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        if ext.eq_ignore_ascii_case("epub") {
            Some(Self::Epub)
        } else if ext.eq_ignore_ascii_case("pdf") {
            Some(Self::Pdf)
        } else if ext.eq_ignore_ascii_case("cbz") {
            Some(Self::Cbz)
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Epub => "epub",
            Self::Pdf => "pdf",
            Self::Cbz => "cbz",
        }
    }

    pub fn content_type(&self) -> &'static str {
        match self {
            Self::Epub => "application/epub+zip",
            Self::Pdf => "application/pdf",
            Self::Cbz => "application/vnd.comicbook+zip",
        }
    }
}

/// Offsets exatos em bytes usados pelo algoritmo Partial MD5 estilo KOReader:
/// `1024 << (2 * i)` para `i = -1..=10`:
/// `[0, 1024, 4096, 16384, 65536, 262144, 1048576, 4194304, 16777216, 67108864, 268435456, 1073741824]`.
pub const KOREADER_PARTIAL_MD5_OFFSETS: [u64; 12] = [
    0,
    1024,
    4096,
    16384,
    65536,
    262144,
    1_048_576,
    4_194_304,
    16_777_216,
    67_108_864,
    268_435_456,
    1_073_741_824,
];

/// Tamanho de cada bloco amostrado no Partial MD5 (1024 bytes = 1 KiB).
pub const KOREADER_SAMPLE_SIZE: usize = 1024;

/// Calcula o Partial MD5 estilo KOReader lendo ate 1024 bytes em cada um dos 12 offsets
/// `KOREADER_PARTIAL_MD5_OFFSETS`.
pub fn compute_partial_md5<R: Read + Seek>(reader: &mut R) -> std::io::Result<String> {
    let mut hasher = Md5::new();
    let mut buf = [0u8; KOREADER_SAMPLE_SIZE];

    for &offset in &KOREADER_PARTIAL_MD5_OFFSETS {
        reader.seek(SeekFrom::Start(offset))?;
        let mut read_total = 0usize;
        while read_total < KOREADER_SAMPLE_SIZE {
            let n = reader.read(&mut buf[read_total..])?;
            if n == 0 {
                break;
            }
            read_total += n;
        }
        if read_total > 0 {
            hasher.update(&buf[..read_total]);
        } else {
            break;
        }
    }

    Ok(hex_encode(&hasher.finalize()))
}

/// Identificador estavel de livro no formato `{partial_md5}-{size_bytes}`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BookId(String);

impl BookId {
    pub fn from_parts(partial_md5: &str, size_bytes: u64) -> Result<Self, CoreError> {
        let candidate = format!("{partial_md5}-{size_bytes}");
        Self::parse(&candidate)
    }

    pub fn from_reader<R: Read + Seek>(
        reader: &mut R,
        size_bytes: u64,
    ) -> std::io::Result<(Self, String)> {
        let partial_md5 = compute_partial_md5(reader)?;
        let id = Self(format!("{partial_md5}-{size_bytes}"));
        Ok((id, partial_md5))
    }

    pub fn from_file(path: &Path) -> std::io::Result<(Self, String, u64)> {
        let mut file = File::open(path)?;
        let size_bytes = file.metadata()?.len();
        let (id, partial_md5) = Self::from_reader(&mut file, size_bytes)?;
        Ok((id, partial_md5, size_bytes))
    }

    /// Valida estritamente um `book_id` fornecido pelo cliente:
    /// deve ser `<32 digitos hex lowercase>-<digitos decimais u64>`.
    /// Rejeita qualquer tentativa de path traversal (`..`, `/`, `\`, `%`, espacos, etc.).
    pub fn parse(raw: &str) -> Result<Self, CoreError> {
        let (md5_part, size_part) = raw
            .split_once('-')
            .ok_or_else(|| CoreError::InvalidBookId(raw.to_string()))?;

        if md5_part.len() != 32
            || !md5_part
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(CoreError::InvalidBookId(raw.to_string()));
        }

        if size_part.is_empty() || !size_part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(CoreError::InvalidBookId(raw.to_string()));
        }

        let _size = size_part
            .parse::<u64>()
            .map_err(|_| CoreError::InvalidBookId(raw.to_string()))?;

        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BookId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for BookId {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

// ============================================================================
// Posicao de Leitura e Resolucao de Conflitos LWW
// ============================================================================

/// Payload de entrada em `PUT /books/{id}/position`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PositionUpdate {
    pub device_id: String,
    pub hlc: String,
    pub locator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    pub percent: f64,
}

/// Posicao validada pronta para avaliacao LWW e persistencia.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedPositionUpdate {
    pub device_id: String,
    pub hlc: Hlc,
    pub locator: String,
    pub snippet: Option<String>,
    pub percent: f64,
}

impl PositionUpdate {
    /// Valida todos os campos de `PositionUpdate` e a deriva futura do HLC.
    pub fn validate(
        &self,
        now_ms: u64,
        max_future_drift_ms: u64,
    ) -> Result<ValidatedPositionUpdate, CoreError> {
        let device_id = self.device_id.trim();
        if device_id.is_empty()
            || device_id.len() > 128
            || device_id.chars().any(|c| c.is_control())
        {
            return Err(CoreError::InvalidDeviceId);
        }

        let locator = self.locator.trim();
        if locator.is_empty() || locator.len() > MAX_LOCATOR_BYTES {
            return Err(CoreError::InvalidLocator);
        }

        if let Some(ref s) = self.snippet {
            let char_count = s.chars().count();
            if char_count > MAX_SNIPPET_CHARS {
                return Err(CoreError::SnippetTooLong {
                    len: char_count,
                    max: MAX_SNIPPET_CHARS,
                });
            }
        }

        if !self.percent.is_finite() || !(0.0..=100.0).contains(&self.percent) {
            return Err(CoreError::InvalidPercent(self.percent));
        }

        let hlc = Hlc::parse(&self.hlc)?;
        hlc.validate_drift(now_ms, max_future_drift_ms)?;

        Ok(ValidatedPositionUpdate {
            device_id: device_id.to_string(),
            hlc,
            locator: locator.to_string(),
            snippet: self.snippet.clone(),
            percent: self.percent,
        })
    }
}

/// Posicao persistida (vencedora atual) de um livro.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PositionRecord {
    pub book_id: String,
    pub device_id: String,
    pub hlc: String,
    pub hlc_wall_ms: u64,
    pub hlc_logical: u32,
    pub hlc_node: String,
    pub locator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    pub percent: f64,
    pub server_seq: u64,
    pub updated_at_ms: u64,
}

/// Item do historico de posicoes de um livro (aplicadas e rejeitadas por LWW).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PositionHistoryEntry {
    pub book_id: String,
    pub device_id: String,
    pub hlc: String,
    pub locator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    pub percent: f64,
    pub applied: bool,
    pub server_seq: u64,
    pub recorded_at_ms: u64,
}

/// Decisao pura da regra Last-Writer-Wins (LWW) por HLC total.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LwwDecision {
    /// O HLC recebido e estritamente maior que o atual (ou nao havia posicao anterior).
    Apply,
    /// O HLC recebido e menor ou igual ao atual; nao sobrescreve a posicao vencedora.
    RejectLate,
}

/// Compara o HLC vencedor atual (se existir) com o HLC recebido.
pub fn decide_lww(current_winner: Option<&Hlc>, incoming: &Hlc) -> LwwDecision {
    match current_winner {
        None => LwwDecision::Apply,
        Some(curr) => {
            if incoming > curr {
                LwwDecision::Apply
            } else {
                LwwDecision::RejectLate
            }
        }
    }
}

/// Registrador LWW em memoria para um livro (usado em logica pura e testes de propriedades),
/// mantendo a posicao vencedora e um historico limitado das ultimas `history_limit` escritas.
#[derive(Clone, Debug)]
pub struct LwwBookRegister {
    book_id: String,
    winner: Option<(Hlc, PositionRecord)>,
    history: VecDeque<PositionHistoryEntry>,
    history_limit: usize,
    next_seq: u64,
}

impl LwwBookRegister {
    pub fn new(book_id: impl Into<String>, history_limit: usize) -> Self {
        Self {
            book_id: book_id.into(),
            winner: None,
            history: VecDeque::with_capacity(history_limit.max(1)),
            history_limit: history_limit.max(1),
            next_seq: 1,
        }
    }

    pub fn winner(&self) -> Option<&PositionRecord> {
        self.winner.as_ref().map(|(_, rec)| rec)
    }

    pub fn history(&self) -> &VecDeque<PositionHistoryEntry> {
        &self.history
    }

    pub fn apply_validated(
        &mut self,
        update: ValidatedPositionUpdate,
        now_ms: u64,
    ) -> (bool, PositionRecord) {
        let seq = self.next_seq;
        self.next_seq += 1;

        let decision = decide_lww(self.winner.as_ref().map(|(h, _)| h), &update.hlc);
        let applied = decision == LwwDecision::Apply;
        let canonical = update.hlc.to_canonical_string();

        let history_entry = PositionHistoryEntry {
            book_id: self.book_id.clone(),
            device_id: update.device_id.clone(),
            hlc: canonical.clone(),
            locator: update.locator.clone(),
            snippet: update.snippet.clone(),
            percent: update.percent,
            applied,
            server_seq: seq,
            recorded_at_ms: now_ms,
        };

        self.history.push_front(history_entry);
        while self.history.len() > self.history_limit {
            self.history.pop_back();
        }

        if applied {
            let record = PositionRecord {
                book_id: self.book_id.clone(),
                device_id: update.device_id,
                hlc: canonical,
                hlc_wall_ms: update.hlc.wall_ms,
                hlc_logical: update.hlc.logical,
                hlc_node: update.hlc.node.clone(),
                locator: update.locator,
                snippet: update.snippet,
                percent: update.percent,
                server_seq: seq,
                updated_at_ms: now_ms,
            };
            self.winner = Some((update.hlc, record.clone()));
            (true, record)
        } else {
            let current = self
                .winner
                .as_ref()
                .map(|(_, rec)| rec.clone())
                .expect("winner must exist when decision is RejectLate");
            (false, current)
        }
    }
}

// ============================================================================
// Payload de Pareamento (QR Code) com Redacao de Segredo em Debug
// ============================================================================

/// Payload JSON versionado contido no QR Code de pareamento.
///
/// Exemplo de JSON:
/// `{"v":1,"ips":["192.168.1.2"],"ip":"192.168.1.2","port":47600,"cert_sha256":"...","spki_sha256":"...","secret":"...","name":"Bookferry PC"}`
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingQrPayload {
    pub v: u32,
    pub ips: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    pub port: u16,
    pub cert_sha256: String,
    pub spki_sha256: String,
    pub secret: String,
    pub name: String,
}

impl PairingQrPayload {
    pub fn new(
        ips: Vec<String>,
        port: u16,
        cert_sha256: String,
        spki_sha256: String,
        secret: String,
        name: String,
    ) -> Self {
        let primary_ip = ips.first().cloned();
        Self {
            v: PAIRING_QR_VERSION,
            ips,
            ip: primary_ip,
            port,
            cert_sha256,
            spki_sha256,
            secret,
            name,
        }
    }

    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn from_json_str(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

impl fmt::Debug for PairingQrPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairingQrPayload")
            .field("v", &self.v)
            .field("ips", &self.ips)
            .field("ip", &self.ip)
            .field("port", &self.port)
            .field("cert_sha256", &self.cert_sha256)
            .field("spki_sha256", &self.spki_sha256)
            .field("secret", &"[REDACTED]")
            .field("name", &self.name)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::io::Cursor;

    #[test]
    fn hlc_canonical_roundtrip_and_order() {
        let a = Hlc::new(1_700_000_000_000, 0, "android").unwrap();
        let b = Hlc::new(1_700_000_000_000, 1, "android").unwrap();
        let c = Hlc::new(1_700_000_000_000, 1, "pc").unwrap();
        let d = Hlc::new(1_700_000_000_001, 0, "android").unwrap();

        assert!(a < b);
        assert!(b < c);
        assert!(c < d);

        assert!(a.to_canonical_string() < b.to_canonical_string());
        assert!(b.to_canonical_string() < c.to_canonical_string());
        assert!(c.to_canonical_string() < d.to_canonical_string());

        let parsed = Hlc::parse(&c.to_canonical_string()).unwrap();
        assert_eq!(parsed, c);
    }

    #[test]
    fn hlc_clock_monotonic_and_receive_advances() {
        let clock = Arc::new(ManualClock::new(1_000));
        let hlc_clock = HlcClock::with_clock("pc", clock.clone()).unwrap();

        let t1 = hlc_clock.now().unwrap();
        let t2 = hlc_clock.now().unwrap();
        assert_eq!(t1, Hlc::new(1_000, 0, "pc").unwrap());
        assert_eq!(t2, Hlc::new(1_000, 1, "pc").unwrap());
        assert!(t2 > t1);

        // Recebe mensagem de outro no com wall_ms mais adiantado (1_500, logical 4)
        let remote = Hlc::new(1_500, 4, "android").unwrap();
        let after_recv = hlc_clock
            .receive(&remote, DEFAULT_MAX_FUTURE_DRIFT_MS)
            .unwrap();
        assert_eq!(after_recv, Hlc::new(1_500, 5, "pc").unwrap());
        assert!(after_recv > remote);

        // Proximo evento local com relogio fisico ainda em 1_000 continua monotono
        let t3 = hlc_clock.now().unwrap();
        assert_eq!(t3, Hlc::new(1_500, 6, "pc").unwrap());
        assert!(t3 > after_recv);

        // Quando o relogio fisico passa de 1_500, zera o contador logico
        clock.set_ms(2_000);
        let t4 = hlc_clock.now().unwrap();
        assert_eq!(t4, Hlc::new(2_000, 0, "pc").unwrap());
        assert!(t4 > t3);
    }

    #[test]
    fn hlc_rejects_absurd_future_drift() {
        let clock = Arc::new(ManualClock::new(100_000));
        let hlc_clock = HlcClock::with_clock("server", clock).unwrap();
        let too_future = Hlc::new(
            100_000 + DEFAULT_MAX_FUTURE_DRIFT_MS + 1,
            0,
            "bad-clock-node",
        )
        .unwrap();

        let err = hlc_clock
            .receive(&too_future, DEFAULT_MAX_FUTURE_DRIFT_MS)
            .unwrap_err();
        assert_eq!(err.error_code(), "hlc_too_far_in_future");
    }

    #[test]
    fn lww_tie_breaking_and_history_retention() {
        let mut reg = LwwBookRegister::new("d41d8cd98f00b204e9800998ecf8427e-100", 20);
        let now = 5_000_000;

        // 1. Escrita inicial
        let u1 = PositionUpdate {
            device_id: "dev-b".into(),
            hlc: "5000000:0:dev-b".into(),
            locator: "page:10".into(),
            snippet: Some("Cap 1".into()),
            percent: 10.0,
        }
        .validate(now, DEFAULT_MAX_FUTURE_DRIFT_MS)
        .unwrap();
        let (applied, winner) = reg.apply_validated(u1, now);
        assert!(applied);
        assert_eq!(winner.locator, "page:10");

        // 2. Escrita fora de ordem (wall_ms menor) -> rejeitada, mas guardada no historico
        let u_late = PositionUpdate {
            device_id: "dev-a".into(),
            hlc: "4999999:99:dev-a".into(),
            locator: "page:5".into(),
            snippet: None,
            percent: 5.0,
        }
        .validate(now, DEFAULT_MAX_FUTURE_DRIFT_MS)
        .unwrap();
        let (applied, winner) = reg.apply_validated(u_late, now);
        assert!(!applied);
        assert_eq!(winner.locator, "page:10");

        // 3. Empate de wall_ms, vence maior contador logico
        let u_logical = PositionUpdate {
            device_id: "dev-a".into(),
            hlc: "5000000:1:dev-a".into(),
            locator: "page:12".into(),
            snippet: None,
            percent: 12.0,
        }
        .validate(now, DEFAULT_MAX_FUTURE_DRIFT_MS)
        .unwrap();
        let (applied, winner) = reg.apply_validated(u_logical, now);
        assert!(applied);
        assert_eq!(winner.locator, "page:12");

        // 4. Empate de wall_ms E logical, desempata por node_id ("dev-c" > "dev-a")
        let u_node_win = PositionUpdate {
            device_id: "dev-c".into(),
            hlc: "5000000:1:dev-c".into(),
            locator: "page:15".into(),
            snippet: None,
            percent: 15.0,
        }
        .validate(now, DEFAULT_MAX_FUTURE_DRIFT_MS)
        .unwrap();
        let (applied, winner) = reg.apply_validated(u_node_win, now);
        assert!(applied);
        assert_eq!(winner.locator, "page:15");

        // 5. Empate de wall_ms E logical com node_id menor ("dev-b" < "dev-c") -> rejeitada
        let u_node_lose = PositionUpdate {
            device_id: "dev-b".into(),
            hlc: "5000000:1:dev-b".into(),
            locator: "page:14".into(),
            snippet: None,
            percent: 14.0,
        }
        .validate(now, DEFAULT_MAX_FUTURE_DRIFT_MS)
        .unwrap();
        let (applied, winner) = reg.apply_validated(u_node_lose, now);
        assert!(!applied);
        assert_eq!(winner.locator, "page:15");

        // Preenche mais 25 entradas atrasadas e confirma limite de 20 no historico
        for i in 0..25 {
            let u = PositionUpdate {
                device_id: "dev-a".into(),
                hlc: format!("{}:{}:dev-a", 4_000_000 + i, 0),
                locator: format!("page:{i}"),
                snippet: None,
                percent: 1.0,
            }
            .validate(now, DEFAULT_MAX_FUTURE_DRIFT_MS)
            .unwrap();
            reg.apply_validated(u, now);
        }
        assert_eq!(reg.history().len(), 20);
        assert_eq!(reg.winner().unwrap().locator, "page:15");
    }

    #[test]
    fn book_id_partial_md5_and_path_traversal_validation() {
        let data = vec![b'A'; 10_000];
        let mut cursor = Cursor::new(&data);
        let (book_id, md5_hex) = BookId::from_reader(&mut cursor, data.len() as u64).unwrap();
        assert_eq!(md5_hex.len(), 32);
        assert_eq!(book_id.as_str(), format!("{md5_hex}-10000"));

        // Dois arquivos identicos produzem o mesmo book_id
        let mut cursor2 = Cursor::new(&data);
        let (book_id2, _) = BookId::from_reader(&mut cursor2, data.len() as u64).unwrap();
        assert_eq!(book_id, book_id2);

        // IDs maliciosos sao rejeitados
        for malicious in [
            "../../etc/passwd",
            "..\\..\\windows\\system32",
            "d41d8cd98f00b204e9800998ecf8427e-100/../secret",
            "d41d8cd98f00b204e9800998ecf8427e-",
            "ZZZZ8cd98f00b204e9800998ecf8427e-100",
            "'; DROP TABLE books; --",
        ] {
            assert!(
                BookId::parse(malicious).is_err(),
                "deveria rejeitar book_id malicioso: {malicious}"
            );
        }
    }

    #[test]
    fn pairing_payload_redacts_secret_in_debug() {
        let payload = PairingQrPayload::new(
            vec!["127.0.0.1".into()],
            47600,
            "cert_fp".into(),
            "spki_fp".into(),
            "super_secret_value_123456".into(),
            "TestServer".into(),
        );
        let debug_str = format!("{payload:?}");
        assert!(!debug_str.contains("super_secret_value_123456"));
        assert!(debug_str.contains("[REDACTED]"));
    }

    proptest! {
        #[test]
        fn prop_hlc_total_order_matches_canonical_string(
            w1 in 0u64..=MAX_HLC_WALL_MS,
            l1 in 0u32..=MAX_HLC_LOGICAL,
            n1 in "[a-zA-Z0-9_-]{1,16}",
            w2 in 0u64..=MAX_HLC_WALL_MS,
            l2 in 0u32..=MAX_HLC_LOGICAL,
            n2 in "[a-zA-Z0-9_-]{1,16}",
        ) {
            let h1 = Hlc::new(w1, l1, n1).unwrap();
            let h2 = Hlc::new(w2, l2, n2).unwrap();

            let s1 = h1.to_canonical_string();
            let s2 = h2.to_canonical_string();

            prop_assert_eq!(h1.cmp(&h2), s1.cmp(&s2));
            prop_assert_eq!(&Hlc::parse(&s1).unwrap(), &h1);
            prop_assert_eq!(&Hlc::parse(&s2).unwrap(), &h2);
        }

        #[test]
        fn prop_hlc_clock_strictly_monotonic(
            phys_steps in prop::collection::vec(0u64..1_000_000u64, 1..50),
            remote_steps in prop::collection::vec((0u64..1_000_000u64, 0u32..100u32), 1..50),
        ) {
            let clock = Arc::new(ManualClock::new(500_000));
            let hlc_clock = HlcClock::with_clock("node-local", clock.clone()).unwrap();

            let mut prev = hlc_clock.now().unwrap();
            for (idx, &phys) in phys_steps.iter().enumerate() {
                clock.set_ms(phys);
                let next = if idx % 2 == 0 {
                    hlc_clock.now().unwrap()
                } else {
                    let (rw, rl) = remote_steps[idx % remote_steps.len()];
                    let remote = Hlc::new(rw, rl, "node-remote").unwrap();
                    hlc_clock.receive(&remote, u64::MAX).unwrap()
                };
                prop_assert!(next > prev, "expected {next} > {prev}");
                if idx % 2 == 1 {
                    let (rw, rl) = remote_steps[idx % remote_steps.len()];
                    let remote = Hlc::new(rw, rl, "node-remote").unwrap();
                    prop_assert!(
                        (next.wall_ms, next.logical) > (remote.wall_ms, remote.logical),
                        "receive must advance past remote ({next} vs {remote})"
                    );
                }
                prev = next;
            }
        }
    }
}

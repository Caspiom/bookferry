use crate::error::ClientError;
use crate::tls::TlsPin;
use bookferry_core::{
    decide_lww, generate_random_hex, BookFormat, BookId, CoreError, Hlc, LwwDecision,
    PositionRecord, PositionUpdate, MAX_HLC_LOGICAL,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

pub const CLIENT_DB_FILENAME: &str = "client.db";
pub const TOKEN_SECRET_FILENAME: &str = "token.secret";
pub const CURRENT_CLIENT_SCHEMA_VERSION: i32 = 3;

/// DDL completo da versao 1 do banco SQLite local do cliente (`PRAGMA user_version = 1`).
pub const CLIENT_SCHEMA_V1_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS pairing (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    device_id      TEXT NOT NULL,
    device_name    TEXT NOT NULL,
    server_name    TEXT NOT NULL,
    port           INTEGER NOT NULL,
    last_good_ip   TEXT NULL,
    known_ips_json TEXT NOT NULL,
    cert_sha256    TEXT NOT NULL,
    spki_sha256    TEXT NOT NULL,
    paired_at_ms   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS cached_books (
    book_id       TEXT PRIMARY KEY NOT NULL,
    partial_md5   TEXT NOT NULL,
    title         TEXT NOT NULL,
    format        TEXT NOT NULL CHECK (format IN ('epub', 'pdf', 'cbz')),
    size_bytes    INTEGER NOT NULL,
    mtime_ms      INTEGER NOT NULL,
    etag          TEXT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS positions (
    book_id       TEXT PRIMARY KEY NOT NULL,
    device_id     TEXT NOT NULL,
    hlc_canonical TEXT NOT NULL,
    hlc_wall_ms   INTEGER NOT NULL,
    hlc_logical   INTEGER NOT NULL,
    hlc_node      TEXT NOT NULL,
    locator       TEXT NOT NULL,
    snippet       TEXT NULL,
    percent       REAL NOT NULL CHECK (percent >= 0.0 AND percent <= 100.0),
    server_seq    INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_client_positions_hlc ON positions(hlc_canonical);

CREATE TABLE IF NOT EXISTS device_positions (
    book_id       TEXT NOT NULL,
    is_local      INTEGER NOT NULL CHECK (is_local IN (0, 1)),
    device_id     TEXT NOT NULL,
    hlc_canonical TEXT NOT NULL,
    hlc_wall_ms   INTEGER NOT NULL,
    hlc_logical   INTEGER NOT NULL,
    hlc_node      TEXT NOT NULL,
    locator       TEXT NOT NULL,
    snippet       TEXT NULL,
    percent       REAL NOT NULL CHECK (percent >= 0.0 AND percent <= 100.0),
    server_seq    INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (book_id, is_local)
);

CREATE TABLE IF NOT EXISTS outbox (
    book_id       TEXT PRIMARY KEY NOT NULL,
    device_id     TEXT NOT NULL,
    hlc_canonical TEXT NOT NULL,
    hlc_wall_ms   INTEGER NOT NULL,
    hlc_logical   INTEGER NOT NULL,
    hlc_node      TEXT NOT NULL,
    locator       TEXT NOT NULL,
    snippet       TEXT NULL,
    percent       REAL NOT NULL CHECK (percent >= 0.0 AND percent <= 100.0),
    attempts      INTEGER NOT NULL DEFAULT 0,
    last_error    TEXT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_client_outbox_hlc ON outbox(hlc_canonical ASC);
"#;

/// Informacoes publicas do servidor pareado (sem expor o token).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedServerInfo {
    pub device_id: String,
    pub device_name: String,
    pub server_name: String,
    pub port: u16,
    pub last_good_ip: Option<String>,
    pub known_ips: Vec<String>,
    pub cert_sha256: String,
    pub spki_sha256: String,
    pub paired_at_ms: u64,
}

impl PairedServerInfo {
    pub fn tls_pin(&self) -> Result<TlsPin, ClientError> {
        TlsPin::new(
            Some(self.cert_sha256.clone()),
            Some(self.spki_sha256.clone()),
        )
    }
}

/// Credenciais completas carregadas internamente pelo cliente (com redacao de `token` em `Debug`).
#[derive(Clone)]
pub struct StoredPairing {
    pub info: PairedServerInfo,
    pub token: String,
}

impl fmt::Debug for StoredPairing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredPairing")
            .field("info", &self.info)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

/// Livro em cache local no cliente, combinado com a melhor posicao conhecida e estado de download.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CachedBook {
    pub book_id: String,
    pub partial_md5: String,
    pub title: String,
    pub format: BookFormat,
    pub size_bytes: u64,
    pub mtime_ms: u64,
    /// Pasta relativa ao vault do servidor (`""` = raiz), separada por `/`.
    pub folder: String,
    /// Nome escolhido pelo usuario; a UI mostra este no lugar de `title` quando presente.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_title: Option<String>,
    /// `0` = sem capa; senao identifica a versao da capa (cache-buster).
    pub cover_version: u64,
    /// `embedded`, `custom` ou `web` (`None` = sem capa).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_source: Option<String>,
    /// Instante em que o usuario tirou o livro do "Continuar lendo" (relogio do servidor).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden_continue_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<PositionRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_path: Option<PathBuf>,
}

/// Metadados editaveis de um livro, como o servidor os envia (resposta das escritas e SSE).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookMeta {
    pub book_id: String,
    #[serde(default)]
    pub display_title: Option<String>,
    #[serde(default)]
    pub cover_version: u64,
    #[serde(default)]
    pub cover_source: Option<String>,
    #[serde(default)]
    pub hidden_continue_at_ms: Option<u64>,
}

/// Item pendente na fila `outbox` para envio ao servidor (`PUT /books/{id}/position`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutboxEntry {
    pub book_id: String,
    pub device_id: String,
    pub hlc: String,
    pub hlc_wall_ms: u64,
    pub hlc_logical: u32,
    pub hlc_node: String,
    pub locator: String,
    pub snippet: Option<String>,
    pub percent: f64,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub updated_at_ms: u64,
}

impl OutboxEntry {
    pub fn to_position_update(&self) -> PositionUpdate {
        PositionUpdate {
            device_id: self.device_id.clone(),
            hlc: self.hlc.clone(),
            locator: self.locator.clone(),
            snippet: self.snippet.clone(),
            percent: self.percent,
        }
    }
}

/// Banco SQLite local do cliente (`client.db`) e arquivo de segredo `token.secret` (ambos `0600`).
pub struct ClientDatabase {
    data_dir: PathBuf,
    db_path: PathBuf,
    token_path: PathBuf,
    conn: Mutex<Connection>,
}

impl ClientDatabase {
    pub fn open(data_dir: &Path, default_device_name: &str) -> Result<Self, ClientError> {
        fs::create_dir_all(data_dir)?;
        let db_path = data_dir.join(CLIENT_DB_FILENAME);
        let token_path = data_dir.join(TOKEN_SECRET_FILENAME);

        ensure_file_0600(&db_path)?;
        if token_path.exists() {
            enforce_permissions_0600(&token_path)?;
        }

        let mut conn = Connection::open(&db_path)?;
        conn.busy_timeout(Duration::from_millis(5_000))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        run_migrations(&mut conn)?;
        enforce_permissions_0600(&db_path)?;

        let db = Self {
            data_dir: data_dir.to_path_buf(),
            db_path,
            token_path,
            conn: Mutex::new(conn),
        };
        db.ensure_initial_hlc_state(default_device_name)?;
        Ok(db)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn token_path(&self) -> &Path {
        &self.token_path
    }

    fn lock_conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn ensure_initial_hlc_state(&self, default_device_name: &str) -> Result<(), ClientError> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let existing_node = get_meta_in_tx(&tx, "hlc_node")?;
        if existing_node.is_none() {
            let fallback_node = format!(
                "local_{}",
                sanitize_hlc_node(default_device_name, &generate_random_hex(4)?)
            );
            set_meta_in_tx(&tx, "hlc_node", &fallback_node)?;
            set_meta_in_tx(&tx, "hlc_wall_ms", "0")?;
            set_meta_in_tx(&tx, "hlc_logical", "0")?;
            set_meta_in_tx(&tx, "clock_offset_ms", "0")?;
        }

        // Garante que o estado HLC no `meta` nunca retroceda em relacao a qualquer posicao ja salva
        let max_pos_hlc: Option<String> = tx
            .query_row(
                "SELECT hlc_canonical FROM positions ORDER BY hlc_canonical DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let max_outbox_hlc: Option<String> = tx
            .query_row(
                "SELECT hlc_canonical FROM outbox ORDER BY hlc_canonical DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;

        for maybe_raw in [max_pos_hlc, max_outbox_hlc].into_iter().flatten() {
            if let Ok(parsed) = Hlc::parse(&maybe_raw) {
                let (cur_w, cur_l, _) = read_hlc_state_in_tx(&tx)?;
                if (parsed.wall_ms, parsed.logical) > (cur_w, cur_l) {
                    set_meta_in_tx(&tx, "hlc_wall_ms", &parsed.wall_ms.to_string())?;
                    set_meta_in_tx(&tx, "hlc_logical", &parsed.logical.to_string())?;
                }
            }
        }

        tx.commit()?;
        Ok(())
    }

    /// Salva o pareamento (unico servidor por instalacao) no SQLite e grava o token em
    /// `data_dir/token.secret` com permissao `0600`.
    pub fn save_pairing(&self, info: &PairedServerInfo, token: &str) -> Result<(), ClientError> {
        write_secret_file_0600(&self.token_path, token.as_bytes())?;
        enforce_permissions_0600(&self.db_path)?;

        let known_ips_json = serde_json::to_string(&info.known_ips)?;
        let node = sanitize_hlc_node(&info.device_id, "client");

        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        // Um unico servidor por instalacao (`id = 1`)
        tx.execute("DELETE FROM pairing", [])?;
        tx.execute(
            "INSERT INTO pairing (
                id, device_id, device_name, server_name, port, last_good_ip,
                known_ips_json, cert_sha256, spki_sha256, paired_at_ms
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                info.device_id,
                info.device_name,
                info.server_name,
                i64::from(info.port),
                info.last_good_ip,
                known_ips_json,
                info.cert_sha256,
                info.spki_sha256,
                info.paired_at_ms as i64,
            ],
        )?;

        // Atualiza o node_id do HLC para o device_id oficial atribuido pelo servidor,
        // preservando wall_ms e logical monotonicos!
        set_meta_in_tx(&tx, "hlc_node", &node)?;

        // Re-atribui eventuais entradas da outbox criadas antes do pareamento para o device_id oficial
        tx.execute(
            "UPDATE outbox SET device_id = ?1 WHERE device_id LIKE 'local_%'",
            params![info.device_id],
        )?;
        tx.execute(
            "UPDATE positions SET device_id = ?1 WHERE device_id LIKE 'local_%'",
            params![info.device_id],
        )?;
        tx.execute(
            "UPDATE device_positions SET device_id = ?1 WHERE is_local = 1 AND device_id LIKE 'local_%'",
            params![info.device_id],
        )?;

        tx.commit()?;
        Ok(())
    }

    /// Carrega o pareamento ativo e o token de `token.secret`.
    pub fn load_pairing(&self) -> Result<Option<StoredPairing>, ClientError> {
        let conn = self.lock_conn();
        let row: Option<PairedServerInfo> = conn
            .query_row(
                "SELECT device_id, device_name, server_name, port, last_good_ip,
                        known_ips_json, cert_sha256, spki_sha256, paired_at_ms
                 FROM pairing
                 WHERE id = 1",
                [],
                |r| {
                    let known_ips_raw: String = r.get(5)?;
                    let known_ips: Vec<String> =
                        serde_json::from_str(&known_ips_raw).unwrap_or_default();
                    Ok(PairedServerInfo {
                        device_id: r.get(0)?,
                        device_name: r.get(1)?,
                        server_name: r.get(2)?,
                        port: r.get::<_, i64>(3)?.clamp(0, 65535) as u16,
                        last_good_ip: r.get(4)?,
                        known_ips,
                        cert_sha256: r.get(6)?,
                        spki_sha256: r.get(7)?,
                        paired_at_ms: r.get::<_, i64>(8)?.max(0) as u64,
                    })
                },
            )
            .optional()?;
        drop(conn);

        let Some(info) = row else {
            return Ok(None);
        };

        let token = match fs::read_to_string(&self.token_path) {
            Ok(s) => s.trim().to_string(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(ClientError::Io(e)),
        };
        if token.is_empty() {
            return Ok(None);
        }

        Ok(Some(StoredPairing { info, token }))
    }

    /// Atualiza `last_good_ip` e `port` quando o cliente conecta em um novo IP (ex.: mudanca DHCP via mDNS).
    pub fn update_last_good_endpoint(&self, ip: &str, port: u16) -> Result<bool, ClientError> {
        let conn = self.lock_conn();
        let changed = conn.execute(
            "UPDATE pairing
             SET last_good_ip = ?1, port = ?2
             WHERE id = 1 AND (last_good_ip IS NOT ?1 OR port != ?2)",
            params![ip, i64::from(port)],
        )?;
        Ok(changed > 0)
    }

    /// Remove apenas a sessao de pareamento e o arquivo de token (usado quando o servidor
    /// retorna `401 Unauthorized` indicando token revogado, preservando livros/posicoes locais).
    pub fn clear_auth_on_revocation(&self) -> Result<(), ClientError> {
        if self.token_path.exists() {
            let _ = fs::remove_file(&self.token_path);
        }
        let conn = self.lock_conn();
        conn.execute("DELETE FROM pairing", [])?;
        Ok(())
    }

    /// Apaga absolutamente todos os dados locais (`unpair()`): pareamento, token, outbox,
    /// cache da biblioteca, posicoes e livros baixados em `data_dir/books/`.
    pub fn wipe_all_on_unpair(&self) -> Result<(), ClientError> {
        if self.token_path.exists() {
            let _ = fs::remove_file(&self.token_path);
        }
        let books_dir = self.data_dir.join("books");
        if books_dir.exists() {
            let _ = fs::remove_dir_all(&books_dir);
        }

        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM pairing", [])?;
        tx.execute("DELETE FROM outbox", [])?;
        tx.execute("DELETE FROM cached_books", [])?;
        tx.execute("DELETE FROM positions", [])?;
        tx.execute("DELETE FROM device_positions", [])?;
        tx.execute(
            "DELETE FROM meta WHERE key IN ('last_pull_hlc', 'last_sse_event_id', 'clock_offset_ms')",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Gera e persiste o proximo carimbo HLC monotonico do cliente.
    pub fn next_hlc(&self, raw_now_ms: u64) -> Result<Hlc, ClientError> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let hlc = next_hlc_in_tx(&tx, raw_now_ms)?;
        tx.commit()?;
        Ok(hlc)
    }

    /// Recebe um HLC remoto e avanca o relogio HLC persistido no SQLite.
    pub fn receive_hlc(&self, remote: &Hlc, raw_now_ms: u64) -> Result<Hlc, ClientError> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let hlc = receive_hlc_in_tx(&tx, remote, raw_now_ms)?;
        tx.commit()?;
        Ok(hlc)
    }

    /// Grava uma posicao local imediatamente (`set_position`) e insere/atualiza na `outbox`
    /// (um unico registro por `book_id` com o estado mais recente).
    pub fn set_local_position(
        &self,
        book_id: &BookId,
        locator: &str,
        snippet: Option<String>,
        percent: f64,
        raw_now_ms: u64,
    ) -> Result<PositionRecord, ClientError> {
        let locator = locator.trim();
        if locator.is_empty() || locator.len() > bookferry_core::MAX_LOCATOR_BYTES {
            return Err(ClientError::Core(CoreError::InvalidLocator));
        }
        if let Some(ref s) = snippet {
            let count = s.chars().count();
            if count > bookferry_core::MAX_SNIPPET_CHARS {
                return Err(ClientError::Core(CoreError::SnippetTooLong {
                    len: count,
                    max: bookferry_core::MAX_SNIPPET_CHARS,
                }));
            }
        }
        if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
            return Err(ClientError::Core(CoreError::InvalidPercent(percent)));
        }

        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        // Se ja existe uma posicao conhecida (ex.: remota) com HLC maior que o estado atual,
        // avanca o HLC local primeiro para que a acao explicita do usuario vença.
        let existing_pos = get_position_in_tx(&tx, book_id.as_str())?;
        if let Some(ref cur) = existing_pos {
            if let Ok(cur_hlc) = Hlc::parse(&cur.hlc) {
                let (cur_w, cur_l, _) = read_hlc_state_in_tx(&tx)?;
                if (cur_hlc.wall_ms, cur_hlc.logical) > (cur_w, cur_l) {
                    let _ = receive_hlc_in_tx(&tx, &cur_hlc, raw_now_ms)?;
                }
            }
        }

        let hlc = next_hlc_in_tx(&tx, raw_now_ms)?;
        let canonical = hlc.to_canonical_string();
        let device_id = get_device_id_or_node_in_tx(&tx)?;
        let now_ms = effective_now_ms_in_tx(&tx, raw_now_ms)?;

        let record = PositionRecord {
            book_id: book_id.as_str().to_string(),
            device_id: device_id.clone(),
            hlc: canonical.clone(),
            hlc_wall_ms: hlc.wall_ms,
            hlc_logical: hlc.logical,
            hlc_node: hlc.node.clone(),
            locator: locator.to_string(),
            snippet: snippet.clone(),
            percent,
            server_seq: existing_pos.map_or(0, |p| p.server_seq),
            updated_at_ms: now_ms,
        };

        upsert_position_table_in_tx(&tx, "positions", &record)?;
        upsert_device_position_in_tx(&tx, true, &record)?;

        // Upsert na outbox: garante no maximo 1 PUT pendente por livro (sempre o mais recente)
        tx.execute(
            "INSERT INTO outbox (
                book_id, device_id, hlc_canonical, hlc_wall_ms, hlc_logical, hlc_node,
                locator, snippet, percent, attempts, last_error, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, NULL, ?10)
             ON CONFLICT(book_id) DO UPDATE SET
                device_id = excluded.device_id,
                hlc_canonical = excluded.hlc_canonical,
                hlc_wall_ms = excluded.hlc_wall_ms,
                hlc_logical = excluded.hlc_logical,
                hlc_node = excluded.hlc_node,
                locator = excluded.locator,
                snippet = excluded.snippet,
                percent = excluded.percent,
                attempts = 0,
                last_error = NULL,
                updated_at_ms = excluded.updated_at_ms",
            params![
                record.book_id,
                record.device_id,
                record.hlc,
                record.hlc_wall_ms as i64,
                i64::from(record.hlc_logical),
                record.hlc_node,
                record.locator,
                record.snippet,
                record.percent,
                record.updated_at_ms as i64,
            ],
        )?;

        tx.commit()?;
        Ok(record)
    }

    /// Retorna a melhor posicao conhecida para `book_id` (local vs servidor por HLC).
    pub fn get_position(&self, book_id: &BookId) -> Result<Option<PositionRecord>, ClientError> {
        let conn = self.lock_conn();
        Ok(query_position_from_conn(&conn, book_id.as_str())?)
    }

    /// Retorna `(local_position, remote_position)` para `book_id` (usado por `decide_book_resume`).
    pub fn get_local_and_remote_positions(
        &self,
        book_id: &BookId,
    ) -> Result<(Option<PositionRecord>, Option<PositionRecord>), ClientError> {
        let conn = self.lock_conn();
        let local = query_device_position_from_conn(&conn, book_id.as_str(), true)?;
        let remote = query_device_position_from_conn(&conn, book_id.as_str(), false)?;
        Ok((local, remote))
    }

    /// Lista todos os itens pendentes na `outbox` em ordem cronologica de HLC.
    pub fn list_outbox(&self) -> Result<Vec<OutboxEntry>, ClientError> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT book_id, device_id, hlc_canonical, hlc_wall_ms, hlc_logical, hlc_node,
                    locator, snippet, percent, attempts, last_error, updated_at_ms
             FROM outbox
             ORDER BY hlc_canonical ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(OutboxEntry {
                book_id: r.get(0)?,
                device_id: r.get(1)?,
                hlc: r.get(2)?,
                hlc_wall_ms: r.get::<_, i64>(3)?.max(0) as u64,
                hlc_logical: r.get::<_, i64>(4)?.max(0) as u32,
                hlc_node: r.get(5)?,
                locator: r.get(6)?,
                snippet: r.get(7)?,
                percent: r.get(8)?,
                attempts: r.get::<_, i64>(9)?.max(0) as u32,
                last_error: r.get(10)?,
                updated_at_ms: r.get::<_, i64>(11)?.max(0) as u64,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn outbox_len(&self) -> Result<usize, ClientError> {
        let conn = self.lock_conn();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get(0))?;
        Ok(count.max(0) as usize)
    }

    /// Registra falha transitoria ao enviar um item da `outbox`.
    pub fn record_outbox_retry_error(
        &self,
        book_id: &str,
        sent_hlc_canonical: &str,
        error_msg: &str,
    ) -> Result<(), ClientError> {
        let conn = self.lock_conn();
        conn.execute(
            "UPDATE outbox
             SET attempts = attempts + 1, last_error = ?1
             WHERE book_id = ?2 AND hlc_canonical = ?3",
            params![error_msg, book_id, sent_hlc_canonical],
        )?;
        Ok(())
    }

    /// Remove um item permanente invalido (ex.: 404 book_not_found) da `outbox`.
    pub fn remove_outbox_if_matching(
        &self,
        book_id: &str,
        sent_hlc_canonical: &str,
    ) -> Result<(), ClientError> {
        let conn = self.lock_conn();
        conn.execute(
            "DELETE FROM outbox WHERE book_id = ?1 AND hlc_canonical <= ?2",
            params![book_id, sent_hlc_canonical],
        )?;
        Ok(())
    }

    /// Conclui o envio de um item da `outbox` apos resposta `200 OK` de `PUT /books/{id}/position`.
    ///
    /// Retorna `Some(winning_position)` quando `applied == false` e o servidor tinha uma
    /// posicao mais nova que foi adotada localmente (para emitir `ClientEvent::PositionChanged`).
    pub fn complete_outbox_put(
        &self,
        book_id: &str,
        sent_hlc_canonical: &str,
        applied: bool,
        server_position: &PositionRecord,
        server_hlc_str: &str,
        raw_now_ms: u64,
    ) -> Result<Option<PositionRecord>, ClientError> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        if let Ok(srv_hlc) = Hlc::parse(server_hlc_str) {
            let _ = receive_hlc_in_tx(&tx, &srv_hlc, raw_now_ms)?;
        }
        if let Ok(pos_hlc) = Hlc::parse(&server_position.hlc) {
            let _ = receive_hlc_in_tx(&tx, &pos_hlc, raw_now_ms)?;
        }

        // Remove da outbox apenas se nenhuma escrita local mais nova tiver ocorrido nesse meio-tempo
        tx.execute(
            "DELETE FROM outbox WHERE book_id = ?1 AND hlc_canonical <= ?2",
            params![book_id, sent_hlc_canonical],
        )?;

        let my_device_id = get_device_id_or_node_in_tx(&tx)?;
        let is_from_self = server_position.device_id == my_device_id;
        upsert_device_position_in_tx(&tx, is_from_self, server_position)?;

        // Verifica se a posicao do servidor deve atualizar a posicao vencedora local
        let existing = get_position_in_tx(&tx, book_id)?;
        let current_hlc = existing.as_ref().and_then(|p| Hlc::parse(&p.hlc).ok());
        let incoming_hlc = Hlc::parse(&server_position.hlc)?;

        let mut adopted_conflict_winner = None;
        if applied {
            // Se aplicou e nao houve escrita local mais nova ainda na outbox, atualiza `server_seq`
            let should_update = match current_hlc {
                None => true,
                Some(ref cur) => &incoming_hlc >= cur,
            };
            if should_update {
                upsert_position_table_in_tx(&tx, "positions", server_position)?;
            }
        } else {
            // applied == false: o servidor tinha uma posicao mais nova!
            let should_update = match current_hlc {
                None => true,
                Some(ref cur) => &incoming_hlc >= cur,
            };
            if should_update {
                upsert_position_table_in_tx(&tx, "positions", server_position)?;
            }
            adopted_conflict_winner = Some(server_position.clone());
        }

        tx.commit()?;
        Ok(adopted_conflict_winner)
    }

    /// Ajusta o relogio HLC e re-carimba o item da `outbox` quando o servidor rejeita
    /// com `422 hlc_too_far_in_future`.
    pub fn adjust_hlc_on_future_rejection(
        &self,
        book_id: &str,
        server_now_ms: u64,
        server_current_pos: Option<&PositionRecord>,
        raw_now_ms: u64,
    ) -> Result<Option<OutboxEntry>, ClientError> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let offset_ms = (server_now_ms as i128) - (raw_now_ms as i128);
        set_meta_in_tx(&tx, "clock_offset_ms", &offset_ms.to_string())?;
        set_meta_in_tx(&tx, "hlc_wall_ms", &server_now_ms.to_string())?;
        set_meta_in_tx(&tx, "hlc_logical", "0")?;

        if let Some(srv_pos) = server_current_pos {
            if let Ok(srv_hlc) = Hlc::parse(&srv_pos.hlc) {
                if srv_hlc
                    .validate_drift(server_now_ms, bookferry_core::DEFAULT_MAX_FUTURE_DRIFT_MS)
                    .is_ok()
                {
                    let _ = receive_hlc_in_tx(&tx, &srv_hlc, raw_now_ms)?;
                }
            }
        }

        let new_hlc = next_hlc_in_tx(&tx, raw_now_ms)?;
        let canonical = new_hlc.to_canonical_string();

        tx.execute(
            "UPDATE outbox
             SET hlc_canonical = ?1, hlc_wall_ms = ?2, hlc_logical = ?3, hlc_node = ?4, updated_at_ms = ?5
             WHERE book_id = ?6",
            params![
                canonical,
                new_hlc.wall_ms as i64,
                i64::from(new_hlc.logical),
                new_hlc.node,
                server_now_ms as i64,
                book_id,
            ],
        )?;

        tx.execute(
            "UPDATE positions
             SET hlc_canonical = ?1, hlc_wall_ms = ?2, hlc_logical = ?3, hlc_node = ?4, updated_at_ms = ?5
             WHERE book_id = ?6",
            params![
                canonical,
                new_hlc.wall_ms as i64,
                i64::from(new_hlc.logical),
                new_hlc.node,
                server_now_ms as i64,
                book_id,
            ],
        )?;

        tx.execute(
            "UPDATE device_positions
             SET hlc_canonical = ?1, hlc_wall_ms = ?2, hlc_logical = ?3, hlc_node = ?4, updated_at_ms = ?5
             WHERE book_id = ?6 AND is_local = 1",
            params![
                canonical,
                new_hlc.wall_ms as i64,
                i64::from(new_hlc.logical),
                new_hlc.node,
                server_now_ms as i64,
                book_id,
            ],
        )?;

        let updated_entry: Option<OutboxEntry> = tx
            .query_row(
                "SELECT book_id, device_id, hlc_canonical, hlc_wall_ms, hlc_logical, hlc_node,
                        locator, snippet, percent, attempts, last_error, updated_at_ms
                 FROM outbox
                 WHERE book_id = ?1",
                params![book_id],
                |r| {
                    Ok(OutboxEntry {
                        book_id: r.get(0)?,
                        device_id: r.get(1)?,
                        hlc: r.get(2)?,
                        hlc_wall_ms: r.get::<_, i64>(3)?.max(0) as u64,
                        hlc_logical: r.get::<_, i64>(4)?.max(0) as u32,
                        hlc_node: r.get(5)?,
                        locator: r.get(6)?,
                        snippet: r.get(7)?,
                        percent: r.get(8)?,
                        attempts: r.get::<_, i64>(9)?.max(0) as u32,
                        last_error: r.get(10)?,
                        updated_at_ms: r.get::<_, i64>(11)?.max(0) as u64,
                    })
                },
            )
            .optional()?;

        tx.commit()?;
        Ok(updated_entry)
    }

    /// Mescla uma lista de posicoes vindas do servidor (`pull()` ou `SSE`) por HLC LWW.
    /// Retorna apenas as posicoes que efetivamente venceram e alteraram o estado local.
    pub fn merge_remote_positions(
        &self,
        remote_positions: &[PositionRecord],
        advance_pull_cursor: bool,
        raw_now_ms: u64,
    ) -> Result<Vec<PositionRecord>, ClientError> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let my_device_id = get_device_id_or_node_in_tx(&tx)?;
        let mut changed = Vec::new();
        let mut max_seen_hlc =
            get_meta_in_tx(&tx, "last_pull_hlc")?.and_then(|s| Hlc::parse(&s).ok());

        for rem in remote_positions {
            let Ok(rem_hlc) = Hlc::parse(&rem.hlc) else {
                continue;
            };
            let _ = receive_hlc_in_tx(&tx, &rem_hlc, raw_now_ms)?;

            if let Some(ref cur_max) = max_seen_hlc {
                if &rem_hlc > cur_max {
                    max_seen_hlc = Some(rem_hlc.clone());
                }
            } else {
                max_seen_hlc = Some(rem_hlc.clone());
            }

            let is_from_self = rem.device_id == my_device_id;
            let existing_dev_pos = query_device_position_in_tx(&tx, &rem.book_id, is_from_self)?;
            let existing_dev_hlc = existing_dev_pos
                .as_ref()
                .and_then(|p| Hlc::parse(&p.hlc).ok());
            if decide_lww(existing_dev_hlc.as_ref(), &rem_hlc) == LwwDecision::Apply {
                upsert_device_position_in_tx(&tx, is_from_self, rem)?;
            }

            let current_winner = get_position_in_tx(&tx, &rem.book_id)?;
            let current_hlc = current_winner
                .as_ref()
                .and_then(|p| Hlc::parse(&p.hlc).ok());

            if decide_lww(current_hlc.as_ref(), &rem_hlc) == LwwDecision::Apply {
                let canonical = rem_hlc.to_canonical_string();
                let mut normalized = rem.clone();
                normalized.hlc = canonical;
                upsert_position_table_in_tx(&tx, "positions", &normalized)?;

                // Nota: NAO deletamos itens da `outbox` aqui! Qualquer escrita local pendente
                // na `outbox` ainda deve ser enviada via PUT ao servidor para que seja registrada
                // em `position_history` (`applied = false`, zero perda silenciosa de dados).

                if !is_from_self {
                    changed.push(normalized);
                }
            }
        }

        if advance_pull_cursor {
            if let Some(max_hlc) = max_seen_hlc {
                set_meta_in_tx(&tx, "last_pull_hlc", &max_hlc.to_canonical_string())?;
            }
        }

        tx.commit()?;
        Ok(changed)
    }

    pub fn last_pull_hlc(&self) -> Result<Option<String>, ClientError> {
        let conn = self.lock_conn();
        Ok(conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'last_pull_hlc'",
                [],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn last_sse_event_id(&self) -> Result<Option<u64>, ClientError> {
        let conn = self.lock_conn();
        let raw: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'last_sse_event_id'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(raw.and_then(|s| s.parse::<u64>().ok()))
    }

    pub fn set_last_sse_event_id(&self, seq: u64) -> Result<(), ClientError> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = get_meta_in_tx(&tx, "last_sse_event_id")?
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        if seq > current {
            set_meta_in_tx(&tx, "last_sse_event_id", &seq.to_string())?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Atualiza o cache local da biblioteca (`cached_books`) e mescla as posicoes vindas em `GET /books`.
    /// Retorna `(lista_de_livros_atualizada, mudou_biblioteca, posicoes_remotas_aplicadas)`.
    pub fn store_library_snapshot(
        &self,
        books: &[ServerBookEntry],
        raw_now_ms: u64,
    ) -> Result<(Vec<CachedBook>, bool, Vec<PositionRecord>), ClientError> {
        let mut remote_positions = Vec::new();
        for b in books {
            if let Some(ref pos) = b.position {
                remote_positions.push(pos.clone());
            }
        }

        let applied_positions =
            self.merge_remote_positions(&remote_positions, false, raw_now_ms)?;

        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now_ms = effective_now_ms_in_tx(&tx, raw_now_ms)?;

        let prev_ids: Vec<(String, String, u64)> = {
            let mut stmt = tx.prepare(
                "SELECT book_id, folder || '/' || title || '|' || COALESCE(display_title, '') || '|' || cover_version || '|' || COALESCE(hidden_continue_at_ms, 0), mtime_ms FROM cached_books ORDER BY book_id ASC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?.max(0) as u64,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        tx.execute("DELETE FROM cached_books", [])?;
        for b in books {
            tx.execute(
                "INSERT INTO cached_books (
                    book_id, partial_md5, title, format, size_bytes, mtime_ms, etag, updated_at_ms,
                    folder, display_title, cover_version, cover_source, hidden_continue_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    b.book_id,
                    b.partial_md5,
                    b.title,
                    b.format.as_str(),
                    b.size_bytes as i64,
                    b.mtime_ms as i64,
                    now_ms as i64,
                    b.folder,
                    b.display_title,
                    b.cover_version as i64,
                    b.cover_source,
                    b.hidden_continue_at_ms.map(|v| v as i64),
                ],
            )?;
        }

        let new_ids: Vec<(String, String, u64)> = {
            let mut stmt = tx.prepare(
                "SELECT book_id, folder || '/' || title || '|' || COALESCE(display_title, '') || '|' || cover_version || '|' || COALESCE(hidden_continue_at_ms, 0), mtime_ms FROM cached_books ORDER BY book_id ASC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?.max(0) as u64,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        tx.commit()?;
        drop(conn);

        let library_changed = prev_ids != new_ids;
        let cached = self.load_cached_library()?;
        Ok((cached, library_changed, applied_positions))
    }

    /// Carrega a biblioteca do cache SQLite local combinada com a melhor posicao conhecida
    /// (local vs remota por HLC) e o caminho do arquivo baixado se existir.
    pub fn load_cached_library(&self) -> Result<Vec<CachedBook>, ClientError> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT
                b.book_id, b.partial_md5, b.title, b.format, b.size_bytes, b.mtime_ms,
                p.device_id, p.hlc_canonical, p.hlc_wall_ms, p.hlc_logical, p.hlc_node,
                p.locator, p.snippet, p.percent, p.server_seq, p.updated_at_ms,
                b.folder, b.display_title, b.cover_version, b.cover_source, b.hidden_continue_at_ms
             FROM cached_books b
             LEFT JOIN positions p ON b.book_id = p.book_id
             ORDER BY b.title ASC, b.book_id ASC",
        )?;

        let books_dir = self.data_dir.join("books");
        let rows = stmt.query_map([], |r| {
            let book_id: String = r.get(0)?;
            let fmt_str: String = r.get(3)?;
            let format = BookFormat::from_extension(&fmt_str).unwrap_or(BookFormat::Epub);
            let pos_dev: Option<String> = r.get(6)?;
            let position = match pos_dev {
                Some(device_id) => Some(PositionRecord {
                    book_id: book_id.clone(),
                    device_id,
                    hlc: r.get(7)?,
                    hlc_wall_ms: r.get::<_, i64>(8)?.max(0) as u64,
                    hlc_logical: r.get::<_, i64>(9)?.max(0) as u32,
                    hlc_node: r.get(10)?,
                    locator: r.get(11)?,
                    snippet: r.get(12)?,
                    percent: r.get(13)?,
                    server_seq: r.get::<_, i64>(14)?.max(0) as u64,
                    updated_at_ms: r.get::<_, i64>(15)?.max(0) as u64,
                }),
                None => None,
            };

            let candidate_path = books_dir.join(&book_id);
            let local_path = if candidate_path.is_file() {
                Some(candidate_path)
            } else {
                None
            };

            Ok(CachedBook {
                book_id,
                partial_md5: r.get(1)?,
                title: r.get(2)?,
                format,
                size_bytes: r.get::<_, i64>(4)?.max(0) as u64,
                mtime_ms: r.get::<_, i64>(5)?.max(0) as u64,
                folder: r.get(16)?,
                display_title: r.get(17)?,
                cover_version: r.get::<_, i64>(18)?.max(0) as u64,
                cover_source: r.get(19)?,
                hidden_continue_at_ms: r.get::<_, Option<i64>>(20)?.map(|v| v.max(0) as u64),
                position,
                local_path,
            })
        })?;

        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Aplica metadados recebidos (resposta de escrita ou SSE) a um livro ja em cache.
    /// Retorna `true` se a linha existe e algo mudou.
    pub fn apply_book_meta(&self, meta: &BookMeta) -> Result<bool, ClientError> {
        let conn = self.lock_conn();
        let n = conn.execute(
            "UPDATE cached_books
             SET display_title = ?2, cover_version = ?3, cover_source = ?4, hidden_continue_at_ms = ?5
             WHERE book_id = ?1
               AND (display_title IS NOT ?2 OR cover_version != ?3 OR cover_source IS NOT ?4
                    OR hidden_continue_at_ms IS NOT ?5)",
            params![
                meta.book_id,
                meta.display_title,
                meta.cover_version as i64,
                meta.cover_source,
                meta.hidden_continue_at_ms.map(|v| v as i64),
            ],
        )?;
        Ok(n > 0)
    }

    /// Versao da capa em cache para o livro (`0` = sem capa ou livro desconhecido).
    pub fn cover_version(&self, book_id: &str) -> Result<u64, ClientError> {
        let conn = self.lock_conn();
        Ok(conn
            .query_row(
                "SELECT cover_version FROM cached_books WHERE book_id = ?1",
                params![book_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0)
            .max(0) as u64)
    }

    pub fn get_download_etag(&self, book_id: &str) -> Result<Option<String>, ClientError> {
        let conn = self.lock_conn();
        let key = format!("download_etag:{book_id}");
        Ok(conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn set_download_etag(&self, book_id: &str, etag: Option<&str>) -> Result<(), ClientError> {
        let conn = self.lock_conn();
        let key = format!("download_etag:{book_id}");
        match etag {
            Some(val) => {
                conn.execute(
                    "INSERT INTO meta (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, val],
                )?;
            }
            None => {
                conn.execute("DELETE FROM meta WHERE key = ?1", params![key])?;
            }
        }
        Ok(())
    }
}

/// DTO de livro recebido em `GET /books` do servidor.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerBookEntry {
    pub book_id: String,
    pub partial_md5: String,
    pub title: String,
    pub format: BookFormat,
    pub size_bytes: u64,
    pub mtime_ms: u64,
    /// Ausente em servidores antigos: vira `""` (raiz).
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub display_title: Option<String>,
    #[serde(default)]
    pub cover_version: u64,
    #[serde(default)]
    pub cover_source: Option<String>,
    #[serde(default)]
    pub hidden_continue_at_ms: Option<u64>,
    #[serde(default)]
    pub position: Option<PositionRecord>,
}

fn get_meta_in_tx(tx: &Transaction<'_>, key: &str) -> Result<Option<String>, rusqlite::Error> {
    tx.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
        r.get(0)
    })
    .optional()
}

fn set_meta_in_tx(tx: &Transaction<'_>, key: &str, value: &str) -> Result<(), rusqlite::Error> {
    tx.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn get_device_id_or_node_in_tx(tx: &Transaction<'_>) -> Result<String, rusqlite::Error> {
    let paired_dev: Option<String> = tx
        .query_row("SELECT device_id FROM pairing WHERE id = 1", [], |r| {
            r.get(0)
        })
        .optional()?;
    if let Some(dev) = paired_dev {
        return Ok(dev);
    }
    Ok(get_meta_in_tx(tx, "hlc_node")?.unwrap_or_else(|| "local_client".to_string()))
}

fn effective_now_ms_in_tx(tx: &Transaction<'_>, raw_now_ms: u64) -> Result<u64, rusqlite::Error> {
    let offset_ms: i128 = get_meta_in_tx(tx, "clock_offset_ms")?
        .and_then(|s| s.parse::<i128>().ok())
        .unwrap_or(0);
    let adjusted = (raw_now_ms as i128).saturating_add(offset_ms);
    Ok(adjusted.max(0) as u64)
}

fn read_hlc_state_in_tx(tx: &Transaction<'_>) -> Result<(u64, u32, String), rusqlite::Error> {
    let wall_ms = get_meta_in_tx(tx, "hlc_wall_ms")?
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let logical = get_meta_in_tx(tx, "hlc_logical")?
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    let node = get_device_id_or_node_in_tx(tx)?;
    let sanitized_node = sanitize_hlc_node(&node, "client");
    Ok((wall_ms, logical, sanitized_node))
}

fn next_hlc_in_tx(tx: &Transaction<'_>, raw_now_ms: u64) -> Result<Hlc, ClientError> {
    let pt = effective_now_ms_in_tx(tx, raw_now_ms)?;
    let (l_prev, c_prev, node) = read_hlc_state_in_tx(tx)?;

    let (l_new, c_new) = if pt > l_prev {
        (pt, 0)
    } else {
        let next_c = c_prev.checked_add(1).ok_or(CoreError::HlcLogicalOverflow)?;
        if next_c > MAX_HLC_LOGICAL {
            return Err(ClientError::Core(CoreError::HlcLogicalOverflow));
        }
        (l_prev, next_c)
    };

    set_meta_in_tx(tx, "hlc_wall_ms", &l_new.to_string())?;
    set_meta_in_tx(tx, "hlc_logical", &c_new.to_string())?;
    set_meta_in_tx(tx, "hlc_node", &node)?;

    Ok(Hlc::new(l_new, c_new, node)?)
}

fn receive_hlc_in_tx(
    tx: &Transaction<'_>,
    remote: &Hlc,
    raw_now_ms: u64,
) -> Result<Hlc, ClientError> {
    let pt = effective_now_ms_in_tx(tx, raw_now_ms)?;
    let (l_prev, c_prev, node) = read_hlc_state_in_tx(tx)?;
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
        return Err(ClientError::Core(CoreError::HlcLogicalOverflow));
    }

    set_meta_in_tx(tx, "hlc_wall_ms", &l_new.to_string())?;
    set_meta_in_tx(tx, "hlc_logical", &c_new.to_string())?;
    set_meta_in_tx(tx, "hlc_node", &node)?;

    Ok(Hlc::new(l_new, c_new, node)?)
}

fn upsert_position_table_in_tx(
    tx: &Transaction<'_>,
    table: &str,
    pos: &PositionRecord,
) -> Result<(), rusqlite::Error> {
    debug_assert_eq!(table, "positions");
    let canonical = Hlc::parse(&pos.hlc)
        .map(|h| h.to_canonical_string())
        .unwrap_or_else(|_| pos.hlc.clone());
    tx.execute(
        "INSERT INTO positions (
            book_id, device_id, hlc_canonical, hlc_wall_ms, hlc_logical, hlc_node,
            locator, snippet, percent, server_seq, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(book_id) DO UPDATE SET
            device_id = excluded.device_id,
            hlc_canonical = excluded.hlc_canonical,
            hlc_wall_ms = excluded.hlc_wall_ms,
            hlc_logical = excluded.hlc_logical,
            hlc_node = excluded.hlc_node,
            locator = excluded.locator,
            snippet = excluded.snippet,
            percent = excluded.percent,
            server_seq = excluded.server_seq,
            updated_at_ms = excluded.updated_at_ms",
        params![
            pos.book_id,
            pos.device_id,
            canonical,
            pos.hlc_wall_ms as i64,
            i64::from(pos.hlc_logical),
            pos.hlc_node,
            pos.locator,
            pos.snippet,
            pos.percent,
            pos.server_seq as i64,
            pos.updated_at_ms as i64,
        ],
    )?;
    Ok(())
}

fn upsert_device_position_in_tx(
    tx: &Transaction<'_>,
    is_local: bool,
    pos: &PositionRecord,
) -> Result<(), rusqlite::Error> {
    let canonical = Hlc::parse(&pos.hlc)
        .map(|h| h.to_canonical_string())
        .unwrap_or_else(|_| pos.hlc.clone());
    tx.execute(
        "INSERT INTO device_positions (
            book_id, is_local, device_id, hlc_canonical, hlc_wall_ms, hlc_logical, hlc_node,
            locator, snippet, percent, server_seq, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(book_id, is_local) DO UPDATE SET
            device_id = excluded.device_id,
            hlc_canonical = excluded.hlc_canonical,
            hlc_wall_ms = excluded.hlc_wall_ms,
            hlc_logical = excluded.hlc_logical,
            hlc_node = excluded.hlc_node,
            locator = excluded.locator,
            snippet = excluded.snippet,
            percent = excluded.percent,
            server_seq = excluded.server_seq,
            updated_at_ms = excluded.updated_at_ms",
        params![
            pos.book_id,
            i32::from(is_local),
            pos.device_id,
            canonical,
            pos.hlc_wall_ms as i64,
            i64::from(pos.hlc_logical),
            pos.hlc_node,
            pos.locator,
            pos.snippet,
            pos.percent,
            pos.server_seq as i64,
            pos.updated_at_ms as i64,
        ],
    )?;
    Ok(())
}

fn get_position_in_tx(
    tx: &Transaction<'_>,
    book_id: &str,
) -> Result<Option<PositionRecord>, rusqlite::Error> {
    tx.query_row(
        "SELECT book_id, device_id, hlc_canonical, hlc_wall_ms, hlc_logical, hlc_node,
                locator, snippet, percent, server_seq, updated_at_ms
         FROM positions
         WHERE book_id = ?1",
        params![book_id],
        map_position_row,
    )
    .optional()
}

fn query_position_from_conn(
    conn: &Connection,
    book_id: &str,
) -> Result<Option<PositionRecord>, rusqlite::Error> {
    conn.query_row(
        "SELECT book_id, device_id, hlc_canonical, hlc_wall_ms, hlc_logical, hlc_node,
                locator, snippet, percent, server_seq, updated_at_ms
         FROM positions
         WHERE book_id = ?1",
        params![book_id],
        map_position_row,
    )
    .optional()
}

fn query_device_position_in_tx(
    tx: &Transaction<'_>,
    book_id: &str,
    is_local: bool,
) -> Result<Option<PositionRecord>, rusqlite::Error> {
    tx.query_row(
        "SELECT book_id, device_id, hlc_canonical, hlc_wall_ms, hlc_logical, hlc_node,
                locator, snippet, percent, server_seq, updated_at_ms
         FROM device_positions
         WHERE book_id = ?1 AND is_local = ?2",
        params![book_id, i32::from(is_local)],
        map_position_row,
    )
    .optional()
}

fn query_device_position_from_conn(
    conn: &Connection,
    book_id: &str,
    is_local: bool,
) -> Result<Option<PositionRecord>, rusqlite::Error> {
    conn.query_row(
        "SELECT book_id, device_id, hlc_canonical, hlc_wall_ms, hlc_logical, hlc_node,
                locator, snippet, percent, server_seq, updated_at_ms
         FROM device_positions
         WHERE book_id = ?1 AND is_local = ?2",
        params![book_id, i32::from(is_local)],
        map_position_row,
    )
    .optional()
}

fn map_position_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PositionRecord> {
    Ok(PositionRecord {
        book_id: r.get(0)?,
        device_id: r.get(1)?,
        hlc: r.get(2)?,
        hlc_wall_ms: r.get::<_, i64>(3)?.max(0) as u64,
        hlc_logical: r.get::<_, i64>(4)?.max(0) as u32,
        hlc_node: r.get(5)?,
        locator: r.get(6)?,
        snippet: r.get(7)?,
        percent: r.get(8)?,
        server_seq: r.get::<_, i64>(9)?.max(0) as u64,
        updated_at_ms: r.get::<_, i64>(10)?.max(0) as u64,
    })
}

fn sanitize_hlc_node(raw: &str, fallback: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_graphic() && !c.is_ascii_control() && *c != ':')
        .take(64)
        .collect();
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned
    }
}

fn run_migrations(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version < 1 {
        let tx = conn.transaction()?;
        tx.execute_batch(CLIENT_SCHEMA_V1_DDL)?;
        tx.pragma_update(None, "user_version", 1)?;
        tx.commit()?;
    }
    if version < 2 {
        // v2: pasta do livro no cache da biblioteca (navegacao por subpastas na UI).
        let tx = conn.transaction()?;
        tx.execute_batch("ALTER TABLE cached_books ADD COLUMN folder TEXT NOT NULL DEFAULT '';")?;
        tx.pragma_update(None, "user_version", 2)?;
        tx.commit()?;
    }
    if version < 3 {
        // v3: nome de exibicao, capa e "Continuar lendo" (metadados editaveis do servidor).
        let tx = conn.transaction()?;
        tx.execute_batch(
            "ALTER TABLE cached_books ADD COLUMN display_title TEXT NULL;
             ALTER TABLE cached_books ADD COLUMN cover_version INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE cached_books ADD COLUMN cover_source TEXT NULL;
             ALTER TABLE cached_books ADD COLUMN hidden_continue_at_ms INTEGER NULL;",
        )?;
        tx.pragma_update(None, "user_version", CURRENT_CLIENT_SCHEMA_VERSION)?;
        tx.commit()?;
    }
    Ok(())
}

pub fn ensure_file_0600(path: &Path) -> Result<(), ClientError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let _f = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _f = OpenOptions::new().create(true).append(true).open(path)?;
    }
    Ok(())
}

pub fn write_secret_file_0600(path: &Path, bytes: &[u8]) -> Result<(), ClientError> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        fs::write(path, bytes)?;
    }
    Ok(())
}

pub fn enforce_permissions_0600(path: &Path) -> Result<(), ClientError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if path.exists() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

#[cfg(test)]
mod outbox_race_tests {
    use super::*;

    /// Escrita local feita ENQUANTO o PUT anterior estava em voo: ao concluir o PUT antigo,
    /// a escrita mais nova NAO pode sumir da outbox (senao a posicao nunca chegaria ao servidor).
    #[test]
    fn completing_old_put_keeps_newer_local_write_in_outbox() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = ClientDatabase::open(dir.path(), "teste").expect("open");
        let book = BookId::from_parts("d41d8cd98f00b204e9800998ecf8427e", 1234).expect("book id");

        let sent = db
            .set_local_position(&book, "page:10", None, 10.0, 1_000_000)
            .expect("set 1");
        let newer = db
            .set_local_position(&book, "page:11", None, 11.0, 1_000_001)
            .expect("set 2");
        assert!(newer.hlc > sent.hlc, "HLC deve crescer");

        let mut acked = sent.clone();
        acked.server_seq = 1;
        db.complete_outbox_put(book.as_str(), &sent.hlc, true, &acked, &sent.hlc, 1_000_002)
            .expect("complete");

        let pending = db.list_outbox().expect("outbox");
        assert_eq!(
            pending.len(),
            1,
            "a escrita mais nova deve continuar pendente"
        );
        assert_eq!(pending[0].locator, "page:11");
    }
}

#[cfg(test)]
mod folder_migration_tests {
    use super::*;

    /// Banco v1 existente (sem a coluna `folder`) deve migrar para v2 sem perder o cache e
    /// guardar/ler a pasta dos livros novos.
    #[test]
    fn book_meta_roundtrips_and_noop_is_detected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = ClientDatabase::open(dir.path(), "teste").expect("open");
        let id = "d41d8cd98f00b204e9800998ecf8427e-10";
        let entry = ServerBookEntry {
            book_id: id.into(),
            partial_md5: "d41d8cd98f00b204e9800998ecf8427e".into(),
            title: "Original".into(),
            format: BookFormat::Pdf,
            size_bytes: 10,
            mtime_ms: 1,
            folder: String::new(),
            display_title: None,
            cover_version: 0,
            cover_source: None,
            hidden_continue_at_ms: None,
            position: None,
        };
        db.store_library_snapshot(std::slice::from_ref(&entry), 5)
            .expect("snap");
        let meta = BookMeta {
            book_id: id.into(),
            display_title: Some("Meu nome".into()),
            cover_version: 7,
            cover_source: Some("custom".into()),
            hidden_continue_at_ms: Some(99),
        };
        assert!(db.apply_book_meta(&meta).expect("apply"));
        assert!(
            !db.apply_book_meta(&meta).expect("apply de novo"),
            "igual = sem mudanca"
        );
        let books = db.load_cached_library().expect("load");
        assert_eq!(books[0].display_title.as_deref(), Some("Meu nome"));
        assert_eq!(books[0].title, "Original");
        assert_eq!(
            (books[0].cover_version, books[0].hidden_continue_at_ms),
            (7, Some(99))
        );
        assert_eq!(db.cover_version(id).expect("v"), 7);
        // Meta de livro desconhecido nao cria linha.
        let ghost = BookMeta {
            book_id: "ffffffffffffffffffffffffffffffff-1".into(),
            ..BookMeta::default()
        };
        assert!(!db.apply_book_meta(&ghost).expect("ghost"));
        // Mudar so a meta conta como mudanca da biblioteca no proximo snapshot.
        let mut changed = entry;
        changed.display_title = Some("Outro".into());
        let (_, lib_changed, _) = db.store_library_snapshot(&[changed], 6).expect("snap2");
        assert!(lib_changed);
    }

    #[test]
    fn v1_cache_migrates_and_roundtrips_folder() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let conn = Connection::open(dir.path().join(CLIENT_DB_FILENAME)).expect("conn");
            conn.execute_batch(CLIENT_SCHEMA_V1_DDL).expect("ddl v1");
            conn.pragma_update(None, "user_version", 1).expect("v1");
            conn.execute(
                "INSERT INTO cached_books (book_id, partial_md5, title, format, size_bytes, mtime_ms, etag, updated_at_ms)
                 VALUES ('d41d8cd98f00b204e9800998ecf8427e-10', 'd41d8cd98f00b204e9800998ecf8427e', 'Antigo', 'pdf', 10, 1, NULL, 1)",
                [],
            )
            .expect("insert v1");
        }
        let db = ClientDatabase::open(dir.path(), "teste").expect("open migra");
        let books = db.load_cached_library().expect("load");
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].folder, "", "livro antigo cai na raiz");

        let entry = ServerBookEntry {
            book_id: "d41d8cd98f00b204e9800998ecf8427e-10".into(),
            partial_md5: "d41d8cd98f00b204e9800998ecf8427e".into(),
            title: "Antigo".into(),
            format: BookFormat::Pdf,
            size_bytes: 10,
            mtime_ms: 1,
            folder: "dnd/regras".into(),
            display_title: None,
            cover_version: 0,
            cover_source: None,
            hidden_continue_at_ms: None,
            position: None,
        };
        let (books, changed, _) = db.store_library_snapshot(&[entry], 5).expect("snapshot");
        assert!(changed, "mudar so a pasta conta como mudanca da biblioteca");
        assert_eq!(books[0].folder, "dnd/regras");
    }
}

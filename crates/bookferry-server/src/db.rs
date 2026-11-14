use crate::error::ServerError;
use bookferry_core::{
    constant_time_eq_str, decide_lww, BookFormat, BookId, Hlc, LwwDecision, PositionHistoryEntry,
    PositionRecord, ValidatedPositionUpdate,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

pub const DB_FILENAME: &str = "bookferry.db";
pub const CURRENT_SCHEMA_VERSION: i32 = 2;

/// DDL completo da versao 1 do schema SQLite.
pub const SCHEMA_V1_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

INSERT OR IGNORE INTO meta (key, value) VALUES ('server_seq', '0');

CREATE TABLE IF NOT EXISTS devices (
    id              TEXT PRIMARY KEY NOT NULL,
    name            TEXT NOT NULL,
    token_sha256    TEXT NOT NULL UNIQUE,
    created_at_ms   INTEGER NOT NULL,
    last_seen_at_ms INTEGER NOT NULL,
    revoked_at_ms   INTEGER NULL
);

CREATE INDEX IF NOT EXISTS idx_devices_token_sha256 ON devices(token_sha256);

CREATE TABLE IF NOT EXISTS books (
    book_id       TEXT PRIMARY KEY NOT NULL,
    partial_md5   TEXT NOT NULL,
    size_bytes    INTEGER NOT NULL,
    title         TEXT NOT NULL,
    format        TEXT NOT NULL CHECK (format IN ('epub', 'pdf', 'cbz')),
    mtime_ms      INTEGER NOT NULL,
    rel_path      TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS positions (
    book_id       TEXT PRIMARY KEY NOT NULL REFERENCES books(book_id) ON DELETE CASCADE,
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

CREATE INDEX IF NOT EXISTS idx_positions_hlc ON positions(hlc_canonical);

CREATE TABLE IF NOT EXISTS position_history (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    book_id        TEXT NOT NULL REFERENCES books(book_id) ON DELETE CASCADE,
    device_id      TEXT NOT NULL,
    hlc_canonical  TEXT NOT NULL,
    hlc_wall_ms    INTEGER NOT NULL,
    hlc_logical    INTEGER NOT NULL,
    hlc_node       TEXT NOT NULL,
    locator        TEXT NOT NULL,
    snippet        TEXT NULL,
    percent        REAL NOT NULL,
    applied        INTEGER NOT NULL CHECK (applied IN (0, 1)),
    server_seq     INTEGER NOT NULL,
    recorded_at_ms INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_position_history_book_id ON position_history(book_id, id DESC);

CREATE TABLE IF NOT EXISTS sse_events (
    seq              INTEGER PRIMARY KEY NOT NULL,
    event_type       TEXT NOT NULL,
    book_id          TEXT NOT NULL,
    origin_device_id TEXT NOT NULL,
    payload_json     TEXT NOT NULL,
    created_at_ms    INTEGER NOT NULL
);
"#;

/// DDL da versao 2: metadados editaveis por livro (nome, capa, "Continuar lendo").
pub const SCHEMA_V2_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS book_meta (
    book_id               TEXT PRIMARY KEY NOT NULL REFERENCES books(book_id) ON DELETE CASCADE,
    display_title         TEXT NULL,
    cover_version         INTEGER NOT NULL DEFAULT 0,
    cover_source          TEXT NULL CHECK (cover_source IN ('embedded', 'custom', 'web')),
    hidden_continue_at_ms INTEGER NULL,
    server_seq            INTEGER NOT NULL,
    updated_at_ms         INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS book_covers (
    book_id TEXT PRIMARY KEY NOT NULL REFERENCES books(book_id) ON DELETE CASCADE,
    mime    TEXT NOT NULL,
    data    BLOB NOT NULL
);
"#;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceSummary {
    pub id: String,
    pub name: String,
    pub created_at_ms: u64,
    pub last_seen_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_at_ms: Option<u64>,
    pub revoked: bool,
}

#[derive(Clone, Debug)]
pub struct AuthenticatedDevice {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct ScannedBook {
    pub book_id: BookId,
    pub partial_md5: String,
    pub size_bytes: u64,
    pub title: String,
    pub format: BookFormat,
    pub mtime_ms: u64,
    pub rel_path: String,
}

#[derive(Clone, Debug)]
pub struct BookRow {
    pub book_id: String,
    pub partial_md5: String,
    pub size_bytes: u64,
    pub title: String,
    pub format: BookFormat,
    pub mtime_ms: u64,
    pub rel_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BookWithPosition {
    pub book_id: String,
    pub partial_md5: String,
    pub title: String,
    pub format: BookFormat,
    pub size_bytes: u64,
    pub mtime_ms: u64,
    /// Pasta do livro relativa ao `vault_dir`, separada por `/` (`""` = raiz). Nunca inclui o
    /// nome do arquivo nem caminho absoluto.
    #[serde(default)]
    pub folder: String,
    /// Nome escolhido pelo usuario (substitui `title` na exibicao; `title` continua o original).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_title: Option<String>,
    /// `0` = sem capa; senao serve de cache-buster para `GET /books/:id/cover?v=`.
    #[serde(default)]
    pub cover_version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden_continue_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<PositionRecord>,
}

/// Pasta (sem o nome do arquivo) de um `rel_path` `a/b/livro.pdf` -> `a/b`; arquivo na raiz -> `""`.
pub fn folder_of(rel_path: &str) -> String {
    match rel_path.rsplit_once('/') {
        Some((dir, _file)) => dir.to_string(),
        None => String::new(),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SsePositionPayload {
    pub seq: u64,
    pub book_id: String,
    pub origin_device_id: String,
    pub position: PositionRecord,
}

#[derive(Clone, Debug)]
pub struct PersistedSseEvent {
    pub seq: u64,
    pub event_type: String,
    pub book_id: String,
    pub origin_device_id: String,
    pub payload_json: String,
}

#[derive(Clone, Debug)]
pub struct PutPositionOutcome {
    pub applied: bool,
    pub position: PositionRecord,
    pub server_seq: u64,
    pub history_count: usize,
    pub sse_event: Option<PersistedSseEvent>,
}

pub enum BookPositionLookup {
    BookNotFound,
    NoPositionYet,
    Found {
        position: Box<PositionRecord>,
        history: Vec<PositionHistoryEntry>,
    },
}

pub struct Database {
    path: PathBuf,
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open(data_dir: &Path) -> Result<Self, ServerError> {
        std::fs::create_dir_all(data_dir)?;
        let db_path = data_dir.join(DB_FILENAME);
        // Cria o arquivo com 0600 antes do SQLite: os arquivos -wal/-shm herdam essa permissao.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(&db_path)?;
        }
        let mut conn = Connection::open(&db_path)?;
        conn.busy_timeout(Duration::from_millis(5_000))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        run_migrations(&mut conn)?;

        Ok(Self {
            path: db_path,
            conn: Mutex::new(conn),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn lock_conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn highest_stored_hlc(&self) -> Result<Option<Hlc>, rusqlite::Error> {
        let conn = self.lock_conn();
        let raw: Option<String> = conn
            .query_row(
                "SELECT hlc_canonical FROM positions ORDER BY hlc_canonical DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(raw.and_then(|s| Hlc::parse(&s).ok()))
    }

    pub fn insert_device(
        &self,
        id: &str,
        name: &str,
        token_sha256: &str,
        now_ms: u64,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.lock_conn();
        conn.execute(
            "INSERT INTO devices (id, name, token_sha256, created_at_ms, last_seen_at_ms, revoked_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?4, NULL)",
            params![id, name, token_sha256, now_ms as i64],
        )?;
        Ok(())
    }

    /// Autentica o hash SHA-256 de um token percorrendo os dispositivos ativos
    /// com comparacao em tempo constante (`subtle::ConstantTimeEq`).
    pub fn authenticate_token_sha256(
        &self,
        candidate_sha256: &str,
        now_ms: u64,
    ) -> Result<Option<AuthenticatedDevice>, rusqlite::Error> {
        let conn = self.lock_conn();
        let mut stmt =
            conn.prepare("SELECT id, name, token_sha256 FROM devices WHERE revoked_at_ms IS NULL")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;

        let mut matched: Option<AuthenticatedDevice> = None;
        for row in rows {
            let (id, name, stored_hash) = row?;
            if constant_time_eq_str(&stored_hash, candidate_sha256) {
                matched = Some(AuthenticatedDevice { id, name });
            }
        }
        drop(stmt);

        if let Some(ref dev) = matched {
            conn.execute(
                "UPDATE devices SET last_seen_at_ms = ?1 WHERE id = ?2",
                params![now_ms as i64, dev.id],
            )?;
        }

        Ok(matched)
    }

    pub fn list_devices(&self) -> Result<Vec<DeviceSummary>, rusqlite::Error> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, created_at_ms, last_seen_at_ms, revoked_at_ms
             FROM devices
             ORDER BY created_at_ms ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let created_at: i64 = row.get(2)?;
            let last_seen: i64 = row.get(3)?;
            let revoked_at: Option<i64> = row.get(4)?;
            Ok(DeviceSummary {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at_ms: created_at.max(0) as u64,
                last_seen_at_ms: last_seen.max(0) as u64,
                revoked_at_ms: revoked_at.map(|v| v.max(0) as u64),
                revoked: revoked_at.is_some(),
            })
        })?;
        rows.collect()
    }

    pub fn revoke_device(&self, device_id: &str, now_ms: u64) -> Result<bool, rusqlite::Error> {
        let conn = self.lock_conn();
        let affected = conn.execute(
            "UPDATE devices
             SET revoked_at_ms = COALESCE(revoked_at_ms, ?1)
             WHERE id = ?2",
            params![now_ms as i64, device_id],
        )?;
        Ok(affected > 0)
    }

    pub fn upsert_scanned_books(
        &self,
        books: &[ScannedBook],
        now_ms: u64,
    ) -> Result<usize, rusqlite::Error> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for book in books {
            tx.execute(
                "INSERT INTO books (book_id, partial_md5, size_bytes, title, format, mtime_ms, rel_path, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(book_id) DO UPDATE SET
                    title = excluded.title,
                    format = excluded.format,
                    mtime_ms = excluded.mtime_ms,
                    rel_path = excluded.rel_path,
                    updated_at_ms = excluded.updated_at_ms",
                params![
                    book.book_id.as_str(),
                    book.partial_md5,
                    book.size_bytes as i64,
                    book.title,
                    book.format.as_str(),
                    book.mtime_ms as i64,
                    book.rel_path,
                    now_ms as i64,
                ],
            )?;
        }
        tx.commit()?;
        Ok(books.len())
    }

    pub fn count_books(&self) -> Result<u64, rusqlite::Error> {
        let conn = self.lock_conn();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM books", [], |row| row.get(0))?;
        Ok(count.max(0) as u64)
    }

    pub fn get_book_by_id(&self, book_id: &BookId) -> Result<Option<BookRow>, rusqlite::Error> {
        let conn = self.lock_conn();
        conn.query_row(
            "SELECT book_id, partial_md5, size_bytes, title, format, mtime_ms, rel_path
             FROM books
             WHERE book_id = ?1",
            params![book_id.as_str()],
            |row| {
                let fmt_str: String = row.get(4)?;
                let format = BookFormat::from_extension(&fmt_str).unwrap_or(BookFormat::Epub);
                Ok(BookRow {
                    book_id: row.get(0)?,
                    partial_md5: row.get(1)?,
                    size_bytes: row.get::<_, i64>(2)?.max(0) as u64,
                    title: row.get(3)?,
                    format,
                    mtime_ms: row.get::<_, i64>(5)?.max(0) as u64,
                    rel_path: row.get(6)?,
                })
            },
        )
        .optional()
    }

    pub fn list_books_with_positions(&self) -> Result<Vec<BookWithPosition>, rusqlite::Error> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT
                b.book_id, b.partial_md5, b.title, b.format, b.size_bytes, b.mtime_ms,
                p.device_id, p.hlc_canonical, p.hlc_wall_ms, p.hlc_logical, p.hlc_node,
                p.locator, p.snippet, p.percent, p.server_seq, p.updated_at_ms,
                b.rel_path,
                m.display_title, m.cover_version, m.cover_source, m.hidden_continue_at_ms
             FROM books b
             LEFT JOIN positions p ON b.book_id = p.book_id
             LEFT JOIN book_meta m ON b.book_id = m.book_id
             ORDER BY b.title ASC, b.book_id ASC",
        )?;

        let rows = stmt.query_map([], |row| {
            let book_id: String = row.get(0)?;
            let fmt_str: String = row.get(3)?;
            let format = BookFormat::from_extension(&fmt_str).unwrap_or(BookFormat::Epub);
            let pos_device: Option<String> = row.get(6)?;
            let position = match pos_device {
                Some(device_id) => Some(PositionRecord {
                    book_id: book_id.clone(),
                    device_id,
                    hlc: row.get(7)?,
                    hlc_wall_ms: row.get::<_, i64>(8)?.max(0) as u64,
                    hlc_logical: row.get::<_, i64>(9)?.max(0) as u32,
                    hlc_node: row.get(10)?,
                    locator: row.get(11)?,
                    snippet: row.get(12)?,
                    percent: row.get(13)?,
                    server_seq: row.get::<_, i64>(14)?.max(0) as u64,
                    updated_at_ms: row.get::<_, i64>(15)?.max(0) as u64,
                }),
                None => None,
            };

            Ok(BookWithPosition {
                book_id,
                partial_md5: row.get(1)?,
                title: row.get(2)?,
                format,
                size_bytes: row.get::<_, i64>(4)?.max(0) as u64,
                mtime_ms: row.get::<_, i64>(5)?.max(0) as u64,
                folder: folder_of(&row.get::<_, String>(16)?),
                display_title: row.get(17)?,
                cover_version: row.get::<_, Option<i64>>(18)?.unwrap_or(0).max(0) as u64,
                cover_source: row.get(19)?,
                hidden_continue_at_ms: row.get::<_, Option<i64>>(20)?.map(|v| v.max(0) as u64),
                position,
            })
        })?;
        rows.collect()
    }

    pub fn put_position(
        &self,
        book_id: &BookId,
        update: ValidatedPositionUpdate,
        now_ms: u64,
        history_limit: usize,
        sse_replay_limit: usize,
    ) -> Result<Option<PutPositionOutcome>, rusqlite::Error> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let exists: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM books WHERE book_id = ?1",
                params![book_id.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Ok(None);
        }

        // Incrementa server_seq monotonicamente dentro da transacao IMMEDIATE
        tx.execute(
            "UPDATE meta SET value = CAST((CAST(value AS INTEGER) + 1) AS TEXT) WHERE key = 'server_seq'",
            [],
        )?;
        let seq_str: String =
            tx.query_row("SELECT value FROM meta WHERE key = 'server_seq'", [], |r| {
                r.get(0)
            })?;
        let server_seq: u64 = seq_str.parse().unwrap_or(1);

        let existing: Option<PositionRecord> = tx
            .query_row(
                "SELECT book_id, device_id, hlc_canonical, hlc_wall_ms, hlc_logical, hlc_node,
                        locator, snippet, percent, server_seq, updated_at_ms
                 FROM positions
                 WHERE book_id = ?1",
                params![book_id.as_str()],
                |row| {
                    Ok(PositionRecord {
                        book_id: row.get(0)?,
                        device_id: row.get(1)?,
                        hlc: row.get(2)?,
                        hlc_wall_ms: row.get::<_, i64>(3)?.max(0) as u64,
                        hlc_logical: row.get::<_, i64>(4)?.max(0) as u32,
                        hlc_node: row.get(5)?,
                        locator: row.get(6)?,
                        snippet: row.get(7)?,
                        percent: row.get(8)?,
                        server_seq: row.get::<_, i64>(9)?.max(0) as u64,
                        updated_at_ms: row.get::<_, i64>(10)?.max(0) as u64,
                    })
                },
            )
            .optional()?;

        let current_hlc = existing.as_ref().and_then(|rec| Hlc::parse(&rec.hlc).ok());
        let decision = decide_lww(current_hlc.as_ref(), &update.hlc);
        let applied = decision == LwwDecision::Apply;
        let canonical = update.hlc.to_canonical_string();

        // Registra no historico (tanto aplicadas quanto rejeitadas)
        tx.execute(
            "INSERT INTO position_history (
                book_id, device_id, hlc_canonical, hlc_wall_ms, hlc_logical, hlc_node,
                locator, snippet, percent, applied, server_seq, recorded_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                book_id.as_str(),
                update.device_id,
                canonical,
                update.hlc.wall_ms as i64,
                update.hlc.logical as i64,
                update.hlc.node,
                update.locator,
                update.snippet,
                update.percent,
                i32::from(applied),
                server_seq as i64,
                now_ms as i64,
            ],
        )?;

        // Retem apenas as ultimas `history_limit` entradas por livro
        tx.execute(
            "DELETE FROM position_history
             WHERE book_id = ?1
               AND id NOT IN (
                   SELECT id FROM position_history
                   WHERE book_id = ?1
                   ORDER BY id DESC
                   LIMIT ?2
               )",
            params![book_id.as_str(), history_limit.max(1) as i64],
        )?;

        let (winning_position, sse_event) = if applied {
            let new_pos = PositionRecord {
                book_id: book_id.as_str().to_string(),
                device_id: update.device_id.clone(),
                hlc: canonical.clone(),
                hlc_wall_ms: update.hlc.wall_ms,
                hlc_logical: update.hlc.logical,
                hlc_node: update.hlc.node.clone(),
                locator: update.locator,
                snippet: update.snippet,
                percent: update.percent,
                server_seq,
                updated_at_ms: now_ms,
            };

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
                    new_pos.book_id,
                    new_pos.device_id,
                    new_pos.hlc,
                    new_pos.hlc_wall_ms as i64,
                    new_pos.hlc_logical as i64,
                    new_pos.hlc_node,
                    new_pos.locator,
                    new_pos.snippet,
                    new_pos.percent,
                    new_pos.server_seq as i64,
                    new_pos.updated_at_ms as i64,
                ],
            )?;

            let payload = SsePositionPayload {
                seq: server_seq,
                book_id: new_pos.book_id.clone(),
                origin_device_id: new_pos.device_id.clone(),
                position: new_pos.clone(),
            };
            let payload_json = serde_json::to_string(&payload).unwrap_or_default();

            tx.execute(
                "INSERT INTO sse_events (seq, event_type, book_id, origin_device_id, payload_json, created_at_ms)
                 VALUES (?1, 'position', ?2, ?3, ?4, ?5)",
                params![
                    server_seq as i64,
                    new_pos.book_id,
                    new_pos.device_id,
                    payload_json,
                    now_ms as i64,
                ],
            )?;

            tx.execute(
                "DELETE FROM sse_events
                 WHERE seq NOT IN (
                     SELECT seq FROM sse_events
                     ORDER BY seq DESC
                     LIMIT ?1
                 )",
                params![sse_replay_limit.max(1) as i64],
            )?;

            (
                new_pos.clone(),
                Some(PersistedSseEvent {
                    seq: server_seq,
                    event_type: "position".to_string(),
                    book_id: new_pos.book_id,
                    origin_device_id: new_pos.device_id,
                    payload_json,
                }),
            )
        } else {
            let winner = existing.unwrap_or_else(|| PositionRecord {
                book_id: book_id.as_str().to_string(),
                device_id: update.device_id,
                hlc: canonical,
                hlc_wall_ms: update.hlc.wall_ms,
                hlc_logical: update.hlc.logical,
                hlc_node: update.hlc.node,
                locator: update.locator,
                snippet: update.snippet,
                percent: update.percent,
                server_seq,
                updated_at_ms: now_ms,
            });
            (winner, None)
        };

        let history_count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM position_history WHERE book_id = ?1",
            params![book_id.as_str()],
            |r| r.get(0),
        )?;

        tx.commit()?;

        Ok(Some(PutPositionOutcome {
            applied,
            position: winning_position,
            server_seq,
            history_count: history_count.max(0) as usize,
            sse_event,
        }))
    }

    pub fn get_book_position(
        &self,
        book_id: &BookId,
        history_limit: usize,
    ) -> Result<BookPositionLookup, rusqlite::Error> {
        let conn = self.lock_conn();
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM books WHERE book_id = ?1",
                params![book_id.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Ok(BookPositionLookup::BookNotFound);
        }

        let position: Option<PositionRecord> = conn
            .query_row(
                "SELECT book_id, device_id, hlc_canonical, hlc_wall_ms, hlc_logical, hlc_node,
                        locator, snippet, percent, server_seq, updated_at_ms
                 FROM positions
                 WHERE book_id = ?1",
                params![book_id.as_str()],
                |row| {
                    Ok(PositionRecord {
                        book_id: row.get(0)?,
                        device_id: row.get(1)?,
                        hlc: row.get(2)?,
                        hlc_wall_ms: row.get::<_, i64>(3)?.max(0) as u64,
                        hlc_logical: row.get::<_, i64>(4)?.max(0) as u32,
                        hlc_node: row.get(5)?,
                        locator: row.get(6)?,
                        snippet: row.get(7)?,
                        percent: row.get(8)?,
                        server_seq: row.get::<_, i64>(9)?.max(0) as u64,
                        updated_at_ms: row.get::<_, i64>(10)?.max(0) as u64,
                    })
                },
            )
            .optional()?;

        let Some(position) = position else {
            return Ok(BookPositionLookup::NoPositionYet);
        };

        let mut stmt = conn.prepare(
            "SELECT book_id, device_id, hlc_canonical, locator, snippet, percent, applied, server_seq, recorded_at_ms
             FROM position_history
             WHERE book_id = ?1
             ORDER BY id DESC
             LIMIT ?2",
        )?;
        let history_rows = stmt.query_map(
            params![book_id.as_str(), history_limit.max(1) as i64],
            |row| {
                Ok(PositionHistoryEntry {
                    book_id: row.get(0)?,
                    device_id: row.get(1)?,
                    hlc: row.get(2)?,
                    locator: row.get(3)?,
                    snippet: row.get(4)?,
                    percent: row.get(5)?,
                    applied: row.get::<_, i32>(6)? != 0,
                    server_seq: row.get::<_, i64>(7)?.max(0) as u64,
                    recorded_at_ms: row.get::<_, i64>(8)?.max(0) as u64,
                })
            },
        )?;
        let history: Vec<PositionHistoryEntry> = history_rows.collect::<Result<_, _>>()?;

        Ok(BookPositionLookup::Found {
            position: Box::new(position),
            history,
        })
    }

    pub fn list_positions_since(
        &self,
        since_canonical: Option<&str>,
    ) -> Result<Vec<PositionRecord>, rusqlite::Error> {
        let conn = self.lock_conn();
        match since_canonical {
            Some(since) => {
                let mut stmt = conn.prepare(
                    "SELECT book_id, device_id, hlc_canonical, hlc_wall_ms, hlc_logical, hlc_node,
                            locator, snippet, percent, server_seq, updated_at_ms
                     FROM positions
                     WHERE hlc_canonical > ?1
                     ORDER BY hlc_canonical ASC",
                )?;
                let rows = stmt.query_map(params![since], map_position_row)?;
                rows.collect()
            }
            None => {
                let mut stmt = conn.prepare(
                    "SELECT book_id, device_id, hlc_canonical, hlc_wall_ms, hlc_logical, hlc_node,
                            locator, snippet, percent, server_seq, updated_at_ms
                     FROM positions
                     ORDER BY hlc_canonical ASC",
                )?;
                let rows = stmt.query_map([], map_position_row)?;
                rows.collect()
            }
        }
    }

    pub fn list_sse_events_after(
        &self,
        last_event_id: u64,
    ) -> Result<Vec<PersistedSseEvent>, rusqlite::Error> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT seq, event_type, book_id, origin_device_id, payload_json
             FROM sse_events
             WHERE seq > ?1
             ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(params![last_event_id as i64], |row| {
            Ok(PersistedSseEvent {
                seq: row.get::<_, i64>(0)?.max(0) as u64,
                event_type: row.get(1)?,
                book_id: row.get(2)?,
                origin_device_id: row.get(3)?,
                payload_json: row.get(4)?,
            })
        })?;
        rows.collect()
    }
}

fn map_position_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PositionRecord> {
    Ok(PositionRecord {
        book_id: row.get(0)?,
        device_id: row.get(1)?,
        hlc: row.get(2)?,
        hlc_wall_ms: row.get::<_, i64>(3)?.max(0) as u64,
        hlc_logical: row.get::<_, i64>(4)?.max(0) as u32,
        hlc_node: row.get(5)?,
        locator: row.get(6)?,
        snippet: row.get(7)?,
        percent: row.get(8)?,
        server_seq: row.get::<_, i64>(9)?.max(0) as u64,
        updated_at_ms: row.get::<_, i64>(10)?.max(0) as u64,
    })
}

fn run_migrations(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version < 1 {
        let tx = conn.transaction()?;
        tx.execute_batch(SCHEMA_V1_DDL)?;
        tx.pragma_update(None, "user_version", 1)?;
        tx.commit()?;
    }
    if version < 2 {
        let tx = conn.transaction()?;
        tx.execute_batch(SCHEMA_V2_DDL)?;
        tx.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod permission_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn database_files_are_owner_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(dir.path()).expect("open");
        drop(db);
        for entry in std::fs::read_dir(dir.path()).expect("read_dir") {
            let entry = entry.expect("entry");
            let mode = entry.metadata().expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{:?} deve ser 0600", entry.file_name());
        }
    }
}

#[cfg(test)]
mod folder_tests {
    use super::folder_of;

    #[test]
    fn folder_of_strips_file_name() {
        assert_eq!(folder_of("livro.pdf"), "");
        assert_eq!(
            folder_of("hqs/DC/Swamp Thing/vol1.cbz"),
            "hqs/DC/Swamp Thing"
        );
        assert_eq!(folder_of("a/b.epub"), "a");
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    #[test]
    fn v1_database_upgrades_to_v2_keeping_data() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let conn = Connection::open(dir.path().join(DB_FILENAME)).expect("v1 conn");
            conn.execute_batch(SCHEMA_V1_DDL).expect("ddl v1");
            conn.pragma_update(None, "user_version", 1).expect("v1");
            conn.execute(
                "INSERT INTO books (book_id, partial_md5, size_bytes, title, format, mtime_ms, rel_path, updated_at_ms)
                 VALUES ('0123456789abcdef0123456789abcdef-9', 'x', 9, 'Antigo', 'pdf', 1, 'hqs/antigo.pdf', 1)",
                [],
            )
            .expect("book v1");
        }
        let db = Database::open(dir.path()).expect("open migra v1 -> v2");
        let books = db.list_books_with_positions().expect("list");
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].title, "Antigo");
        assert_eq!(books[0].folder, "hqs");
        assert_eq!(books[0].display_title, None);
        assert_eq!(books[0].cover_version, 0);
        let version: i32 = db
            .lock_conn()
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .expect("version");
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        // Abrir de novo (ja na v2) e idempotente.
        drop(db);
        Database::open(dir.path()).expect("reabrir v2");
    }
}

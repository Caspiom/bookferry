//! Metadados editaveis por livro, sincronizados entre aparelhos: nome de exibicao, capa e
//! "tirar do Continuar lendo". O servidor e a fonte unica: os aparelhos so escrevem aqui e
//! recebem o resultado por SSE (`book_meta`) ou na proxima listagem de `/books`.

use crate::db::{Database, PersistedSseEvent};
use bookferry_core::BookId;
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};

/// Tamanho maximo de uma capa aceita (os clientes redimensionam para ~480 px antes de enviar).
pub const COVER_MAX_BYTES: usize = 2 * 1024 * 1024;
/// Tamanho maximo (em caracteres) do nome de exibicao.
pub const DISPLAY_TITLE_MAX_CHARS: usize = 200;

/// Origem da capa. `Custom` e `Web` sao escolhas do usuario e nunca sao sobrescritas por `Embedded`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CoverSource {
    Embedded,
    Custom,
    Web,
}

impl CoverSource {
    pub fn as_str(self) -> &'static str {
        match self {
            CoverSource::Embedded => "embedded",
            CoverSource::Custom => "custom",
            CoverSource::Web => "web",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "embedded" => Some(CoverSource::Embedded),
            "custom" => Some(CoverSource::Custom),
            "web" => Some(CoverSource::Web),
            _ => None,
        }
    }

    fn is_user_choice(self) -> bool {
        !matches!(self, CoverSource::Embedded)
    }
}

/// Estado dos metadados de um livro, como enviado aos clientes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookMetaRecord {
    pub book_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_title: Option<String>,
    /// `0` = sem capa; senao, o `server_seq` da ultima mudanca de capa (serve de cache-buster).
    #[serde(default)]
    pub cover_version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_source: Option<String>,
    /// Momento (relogio do servidor) em que o usuario tirou o livro do "Continuar lendo".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden_continue_at_ms: Option<u64>,
    #[serde(default)]
    pub server_seq: u64,
    #[serde(default)]
    pub updated_at_ms: u64,
}

impl BookMetaRecord {
    fn empty(book_id: &str) -> Self {
        Self {
            book_id: book_id.to_string(),
            display_title: None,
            cover_version: 0,
            cover_source: None,
            hidden_continue_at_ms: None,
            server_seq: 0,
            updated_at_ms: 0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SseBookMetaPayload {
    pub seq: u64,
    pub book_id: String,
    pub origin_device_id: String,
    pub meta: BookMetaRecord,
}

#[derive(Debug)]
pub struct MetaOutcome {
    /// `false` quando a escrita foi ignorada (ex.: capa embutida sobre capa escolhida pelo usuario).
    pub applied: bool,
    pub meta: BookMetaRecord,
    pub sse_event: Option<PersistedSseEvent>,
}

/// Capa a gravar (ja validada por `sniff_cover_mime`).
#[derive(Clone, Copy, Debug)]
pub struct NewCover<'a> {
    pub source: CoverSource,
    pub mime: &'static str,
    pub data: &'a [u8],
}

#[derive(Debug, PartialEq, Eq)]
pub struct StoredCover {
    pub mime: &'static str,
    pub data: Vec<u8>,
    pub version: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum MetaValidationError {
    TitleTooLong,
    CoverEmpty,
    CoverTooLarge,
    CoverUnsupportedType,
}

impl MetaValidationError {
    pub fn code(&self) -> &'static str {
        match self {
            MetaValidationError::TitleTooLong => "display_title_too_long",
            MetaValidationError::CoverEmpty => "cover_empty",
            MetaValidationError::CoverTooLarge => "cover_too_large",
            MetaValidationError::CoverUnsupportedType => "cover_unsupported_type",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            MetaValidationError::TitleTooLong => {
                "Nome de exibicao longo demais (maximo 200 caracteres)"
            }
            MetaValidationError::CoverEmpty => "Imagem de capa vazia",
            MetaValidationError::CoverTooLarge => "Imagem de capa maior que 2 MiB",
            MetaValidationError::CoverUnsupportedType => {
                "Imagem de capa deve ser JPEG, PNG ou WebP"
            }
        }
    }
}

/// Normaliza o nome de exibicao: apara espacos, tira caracteres de controle e troca vazio por `None`.
pub fn normalize_display_title(raw: Option<&str>) -> Result<Option<String>, MetaValidationError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        return Ok(None);
    }
    if cleaned.chars().count() > DISPLAY_TITLE_MAX_CHARS {
        return Err(MetaValidationError::TitleTooLong);
    }
    Ok(Some(cleaned))
}

/// Detecta o tipo da imagem pelos bytes (nunca pelo cabecalho do cliente). SVG e outros tipos
/// que poderiam carregar script ficam de fora de proposito.
pub fn sniff_cover_mime(data: &[u8]) -> Result<&'static str, MetaValidationError> {
    if data.is_empty() {
        return Err(MetaValidationError::CoverEmpty);
    }
    if data.len() > COVER_MAX_BYTES {
        return Err(MetaValidationError::CoverTooLarge);
    }
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Ok("image/jpeg");
    }
    if data.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Ok("image/png");
    }
    if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Ok("image/webp");
    }
    Err(MetaValidationError::CoverUnsupportedType)
}

fn mime_static(raw: &str) -> &'static str {
    match raw {
        "image/png" => "image/png",
        "image/webp" => "image/webp",
        _ => "image/jpeg",
    }
}

impl Database {
    /// Le os metadados de um livro (`None` se o livro nao existe).
    pub fn get_book_meta(
        &self,
        book_id: &BookId,
    ) -> Result<Option<BookMetaRecord>, rusqlite::Error> {
        let conn = self.lock_conn();
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM books WHERE book_id = ?1",
                params![book_id.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Ok(None);
        }
        Ok(Some(load_meta(&conn, book_id.as_str())?.unwrap_or_else(
            || BookMetaRecord::empty(book_id.as_str()),
        )))
    }

    pub fn get_cover(&self, book_id: &BookId) -> Result<Option<StoredCover>, rusqlite::Error> {
        let conn = self.lock_conn();
        conn.query_row(
            "SELECT c.mime, c.data, m.cover_version
             FROM book_covers c JOIN book_meta m ON m.book_id = c.book_id
             WHERE c.book_id = ?1",
            params![book_id.as_str()],
            |row| {
                let mime: String = row.get(0)?;
                Ok(StoredCover {
                    mime: mime_static(&mime),
                    data: row.get(1)?,
                    version: row.get::<_, i64>(2)?.max(0) as u64,
                })
            },
        )
        .optional()
    }

    /// Define (ou limpa, com `None`) o nome de exibicao.
    pub fn set_display_title(
        &self,
        book_id: &BookId,
        title: Option<String>,
        origin_device_id: &str,
        now_ms: u64,
        sse_replay_limit: usize,
    ) -> Result<Option<MetaOutcome>, rusqlite::Error> {
        self.mutate_meta(
            book_id,
            origin_device_id,
            now_ms,
            sse_replay_limit,
            |_tx, meta| {
                if meta.display_title == title {
                    return Ok(false);
                }
                meta.display_title = title;
                Ok(true)
            },
        )
    }

    /// Tira (`hidden = true`) ou devolve (`false`) o livro do "Continuar lendo". Ao tirar, guarda o
    /// instante: o livro volta sozinho se a posicao de leitura for atualizada depois disso.
    pub fn set_continue_hidden(
        &self,
        book_id: &BookId,
        hidden: bool,
        origin_device_id: &str,
        now_ms: u64,
        sse_replay_limit: usize,
    ) -> Result<Option<MetaOutcome>, rusqlite::Error> {
        self.mutate_meta(
            book_id,
            origin_device_id,
            now_ms,
            sse_replay_limit,
            |_tx, meta| {
                meta.hidden_continue_at_ms = if hidden { Some(now_ms) } else { None };
                Ok(true)
            },
        )
    }

    /// Grava a capa. Capa `Embedded` nao sobrescreve capa escolhida pelo usuario (`applied = false`).
    pub fn put_cover(
        &self,
        book_id: &BookId,
        cover: NewCover<'_>,
        origin_device_id: &str,
        now_ms: u64,
        sse_replay_limit: usize,
    ) -> Result<Option<MetaOutcome>, rusqlite::Error> {
        self.mutate_meta(
            book_id,
            origin_device_id,
            now_ms,
            sse_replay_limit,
            |tx, meta| {
                let existing = meta.cover_source.as_deref().and_then(CoverSource::parse);
                if let Some(existing) = existing {
                    if existing.is_user_choice() && !cover.source.is_user_choice() {
                        return Ok(false);
                    }
                }
                tx.execute(
                    "INSERT INTO book_covers (book_id, mime, data) VALUES (?1, ?2, ?3)
                 ON CONFLICT(book_id) DO UPDATE SET mime = excluded.mime, data = excluded.data",
                    params![meta.book_id, cover.mime, cover.data],
                )?;
                meta.cover_source = Some(cover.source.as_str().to_string());
                // `cover_version` recebe o server_seq da mudanca no fim de `mutate_meta`.
                meta.cover_version = u64::MAX;
                Ok(true)
            },
        )
    }

    /// Remove a capa ("restaurar original": o app desktop gera a embutida de novo depois).
    pub fn delete_cover(
        &self,
        book_id: &BookId,
        origin_device_id: &str,
        now_ms: u64,
        sse_replay_limit: usize,
    ) -> Result<Option<MetaOutcome>, rusqlite::Error> {
        self.mutate_meta(
            book_id,
            origin_device_id,
            now_ms,
            sse_replay_limit,
            |tx, meta| {
                if meta.cover_source.is_none() {
                    return Ok(false);
                }
                tx.execute(
                    "DELETE FROM book_covers WHERE book_id = ?1",
                    params![meta.book_id],
                )?;
                meta.cover_source = None;
                meta.cover_version = 0;
                Ok(true)
            },
        )
    }

    /// Ids de livros que ainda nao tem nenhuma capa (o app desktop gera a embutida para eles).
    pub fn list_book_ids_without_cover(&self) -> Result<Vec<String>, rusqlite::Error> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT b.book_id FROM books b
             LEFT JOIN book_meta m ON m.book_id = b.book_id
             WHERE m.cover_source IS NULL
             ORDER BY b.title ASC, b.book_id ASC",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect()
    }

    /// Transacao comum: valida o livro, deixa `f` alterar o registro e, se algo mudou, avanca o
    /// `server_seq`, persiste e registra o evento SSE `book_meta` (com poda do replay).
    fn mutate_meta<F>(
        &self,
        book_id: &BookId,
        origin_device_id: &str,
        now_ms: u64,
        sse_replay_limit: usize,
        f: F,
    ) -> Result<Option<MetaOutcome>, rusqlite::Error>
    where
        F: FnOnce(&Transaction<'_>, &mut BookMetaRecord) -> Result<bool, rusqlite::Error>,
    {
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

        let mut meta = load_meta(&tx, book_id.as_str())?
            .unwrap_or_else(|| BookMetaRecord::empty(book_id.as_str()));
        let changed = f(&tx, &mut meta)?;
        if !changed {
            tx.commit()?;
            return Ok(Some(MetaOutcome {
                applied: false,
                meta,
                sse_event: None,
            }));
        }

        tx.execute(
            "UPDATE meta SET value = CAST((CAST(value AS INTEGER) + 1) AS TEXT) WHERE key = 'server_seq'",
            [],
        )?;
        let seq_str: String =
            tx.query_row("SELECT value FROM meta WHERE key = 'server_seq'", [], |r| {
                r.get(0)
            })?;
        let server_seq: u64 = seq_str.parse().unwrap_or(1);

        if meta.cover_version == u64::MAX {
            meta.cover_version = server_seq;
        }
        meta.server_seq = server_seq;
        meta.updated_at_ms = now_ms;

        tx.execute(
            "INSERT INTO book_meta (
                book_id, display_title, cover_version, cover_source, hidden_continue_at_ms,
                server_seq, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(book_id) DO UPDATE SET
                display_title = excluded.display_title,
                cover_version = excluded.cover_version,
                cover_source = excluded.cover_source,
                hidden_continue_at_ms = excluded.hidden_continue_at_ms,
                server_seq = excluded.server_seq,
                updated_at_ms = excluded.updated_at_ms",
            params![
                meta.book_id,
                meta.display_title,
                meta.cover_version as i64,
                meta.cover_source,
                meta.hidden_continue_at_ms.map(|v| v as i64),
                meta.server_seq as i64,
                meta.updated_at_ms as i64,
            ],
        )?;

        let payload = SseBookMetaPayload {
            seq: server_seq,
            book_id: meta.book_id.clone(),
            origin_device_id: origin_device_id.to_string(),
            meta: meta.clone(),
        };
        let payload_json = serde_json::to_string(&payload).unwrap_or_default();
        tx.execute(
            "INSERT INTO sse_events (seq, event_type, book_id, origin_device_id, payload_json, created_at_ms)
             VALUES (?1, 'book_meta', ?2, ?3, ?4, ?5)",
            params![
                server_seq as i64,
                meta.book_id,
                origin_device_id,
                payload_json,
                now_ms as i64
            ],
        )?;
        tx.execute(
            "DELETE FROM sse_events
             WHERE seq NOT IN (SELECT seq FROM sse_events ORDER BY seq DESC LIMIT ?1)",
            params![sse_replay_limit.max(1) as i64],
        )?;
        tx.commit()?;

        Ok(Some(MetaOutcome {
            applied: true,
            sse_event: Some(PersistedSseEvent {
                seq: server_seq,
                event_type: "book_meta".to_string(),
                book_id: meta.book_id.clone(),
                origin_device_id: origin_device_id.to_string(),
                payload_json,
            }),
            meta,
        }))
    }
}

fn load_meta(
    conn: &rusqlite::Connection,
    book_id: &str,
) -> Result<Option<BookMetaRecord>, rusqlite::Error> {
    conn.query_row(
        "SELECT book_id, display_title, cover_version, cover_source, hidden_continue_at_ms,
                server_seq, updated_at_ms
         FROM book_meta WHERE book_id = ?1",
        params![book_id],
        |row| {
            Ok(BookMetaRecord {
                book_id: row.get(0)?,
                display_title: row.get(1)?,
                cover_version: row.get::<_, i64>(2)?.max(0) as u64,
                cover_source: row.get(3)?,
                hidden_continue_at_ms: row.get::<_, Option<i64>>(4)?.map(|v| v.max(0) as u64),
                server_seq: row.get::<_, i64>(5)?.max(0) as u64,
                updated_at_ms: row.get::<_, i64>(6)?.max(0) as u64,
            })
        },
    )
    .optional()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: [u8; 12] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

    fn db_with_book() -> (tempfile::TempDir, Database, BookId) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(dir.path()).expect("open");
        let id = BookId::parse("0123456789abcdef0123456789abcdef-1024").expect("id");
        let conn = db.lock_conn();
        conn.execute(
            "INSERT INTO books (book_id, partial_md5, size_bytes, title, format, mtime_ms, rel_path, updated_at_ms)
             VALUES (?1, 'x', 1024, 'Livro', 'epub', 1, 'livro.epub', 1)",
            params![id.as_str()],
        )
        .expect("insert book");
        drop(conn);
        (dir, db, id)
    }

    #[test]
    fn titles_are_trimmed_collapsed_and_limited() {
        assert_eq!(normalize_display_title(None), Ok(None));
        assert_eq!(normalize_display_title(Some("   \t\n ")), Ok(None));
        assert_eq!(
            normalize_display_title(Some("  Swamp \n Thing\t#1 ")),
            Ok(Some("Swamp Thing #1".to_string()))
        );
        let max = "a".repeat(DISPLAY_TITLE_MAX_CHARS);
        assert_eq!(normalize_display_title(Some(&max)), Ok(Some(max.clone())));
        let over = "a".repeat(DISPLAY_TITLE_MAX_CHARS + 1);
        assert_eq!(
            normalize_display_title(Some(&over)),
            Err(MetaValidationError::TitleTooLong)
        );
        // 200 caracteres multibyte ainda cabem (o limite e em caracteres, nao bytes).
        let accents = "é".repeat(DISPLAY_TITLE_MAX_CHARS);
        assert!(normalize_display_title(Some(&accents)).is_ok());
    }

    #[test]
    fn cover_type_is_detected_by_bytes() {
        assert_eq!(
            sniff_cover_mime(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Ok("image/jpeg")
        );
        assert_eq!(sniff_cover_mime(&PNG), Ok("image/png"));
        assert_eq!(
            sniff_cover_mime(b"RIFF\x00\x00\x00\x00WEBPVP8 "),
            Ok("image/webp")
        );
        assert_eq!(sniff_cover_mime(b""), Err(MetaValidationError::CoverEmpty));
        assert_eq!(
            sniff_cover_mime(b"<svg xmlns='http://www.w3.org/2000/svg'/>"),
            Err(MetaValidationError::CoverUnsupportedType)
        );
        assert_eq!(
            sniff_cover_mime(b"RIFF\x00\x00\x00\x00WAVEfmt "),
            Err(MetaValidationError::CoverUnsupportedType)
        );
        let mut big = PNG.to_vec();
        big.resize(COVER_MAX_BYTES, 0);
        assert_eq!(sniff_cover_mime(&big), Ok("image/png"));
        big.push(0);
        assert_eq!(
            sniff_cover_mime(&big),
            Err(MetaValidationError::CoverTooLarge)
        );
    }

    #[test]
    fn display_title_roundtrip_and_noop_does_not_emit() {
        let (_d, db, id) = db_with_book();
        let out = db
            .set_display_title(&id, Some("Meu nome".into()), "dev-a", 10, 100)
            .unwrap()
            .unwrap();
        assert!(out.applied);
        assert_eq!(out.meta.display_title.as_deref(), Some("Meu nome"));
        let ev = out.sse_event.expect("evento");
        assert_eq!(ev.event_type, "book_meta");
        assert_eq!(ev.origin_device_id, "dev-a");

        let same = db
            .set_display_title(&id, Some("Meu nome".into()), "dev-a", 11, 100)
            .unwrap()
            .unwrap();
        assert!(!same.applied && same.sse_event.is_none());

        let cleared = db
            .set_display_title(&id, None, "dev-b", 12, 100)
            .unwrap()
            .unwrap();
        assert!(cleared.applied);
        assert_eq!(db.get_book_meta(&id).unwrap().unwrap().display_title, None);
    }

    #[test]
    fn unknown_book_returns_none() {
        let (_d, db, _id) = db_with_book();
        let other = BookId::parse("ffffffffffffffffffffffffffffffff-1").unwrap();
        assert!(db
            .set_display_title(&other, None, "d", 1, 10)
            .unwrap()
            .is_none());
        assert!(db
            .set_continue_hidden(&other, true, "d", 1, 10)
            .unwrap()
            .is_none());
        assert!(db.get_book_meta(&other).unwrap().is_none());
        assert!(db.get_cover(&other).unwrap().is_none());
    }

    #[test]
    fn hiding_records_time_and_restoring_clears_it() {
        let (_d, db, id) = db_with_book();
        let hid = db
            .set_continue_hidden(&id, true, "d", 5_000, 10)
            .unwrap()
            .unwrap();
        assert_eq!(hid.meta.hidden_continue_at_ms, Some(5_000));
        // Esconder de novo depois de ler: o instante e renovado.
        let again = db
            .set_continue_hidden(&id, true, "d", 9_000, 10)
            .unwrap()
            .unwrap();
        assert_eq!(again.meta.hidden_continue_at_ms, Some(9_000));
        let shown = db
            .set_continue_hidden(&id, false, "d", 9_500, 10)
            .unwrap()
            .unwrap();
        assert_eq!(shown.meta.hidden_continue_at_ms, None);
    }

    #[test]
    fn cover_version_follows_server_seq_and_embedded_never_overrides_user_choice() {
        let (_d, db, id) = db_with_book();
        assert_eq!(
            db.list_book_ids_without_cover().unwrap(),
            vec![id.as_str().to_string()]
        );

        let emb = db
            .put_cover(
                &id,
                NewCover {
                    source: CoverSource::Embedded,
                    mime: "image/png",
                    data: &PNG,
                },
                "d",
                1,
                10,
            )
            .unwrap()
            .unwrap();
        assert!(emb.applied);
        assert!(emb.meta.cover_version > 0);
        assert_eq!(emb.meta.cover_version, emb.meta.server_seq);
        assert!(db.list_book_ids_without_cover().unwrap().is_empty());

        let custom = db
            .put_cover(
                &id,
                NewCover {
                    source: CoverSource::Custom,
                    mime: "image/jpeg",
                    data: &[0xFF, 0xD8, 0xFF, 1],
                },
                "d",
                2,
                10,
            )
            .unwrap()
            .unwrap();
        assert!(custom.applied);
        assert!(custom.meta.cover_version > emb.meta.cover_version);
        assert_eq!(custom.meta.cover_source.as_deref(), Some("custom"));

        let ignored = db
            .put_cover(
                &id,
                NewCover {
                    source: CoverSource::Embedded,
                    mime: "image/png",
                    data: &PNG,
                },
                "d",
                3,
                10,
            )
            .unwrap()
            .unwrap();
        assert!(!ignored.applied && ignored.sse_event.is_none());
        let stored = db.get_cover(&id).unwrap().unwrap();
        assert_eq!(stored.mime, "image/jpeg");
        assert_eq!(stored.version, custom.meta.cover_version);

        // `Web` troca `Custom` (as duas sao escolhas do usuario; vale a mais recente).
        let web = db
            .put_cover(
                &id,
                NewCover {
                    source: CoverSource::Web,
                    mime: "image/png",
                    data: &PNG,
                },
                "d",
                4,
                10,
            )
            .unwrap()
            .unwrap();
        assert!(web.applied);
        assert_eq!(db.get_cover(&id).unwrap().unwrap().mime, "image/png");
    }

    #[test]
    fn delete_cover_resets_and_allows_embedded_again() {
        let (_d, db, id) = db_with_book();
        assert!(!db.delete_cover(&id, "d", 1, 10).unwrap().unwrap().applied);
        db.put_cover(
            &id,
            NewCover {
                source: CoverSource::Custom,
                mime: "image/png",
                data: &PNG,
            },
            "d",
            2,
            10,
        )
        .unwrap();
        let del = db.delete_cover(&id, "d", 3, 10).unwrap().unwrap();
        assert!(del.applied);
        assert_eq!(
            (del.meta.cover_version, del.meta.cover_source.clone()),
            (0, None)
        );
        assert!(db.get_cover(&id).unwrap().is_none());
        let emb = db
            .put_cover(
                &id,
                NewCover {
                    source: CoverSource::Embedded,
                    mime: "image/png",
                    data: &PNG,
                },
                "d",
                4,
                10,
            )
            .unwrap()
            .unwrap();
        assert!(emb.applied);
    }

    #[test]
    fn sse_replay_is_trimmed() {
        let (_d, db, id) = db_with_book();
        for i in 0..5 {
            db.set_display_title(&id, Some(format!("t{i}")), "d", i, 3)
                .unwrap();
        }
        assert_eq!(db.list_sse_events_after(0).unwrap().len(), 3);
    }
}

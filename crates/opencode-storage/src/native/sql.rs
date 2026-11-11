//! Statements shared by the boot read, the writer and the importer.

use std::collections::BTreeMap;
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{OutboxAttachment, OutboxRow, WindowFrame};

/// Pages read from the file since the connection opened.
#[allow(unsafe_code)]
pub fn cache_misses(conn: &Connection) -> i64 {
    let (mut current, mut high) = (0, 0);
    // SAFETY: `handle` is this live connection's `sqlite3*`, used on its own thread; the two
    // out-pointers are valid `c_int`s for the call.
    let rc = unsafe { rusqlite::ffi::sqlite3_db_status(conn.handle(), rusqlite::ffi::SQLITE_DBSTATUS_CACHE_MISS, &mut current, &mut high, 0) };
    if rc == rusqlite::ffi::SQLITE_OK { i64::from(current) } else { -1 }
}

/// Keeps `-wal` and `-shm` on disk when the last connection closes, so no launch deletes and
/// creates them again (each a new-file scan for antivirus on Windows).
#[allow(unsafe_code)]
pub fn persist_wal(conn: &Connection) {
    let mut on: std::ffi::c_int = 1;
    // SAFETY: `handle` is this live connection's `sqlite3*`; "main" is NUL-terminated; the
    // PERSIST_WAL op reads and writes one `int` through the pointer.
    unsafe {
        rusqlite::ffi::sqlite3_file_control(conn.handle(), c"main".as_ptr(), rusqlite::ffi::SQLITE_FCNTL_PERSIST_WAL, (&raw mut on).cast());
    }
}

/// A stored document's text with its chunks put back. Text without chunk refs comes back
/// byte for byte.
pub fn read_document(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    let Some(stored) = conn.prepare_cached("SELECT value FROM document WHERE key = ?1")?.query_row([key], |r| r.get::<_, String>(0)).optional()? else { return Ok(None) };
    Ok(Some(decode_text(conn, stored)))
}

pub fn decode_text(conn: &Connection, stored: String) -> String {
    if !stored.contains("\"kind\":\"text\"") {
        return stored;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&stored) else { return stored };
    let mut chunk = |id: &str| read_blob(conn, id).ok().flatten().and_then(|b| String::from_utf8(b).ok());
    crate::chunk::decode(&value, &mut chunk).to_string()
}

/// `text` as stored: JSON whose long strings became chunks (returned for `blob`), or the text
/// as it is.
pub fn encode_text(text: &str) -> (String, Vec<(String, Vec<u8>)>) {
    let mut chunks = Vec::new();
    if text.len() < crate::chunk::TEXT_THRESHOLD || !(text.starts_with('{') || text.starts_with('[')) {
        return (text.to_owned(), chunks);
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else { return (text.to_owned(), chunks) };
    let encoded = crate::chunk::encode(&value, &mut chunks);
    if chunks.is_empty() {
        return (text.to_owned(), chunks);
    }
    (encoded.to_string(), chunks)
}

pub fn read_blob(conn: &Connection, id: &str) -> rusqlite::Result<Option<Vec<u8>>> {
    conn.prepare_cached("SELECT data FROM blob WHERE id = ?1")?.query_row([id], |r| r.get(0)).optional()
}

/// Stores a blob once; an existing one keeps its bytes. True when it was new.
pub fn insert_blob(conn: &Connection, id: &str, data: &[u8], now_ms: i64) -> rusqlite::Result<bool> {
    Ok(conn.prepare_cached("INSERT INTO blob (id, data, touched_at) VALUES (?1, ?2, ?3) ON CONFLICT (id) DO NOTHING")?.execute(params![id, data, now_ms])? > 0)
}

pub fn touch_blob(conn: &Connection, id: &str, now_ms: i64) -> rusqlite::Result<()> {
    conn.prepare_cached("UPDATE blob SET touched_at = ?2 WHERE id = ?1")?.execute(params![id, now_ms]).map(drop)
}

pub fn put_state(conn: &Connection, name: &str, key: &str, value: Option<&str>, now_ms: i64) -> rusqlite::Result<()> {
    match value {
        Some(v) => conn
            .prepare_cached("INSERT INTO state (name, key, value, updated_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT (name, key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at")?
            .execute(params![name, key, v, now_ms])
            .map(drop),
        None => conn.prepare_cached("DELETE FROM state WHERE name = ?1 AND key = ?2")?.execute(params![name, key]).map(drop),
    }
}

pub fn read_namespace(conn: &Connection, name: &str) -> rusqlite::Result<BTreeMap<String, Arc<str>>> {
    let mut stmt = conn.prepare_cached("SELECT key, value FROM state WHERE name = ?1")?;
    let rows = stmt.query_map([name], |r| Ok((r.get::<_, String>(0)?, Arc::<str>::from(r.get::<_, String>(1)?))))?;
    rows.collect()
}

pub fn put_document(conn: &Connection, key: &str, value: Option<&str>) -> rusqlite::Result<()> {
    match value {
        Some(v) => conn.prepare_cached("INSERT INTO document (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value")?.execute(params![key, v]).map(drop),
        None => conn.prepare_cached("DELETE FROM document WHERE key = ?1")?.execute([key]).map(drop),
    }
}

pub fn put_window(conn: &Connection, id: &str, frame: Option<&WindowFrame>, now_ms: i64) -> rusqlite::Result<()> {
    let Some(f) = frame else {
        return conn.prepare_cached("DELETE FROM window_state WHERE id = ?1")?.execute([id]).map(drop);
    };
    let d = f.display;
    conn.prepare_cached(
        "INSERT INTO window_state (id, x, y, width, height, maximized, full_screen, display_x, display_y, display_w, display_h, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) ON CONFLICT (id) DO UPDATE SET x = excluded.x, y = excluded.y, width = excluded.width, \
         height = excluded.height, maximized = excluded.maximized, full_screen = excluded.full_screen, display_x = excluded.display_x, \
         display_y = excluded.display_y, display_w = excluded.display_w, display_h = excluded.display_h, updated_at = excluded.updated_at",
    )?
    .execute(params![id, f.x, f.y, f.width, f.height, f.maximized, f.full_screen, d.map(|d| d[0]), d.map(|d| d[1]), d.map(|d| d[2]), d.map(|d| d[3]), now_ms])
    .map(drop)
}

pub fn put_plugin(conn: &Connection, id: &str, enabled: Option<bool>) -> rusqlite::Result<()> {
    match enabled {
        Some(on) => conn.prepare_cached("INSERT INTO plugin (id, enabled) VALUES (?1, ?2) ON CONFLICT (id) DO UPDATE SET enabled = excluded.enabled")?.execute(params![id, on]).map(drop),
        None => conn.prepare_cached("DELETE FROM plugin WHERE id = ?1")?.execute([id]).map(drop),
    }
}

pub fn put_trust(conn: &Connection, id: &str, sha256: Option<&str>, now_ms: i64) -> rusqlite::Result<()> {
    match sha256 {
        Some(sha) => conn
            .prepare_cached("INSERT INTO plugin_trust (id, sha256, trusted_at) VALUES (?1, ?2, ?3) ON CONFLICT (id) DO UPDATE SET sha256 = excluded.sha256, trusted_at = excluded.trusted_at")?
            .execute(params![id, sha, now_ms])
            .map(drop),
        None => conn.prepare_cached("DELETE FROM plugin_trust WHERE id = ?1")?.execute([id]).map(drop),
    }
}

const OUTBOX_COLUMNS: &str = "server, id, seq, session, state, operation_json, draft_json, prompt_json, attachment_blobs_json, paths_json, inbox, why, after_check, restored, durable, legacy_json";

/// `upsert`: replace an existing row (a state change); otherwise keep it (the importer).
pub fn put_outbox(conn: &Connection, row: &OutboxRow, now_ms: i64, upsert: bool) -> rusqlite::Result<bool> {
    let conflict = if upsert {
        "DO UPDATE SET seq = excluded.seq, session = excluded.session, state = excluded.state, operation_json = excluded.operation_json, \
         draft_json = excluded.draft_json, prompt_json = excluded.prompt_json, attachment_blobs_json = excluded.attachment_blobs_json, \
         paths_json = excluded.paths_json, inbox = excluded.inbox, why = excluded.why, after_check = excluded.after_check, \
         restored = excluded.restored, durable = excluded.durable, legacy_json = excluded.legacy_json, updated_at = excluded.updated_at"
    } else {
        "DO NOTHING"
    };
    let sql = format!("INSERT INTO outbox ({OUTBOX_COLUMNS}, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17) ON CONFLICT (server, id) {conflict}");
    let attachments = serde_json::to_string(&row.attachments).unwrap_or_else(|_| "[]".into());
    let n = conn.prepare_cached(&sql)?.execute(params![
        row.server,
        row.id,
        row.seq,
        row.session,
        row.state,
        row.operation_json,
        row.draft_json,
        row.prompt_json,
        attachments,
        row.paths_json,
        row.inbox,
        row.why,
        row.after_check,
        row.restored,
        row.durable,
        row.legacy_json,
        now_ms
    ])?;
    Ok(n > 0)
}

pub fn delete_outbox(conn: &Connection, server: &str, id: &str) -> rusqlite::Result<()> {
    conn.prepare_cached("DELETE FROM outbox WHERE server = ?1 AND id = ?2")?.execute([server, id]).map(drop)
}

fn outbox_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<OutboxRow> {
    let attachments: String = r.get(8)?;
    Ok(OutboxRow {
        server: r.get(0)?,
        id: r.get(1)?,
        seq: r.get(2)?,
        session: r.get(3)?,
        state: r.get(4)?,
        operation_json: r.get(5)?,
        draft_json: r.get(6)?,
        prompt_json: r.get(7)?,
        attachments: serde_json::from_str::<Vec<OutboxAttachment>>(&attachments).unwrap_or_default(),
        paths_json: r.get(9)?,
        inbox: r.get(10)?,
        why: r.get(11)?,
        after_check: r.get(12)?,
        restored: r.get(13)?,
        durable: r.get(14)?,
        legacy_json: r.get(15)?,
    })
}

pub fn read_outbox_row(conn: &Connection, server: &str, id: &str) -> rusqlite::Result<Option<OutboxRow>> {
    conn.prepare_cached(&format!("SELECT {OUTBOX_COLUMNS} FROM outbox WHERE server = ?1 AND id = ?2"))?.query_row([server, id], outbox_from).optional()
}

pub fn read_outbox(conn: &Connection) -> rusqlite::Result<BTreeMap<String, Vec<OutboxRow>>> {
    let mut stmt = conn.prepare_cached(&format!("SELECT {OUTBOX_COLUMNS} FROM outbox ORDER BY server, seq"))?;
    let rows = stmt.query_map([], outbox_from)?;
    let mut out: BTreeMap<String, Vec<OutboxRow>> = BTreeMap::new();
    for row in rows {
        let row = row?;
        out.entry(row.server.clone()).or_default().push(row);
    }
    Ok(out)
}

/// Every blob id something stored references: document image parts and text chunk lists
/// (Desktop's `drafts.ts:169-179`), outbox attachments and the `blob:` files of outbox prompts.
const REFERENCED: &str = "
    SELECT json_extract(node.value, '$.id') FROM document, json_tree(document.value) AS node
    WHERE json_valid(document.value) AND node.key = 'blob' AND node.type = 'object' AND json_type(node.value, '$.id') = 'text'
    UNION SELECT chunk.value FROM document, json_tree(document.value) AS node, json_each(node.value, '$.ids') AS chunk
    WHERE json_valid(document.value) AND node.key = 'blob' AND node.type = 'object' AND json_type(node.value, '$.ids') = 'array'
    UNION SELECT json_extract(item.value, '$.blob') FROM outbox, json_each(outbox.attachment_blobs_json) AS item
    UNION SELECT substr(json_extract(f.value, '$.uri'), 6) FROM outbox, json_each(outbox.prompt_json, '$.files') AS f
    WHERE json_valid(outbox.prompt_json) AND json_extract(f.value, '$.uri') LIKE 'blob:%'";

/// Drops blobs nothing references and nothing touched since `before_ms`; returns the oldest
/// `touched_at` among unreferenced blobs it kept (`collectBlobs`, `drafts.ts:183-195`).
pub fn collect_blobs(conn: &Connection, before_ms: i64) -> rusqlite::Result<Option<i64>> {
    let tx = conn.unchecked_transaction()?;
    if !tx.query_row("SELECT EXISTS (SELECT 1 FROM blob)", [], |r| r.get::<_, bool>(0))? {
        return Ok(None);
    }
    // A NULL among the ids would make every `NOT IN` unknown, and nothing would go.
    tx.execute(&format!("WITH refs(id) AS ({REFERENCED}) DELETE FROM blob WHERE touched_at < ?1 AND id NOT IN (SELECT id FROM refs WHERE id IS NOT NULL)"), [before_ms])?;
    let oldest = tx.query_row(&format!("WITH refs(id) AS ({REFERENCED}) SELECT MIN(touched_at) FROM blob WHERE id NOT IN (SELECT id FROM refs WHERE id IS NOT NULL)"), [], |r| r.get::<_, Option<i64>>(0))?;
    tx.commit()?;
    Ok(oldest)
}

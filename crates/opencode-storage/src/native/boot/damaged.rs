//! A file SQLite can't read (a power cut or a disk error damaged it): told apart from one another
//! process holds, moved aside once, and what the windows showed kept in its replacement.

use rusqlite::Connection;

use super::super::sql;
use crate::boot_packet::BootPacket;
use crate::cache::Cache;

/// Whether SQLite finds the file damaged: it can't be read as a database, or a quick check of
/// its pages fails. Only asked after the open failed, so its cost (a read of the file) is paid
/// on that path alone. A file another process holds answers busy, which isn't damage.
pub(super) fn damaged(path: &std::path::Path) -> bool {
    let Ok(c) = Connection::open(path) else { return false };
    let corrupt = |e: &rusqlite::Error| matches!(e, rusqlite::Error::SqliteFailure(f, _) if matches!(f.code, rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase));
    match c.query_row("PRAGMA quick_check(1)", [], |r| r.get::<_, String>(0)) {
        Ok(v) => v != "ok",
        Err(e) => corrupt(&e),
    }
}

/// Writes what a boot record shows (its windows' rows, frames and shown drafts) into a new file,
/// in one transaction.
pub(super) fn adopt_shown(conn: &Connection, p: &BootPacket, now_ms: i64) -> rusqlite::Result<()> {
    let mut shown = Cache { complete: true, ..Cache::default() };
    p.fill_cache(&mut shown);
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    for (name, ns) in &shown.state {
        let rows: Vec<(&String, &str)> = match ns {
            crate::cache::Namespace::Loaded { rows, .. } => rows.iter().map(|(k, v)| (k, &**v)).collect(),
            crate::cache::Namespace::Unloaded { written, .. } => written.iter().filter_map(|(k, v)| Some((k, &**v.as_ref()?))).collect(),
        };
        for (key, value) in rows {
            sql::put_state(&tx, name, key, Some(value), now_ms)?;
        }
    }
    for (key, doc) in &shown.docs {
        if let crate::cache::Doc::Present(text) = doc {
            let (stored, chunks) = sql::encode_text(text);
            for (id, data) in &chunks {
                if !sql::insert_blob(&tx, id, data, now_ms)? {
                    sql::touch_blob(&tx, id, now_ms)?;
                }
            }
            sql::put_document(&tx, key, Some(&stored))?;
        }
    }
    for (id, frame) in &shown.windows {
        sql::put_window(&tx, id, Some(frame), now_ms)?;
    }
    tx.commit()
}

/// Moves the damaged file and its WAL to `opencode.damaged-<when>.sqlite` (`-wal`) beside it.
pub(super) fn keep_damaged(path: &std::path::Path, now_ms: i64) -> std::io::Result<std::path::PathBuf> {
    let dir = path.parent().ok_or(std::io::ErrorKind::NotFound)?;
    let kept = (0..100).map(|n| dir.join(if n == 0 { format!("opencode.damaged-{now_ms}.sqlite") } else { format!("opencode.damaged-{now_ms}-{n}.sqlite") })).find(|p| !p.exists()).ok_or(std::io::ErrorKind::AlreadyExists)?;
    std::fs::rename(path, &kept)?;
    let side = |p: &std::path::Path, s: &str| std::path::PathBuf::from(format!("{}{s}", p.display()));
    if side(path, "-wal").exists() {
        std::fs::rename(side(path, "-wal"), side(&kept, "-wal")).ok();
    }
    std::fs::remove_file(side(path, "-shm")).ok();
    Ok(kept)
}

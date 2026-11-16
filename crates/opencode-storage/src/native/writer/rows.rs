//! What the writer's commits apply, and how it reports on itself.

use std::collections::HashMap;
use std::time::Instant;

use rune::machine::Attempt;
use rusqlite::Connection;

use super::Batch;
use crate::native::sql;
use crate::Cmd;
use crate::machine::draft_gc::BLOB_GRACE_MS;
use crate::machine::write_behind::WriteBehindEvent;

/// One batch's rows. Documents' long strings become chunks here (never on the caller's thread);
/// a chunk this run touched within half the grace period isn't touched again.
pub(super) fn apply(tx: &Connection, batch: &Batch, now: i64, touched: &HashMap<String, i64>, fresh: &mut Vec<String>) -> rusqlite::Result<()> {
    match batch {
        Batch::State(_, rows) => {
            for ((name, key), value) in rows {
                sql::put_state(tx, name, key, value.as_deref(), now)?;
            }
        }
        Batch::Windows(_, rows) => {
            for (id, frame) in rows {
                sql::put_window(tx, id, frame.as_ref(), now)?;
            }
        }
        Batch::Docs(_, rows) => {
            for (key, (value, rev)) in rows {
                crate::native::search::index::draft(tx,key,now)?;
                if *rev > 0 {
                    crate::native::journal::put_rev(tx, key, *rev)?;
                }
                let Some(text) = value else {
                    sql::put_document(tx, key, None)?;
                    continue;
                };
                let (stored, chunks) = sql::encode_text(text);
                for (id, data) in &chunks {
                    if touched.get(id).is_some_and(|t| now - t < BLOB_GRACE_MS / 2) {
                        continue;
                    }
                    if !sql::insert_blob(tx, id, data, now)? {
                        sql::touch_blob(tx, id, now)?;
                    }
                    fresh.push(id.clone());
                }
                sql::put_document(tx, key, Some(&stored))?;
            }
        }
        Batch::Previews(_, rows) => {
            for ((server, session), value) in rows {
                match value {
                    Some((at, data)) => tx
                        .prepare_cached("INSERT INTO session_preview (server, session, updated_at, data) VALUES (?1, ?2, ?3, ?4) ON CONFLICT (server, session) DO UPDATE SET updated_at = excluded.updated_at, data = excluded.data")?
                        .execute(rusqlite::params![server, session, at, &**data])?,
                    None => tx.prepare_cached("DELETE FROM session_preview WHERE server = ?1 AND session = ?2")?.execute([server, session])?,
                };
            }
        }
        Batch::Caches(_, rows) => {
            for (key, value) in rows {
                if key.starts_with("rune.copy\n") { crate::native::search::index::mark(tx,key,now)?; }
                match value {
                    Some((version, data)) => tx
                        .prepare_cached("INSERT INTO cache (key, version, data, touched_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT (key) DO UPDATE SET version = excluded.version, data = excluded.data, touched_at = excluded.touched_at")?
                        .execute(rusqlite::params![key, version, &**data, now])?,
                    None => tx.prepare_cached("DELETE FROM cache WHERE key = ?1")?.execute([key])?,
                };
            }
        }
    }
    Ok(())
}

/// How a batch's write went, as its machine's event.
pub(super) fn settled<K, V>(attempt: Attempt, error: &Option<String>) -> WriteBehindEvent<K, V> {
    match error {
        None => WriteBehindEvent::WriteSucceeded { attempt },
        Some(e) => WriteBehindEvent::WriteFailed { attempt, error: e.clone() },
    }
}

/// A storage-thread step that held the writer (and so every write and a quit behind it) for long:
/// said on stderr, with what it was.
pub(super) fn slow(what: &str, since: Instant) {
    let us = since.elapsed().as_micros();
    if us >= 250_000 {
        eprintln!("opencode-storage: {what} took {us} us on the storage thread");
    }
}

pub(super) fn cmd_name(cmd: &Cmd) -> &'static str {
    match cmd {
        #[cfg(windows)]
        Cmd::RecoveryPick{..}=>"a recovered draft choice",
        Cmd::State { .. } => "a state row",
        Cmd::ClearState { .. } => "clearing a namespace",
        Cmd::Doc { .. } => "a document",
        Cmd::Blob { .. } => "a blob",
        Cmd::Window { .. } => "a window frame",
        Cmd::PluginRecord { .. } => "a plugin record",
        Cmd::Outbox { .. } => "an outbox change",
        Cmd::LoadState { .. } => "loading namespaces",
        Cmd::LoadDocs { .. } => "loading documents",
        Cmd::LoadOutbox { .. } => "loading the outbox",
        Cmd::GetBlob { .. } => "reading a blob",
        Cmd::Preview { .. } => "a session preview",
        Cmd::GetPreview { .. } => "reading a session preview",
        Cmd::Cache { .. } => "a cache row",
        Cmd::GetCache { .. } => "reading a cache row",
        Cmd::CommitSoon => "a kill-safe commit request",
    }
}

/// A write's outcome for `write_trouble`: SQLite's own `SQLITE_FULL`, or the OS saying the disk
/// has no space (Windows' ERROR_DISK_FULL / ERROR_HANDLE_DISK_FULL, ENOSPC) under an I/O error.
pub(super) fn note<T>(result: &rusqlite::Result<T>) {
    crate::note_write(result.as_ref().err().map(|e| {
        let why = e.to_string();
        let lower = why.to_lowercase();
        let full = matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::DiskFull)
            || lower.contains("disk is full")
            || lower.contains("not enough space")
            || lower.contains("no space left")
            || lower.contains("os error 112")
            || lower.contains("os error 39");
        (why, full)
    }));
}

//! The outbox's statements, run on the storage thread (`native::writer`).

use std::sync::Arc;

use rusqlite::{Connection, params};

use super::{OutboxEntry, OutboxOp, OutboxRead, Written};
use crate::native::sql;

/// An outbox request with its builder run: what to commit (with its blobs and the documents it
/// clears), or what to read.
pub(crate) enum Prepared {
    Write(Written, Vec<(String, Arc<[u8]>)>, Vec<(String, u64)>),
    Read { server: String, adopt: Vec<String>, done: OutboxRead },
}

/// Runs a put's builder here, off the caller's thread.
pub(crate) fn prepare(op: OutboxOp) -> Prepared {
    match op {
        OutboxOp::Put(build) => {
            rune::marks::mark("outbox.taken");
            let put = build();
            rune::marks::mark("outbox.built");
            Prepared::Write(Written::Put(put.row), put.blobs, put.clear_documents)
        }
        OutboxOp::Update { server, id, update } => Prepared::Write(Written::Update { server, id, update }, Vec::new(), Vec::new()),
        OutboxOp::Delete { server, id } => Prepared::Write(Written::Delete { server, id }, Vec::new(), Vec::new()),
        OutboxOp::Read { server, adopt, done } => Prepared::Read { server, adopt, done },
    }
}

/// Commits a write inside the caller's transaction. A blob already stored (a draft held the
/// same image) isn't written again; an update of a row that isn't there is an error.
pub(crate) fn write(c: &Connection, w: &Written, blobs: &[(String, Arc<[u8]>)], clear: &[(String, u64)], now_ms: i64) -> rusqlite::Result<()> {
    match w {
        Written::Put(row) => {
            crate::native::search::index::outbox(c, &row.server, &row.id, now_ms)?;
            rune::marks::mark("outbox.sql");
            for (key, rev) in clear {
                sql::put_document(c, key, None)?;
                crate::native::search::index::draft(c, key, now_ms)?;
                if *rev > 0 {
                    crate::native::journal::put_rev(c, key, *rev)?;
                }
            }
            for (id, data) in blobs {
                if !blob_stored(c, id)? {
                    sql::insert_blob(c, id, data, now_ms)?;
                }
            }
            let put = sql::put_outbox(c, row, now_ms, true).map(drop);
            rune::marks::mark("outbox.sql.done");
            put
        }
        Written::Update { server, id, update } => {
            let n = c
                .prepare_cached("UPDATE outbox SET state = ?3, why = ?4, after_check = ?5, restored = ?6, operation_json = ?7, updated_at = ?8 WHERE server = ?1 AND id = ?2")?
                .execute(params![server, id, update.state, update.why, update.after_check, update.restored, update.operation_json, now_ms])?;
            if n == 0 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            Ok(())
        }
        Written::Delete { server, id } => {
            crate::native::search::index::outbox(c,server,id,now_ms)?;
            sql::delete_outbox(c, server, id)
        },
    }
}

fn blob_stored(c: &Connection, id: &str) -> rusqlite::Result<bool> {
    c.prepare_cached("SELECT 1 FROM blob WHERE id = ?1")?.exists([id])
}

/// Whether rows wait under any of the `adopt` names.
pub(crate) fn has_adoptees(c: &Connection, adopt: &[String]) -> rusqlite::Result<bool> {
    for name in adopt {
        if c.prepare_cached("SELECT 1 FROM outbox WHERE server = ?1")?.exists([name])? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Rows kept under the `adopt` names become `server`'s; one it holds already keeps its own.
pub(crate) fn adopt(c: &Connection, server: &str, adopt: &[String], now_ms: i64) -> rusqlite::Result<()> {
    for name in adopt {
        let rows = sql::read_outbox_server(c, name)?;
        for mut row in rows {
            row.server = server.to_owned();
            sql::put_outbox(c, &row, now_ms, false)?;
        }
        c.prepare_cached("DELETE FROM outbox WHERE server = ?1")?.execute([name])?;
    }
    Ok(())
}

/// Server `server`'s rows, oldest first, with the bytes of every blob they name.
pub(crate) fn read(c: &Connection, server: &str) -> rusqlite::Result<Vec<OutboxEntry>> {
    let rows = sql::read_outbox_server(c, server)?;
    let mut out = Vec::with_capacity(rows.len());
    let mut loaded: Vec<(String, Arc<[u8]>)> = Vec::new();
    for row in rows {
        let mut ids: Vec<String> = row.attachments.iter().filter_map(|a| a.blob.clone()).collect();
        if let Some(p) = row.prompt_json.as_deref().and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok()) {
            ids.extend(p["files"].as_array().into_iter().flatten().filter_map(|f| f["uri"].as_str()?.strip_prefix("blob:").map(str::to_owned)));
        }
        let mut blobs = Vec::new();
        for id in ids {
            if blobs.iter().any(|(b, _): &(String, Arc<[u8]>)| *b == id) {
                continue;
            }
            let data = match loaded.iter().find(|(b, _)| *b == id) {
                Some((_, d)) => Some(d.clone()),
                None => sql::read_blob(c, &id)?.map(Arc::<[u8]>::from),
            };
            if let Some(d) = data {
                loaded.push((id.clone(), d.clone()));
                blobs.push((id, d));
            }
        }
        out.push(OutboxEntry { row, blobs });
    }
    Ok(out)
}

//! The first-run seed from Desktop's own database (`drafts.sqlite` in its `userData`), in
//! process, read-only, after the first frame and off the UI thread. It replaces the
//! `sqlite3.exe` child the app spawned before its first frame (`apps/desktop/src/tabs.rs:283-307`).
//!
//! Desktop's folders are never written: its database opens with `SQLITE_OPEN_READ_ONLY` (falling
//! back to `immutable=1` when it can't be read alongside a running Desktop), and its
//! `opencode.settings` and `window-state-<id>.json` are only read.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;

/// Desktop's built-in plugins whose stores mean the same in Rune.
const PLUGIN_STORES: [&str; 7] = ["plugin.wsl", "plugin.ssh", "plugin.pairing", "plugin.review", "plugin.file", "plugin.updater", "plugin.browser"];

/// Opens `db` read-only; never creates, writes, checkpoints or deletes anything beside it.
pub fn open_read_only(db: &Path) -> Option<Connection> {
    if !db.is_file() {
        return None;
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI;
    let uri = |extra: &str| format!("file:{}?mode=ro{extra}", db.to_string_lossy().replace('\\', "/").replace('?', "%3f").replace('#', "%23"));
    let usable = |c: Connection| {
        c.busy_timeout(std::time::Duration::from_millis(250)).ok();
        c.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0)).ok().map(|_| c)
    };
    Connection::open_with_flags(uri(""), flags).ok().and_then(usable).or_else(|| Connection::open_with_flags(uri("&immutable=1"), flags).ok().and_then(usable))
}

/// The tabs (`tabs`, `tabs.info`) of the window Desktop wrote last, as `apps/desktop/src/tabs.rs`
/// `from_beta_db` read them through `sqlite3.exe`: a drop-in for that call, in process. Pass the
/// result to `opencode_kit::servers::migration::window::from_beta`.
pub fn recent_tabs(db: &Path) -> Option<(Value, Value)> {
    let conn = open_read_only(db)?;
    let name: String = conn.query_row("SELECT name FROM state WHERE key = 'tabs' AND name LIKE 'opencode.window.%' ORDER BY updated_at DESC LIMIT 1", [], |r| r.get(0)).optional().ok()??;
    let get = |key: &str| -> Value { conn.query_row("SELECT value FROM state WHERE name = ?1 AND key = ?2", [&name, key], |r| r.get::<_, String>(0)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null) };
    Some((get("tabs"), get("tabs.info")))
}

/// The `userData` folder among `dirs` whose database holds the newest state.
pub fn pick(dirs: &[PathBuf]) -> Option<(PathBuf, Connection)> {
    dirs.iter()
        .filter_map(|d| {
            let conn = open_read_only(&d.join("drafts.sqlite"))?;
            let newest: i64 = conn.query_row("SELECT COALESCE(MAX(updated_at), 0) FROM state", [], |r| r.get(0)).ok()?;
            Some((newest, d.clone(), conn))
        })
        .max_by_key(|(newest, _, _)| *newest)
        .map(|(_, d, c)| (d, c))
}

/// Copies Desktop's compatible rows into `conn` (an existing row wins). Returns the `state`
/// rows inserted, and what it reports.
pub fn run(conn: &Connection, dirs: &[PathBuf], now_ms: i64) -> (Vec<(String, String, String)>, Vec<String>) {
    let Some((dir, official)) = pick(dirs) else { return (Vec::new(), vec!["no Desktop database to seed from".into()]) };
    let settings: Value = std::fs::read_to_string(dir.join("opencode.settings")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    let ids: Vec<String> = settings["windowIds"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect();
    let mut names: Vec<String> = vec!["opencode.global.dat".into()];
    names.extend(PLUGIN_STORES.iter().map(|s| (*s).to_owned()));
    names.extend(ids.iter().map(|id| crate::window_namespace(id)));
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for name in &names {
        let Ok(mut stmt) = official.prepare("SELECT key, value FROM state WHERE name = ?1") else { continue };
        let Ok(found) = stmt.query_map([name], |r| Ok((name.clone(), r.get::<_, String>(0)?, r.get::<_, String>(1)?))) else { continue };
        rows.extend(found.flatten());
    }
    for (k, v) in settings.as_object().into_iter().flatten() {
        rows.push(("opencode.settings".into(), k.clone(), v.to_string()));
    }
    let plugins: Vec<(String, bool)> = official.prepare("SELECT id, enabled FROM plugin").and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()).unwrap_or_default();
    let frames: Vec<(String, crate::WindowFrame)> = ids
        .iter()
        .filter_map(|id| {
            let v: Value = serde_json::from_str(&std::fs::read_to_string(dir.join(format!("window-state-{id}.json"))).ok()?).ok()?;
            Some((id.clone(), crate::WindowFrame::from_json(&v)?))
        })
        .collect();
    drop(official);
    let result = (|| -> rusqlite::Result<Vec<(String, String, String)>> {
        let tx = conn.unchecked_transaction()?;
        let mut inserted = Vec::new();
        for (name, key, value) in rows {
            if tx.execute("INSERT INTO state (name, key, value, updated_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING", (&name, &key, &value, now_ms))? > 0 {
                inserted.push((name, key, value));
            }
        }
        for (id, enabled) in plugins {
            tx.execute("INSERT INTO plugin (id, enabled) VALUES (?1, ?2) ON CONFLICT DO NOTHING", (id, enabled))?;
        }
        for (id, frame) in frames {
            if tx.query_row("SELECT 1 FROM window_state WHERE id = ?1", [&id], |_| Ok(())).optional()?.is_none() {
                super::sql::put_window(&tx, &id, Some(&frame), now_ms)?;
            }
        }
        tx.commit()?;
        Ok(inserted)
    })();
    match result {
        Ok(inserted) => {
            let report = format!("seeded {} rows from Desktop's database in {}", inserted.len(), dir.display());
            (inserted, vec![report])
        }
        Err(e) => (Vec::new(), vec![format!("seed from Desktop failed: {e}")]),
    }
}

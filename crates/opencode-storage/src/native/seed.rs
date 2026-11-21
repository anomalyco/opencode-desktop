//! The first-run seed from Desktop's own database (`drafts.sqlite` in its `userData`), in
//! process, read-only, before the first boot packet is built on a fresh profile.
//!
//! Desktop's folders are never written: its database opens with `SQLITE_OPEN_READ_ONLY` (falling
//! back to `immutable=1` when it can't be read alongside a running Desktop), and its
//! `opencode.settings` and `window-state-<id>.json` are only read.

use std::path::{Path, PathBuf};

use opencode_kit::servers::{Route, TabKey};
use opencode_servers::tabs::Tab;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Value, json};

use crate::WindowFrame;

/// Desktop's built-in plugins whose stores mean the same in Rune.
const PLUGIN_STORES: [&str; 7] = ["plugin.wsl", "plugin.ssh", "plugin.pairing", "plugin.review", "plugin.file", "plugin.updater", "plugin.browser"];

/// Maximum windows imported from an official Desktop profile.
const MAX_SEED_WINDOWS: usize = 32;

/// Opens `db` read-only; never creates, writes, checkpoints or deletes anything beside it.
pub fn open_read_only(db: &Path) -> Option<Connection> {
    if !db.is_file() {
        return None;
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI;
    let uri = |extra: &str| format!("file:{}?mode=ro{extra}", db.to_string_lossy().replace('\\', "/").replace('?', "%3f").replace('#', "%23"));
    let usable = |c: Connection| {
        super::sql::persist_wal(&c);
        c.busy_timeout(std::time::Duration::from_millis(250)).ok();
        c.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0)).ok().map(|_| c)
    };
    Connection::open_with_flags(uri(""), flags).ok().and_then(usable).or_else(|| Connection::open_with_flags(uri("&immutable=1"), flags).ok().and_then(usable))
}

/// The tabs (`tabs`, `tabs.info`) of the window Desktop wrote last, replacing `apps/desktop/src/shell/tabs/`'s
/// former `sqlite3.exe` lookup: a drop-in for that call, in process. Pass the
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

fn is_safe_window_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && !id.contains("..") && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Official Desktop persists `tabs` and `tabs.recent` (`{"key": ...}`) without `lastActiveUrl`;
/// synthesize `lastActiveUrl` when `tabs.recent` names an open tab in `tabs` so migrated windows
/// reopen on the active tab instead of falling back to Home.
fn derived_last_active_url(win_rows: &[(String, String)]) -> Option<String> {
    if win_rows.iter().any(|(k, _)| k == "lastActiveUrl") {
        return None;
    }
    let recent_raw = win_rows.iter().find(|(k, _)| k == "tabs.recent")?.1.as_str();
    let recent_key = serde_json::from_str::<Value>(recent_raw).ok()?.get("key")?.as_str().and_then(TabKey::parse)?;
    let tabs_raw = win_rows.iter().find(|(k, _)| k == "tabs")?.1.as_str();
    let tabs: Vec<Tab> = serde_json::from_str::<Value>(tabs_raw).ok()?.as_array()?.iter().filter_map(Tab::from_json).collect();
    if !tabs.iter().any(|t| t.key() == recent_key) {
        return None;
    }
    let href = match recent_key {
        TabKey::Session { server, session_id } => Route::Session { server, session_id }.href(),
        TabKey::Draft(id) => Route::Draft(id).href(),
    };
    Some(json!(href).to_string())
}

/// Copies Desktop's compatible rows into `conn` (an existing row wins). Returns the `state`
/// rows inserted, and what it reports.
pub fn run(conn: &Connection, dirs: &[PathBuf], now_ms: i64) -> (Vec<(String, String, String)>, Vec<String>) {
    let Some((dir, official)) = pick(dirs) else { return (Vec::new(), vec!["no Desktop database to seed from".into()]) };
    let settings: Value = std::fs::read_to_string(dir.join("opencode.settings")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);

    let mut rows: Vec<(String, String, String)> = Vec::new();
    // Official Desktop stores preferences (`settings.v3`) in `default.dat`, whereas Rune reads
    // `settings.v3` from `opencode.global.dat` (`features/settings/src/stored.rs`).
    if let Ok(Some(v3)) = official
        .query_row("SELECT value FROM state WHERE name = 'default.dat' AND key = 'settings.v3'", [], |r| r.get::<_, String>(0))
        .optional()
    {
        rows.push(("opencode.global.dat".into(), "settings.v3".into(), v3));
    }

    let mut names: Vec<String> = vec!["opencode.global.dat".into()];
    names.extend(PLUGIN_STORES.iter().map(|s| (*s).to_owned()));
    for name in &names {
        let Ok(mut stmt) = official.prepare("SELECT key, value FROM state WHERE name = ?1") else { continue };
        let Ok(found) = stmt.query_map([name], |r| Ok((name.clone(), r.get::<_, String>(0)?, r.get::<_, String>(1)?))) else { continue };
        rows.extend(found.flatten());
    }

    // Collect saved window ids from `opencode.settings` (falling back to the newest window namespace
    // in `drafts.sqlite` when `windowIds` is absent or empty).
    let mut window_ids: Vec<String> = Vec::new();
    for id in settings["windowIds"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if is_safe_window_id(id) && !window_ids.iter().any(|x| x == id) {
            window_ids.push(id.to_owned());
            if window_ids.len() >= MAX_SEED_WINDOWS {
                break;
            }
        }
    }
    if window_ids.is_empty()
        && let Ok(Some(name)) = official
            .query_row("SELECT name FROM state WHERE name LIKE 'opencode.window.%.dat' GROUP BY name ORDER BY MAX(updated_at) DESC LIMIT 1", [], |r| r.get::<_, String>(0))
            .optional()
        && let Some(id) = name.strip_prefix("opencode.window.").and_then(|s| s.strip_suffix(".dat"))
        && is_safe_window_id(id)
    {
        window_ids.push(id.to_owned());
    }

    for (k, v) in settings.as_object().into_iter().flatten() {
        if k == "windowIds" {
            continue;
        }
        rows.push(("opencode.settings".into(), k.clone(), v.to_string()));
    }
    if !window_ids.is_empty() {
        rows.push(("opencode.settings".into(), "windowIds".into(), json!(&window_ids).to_string()));
    }
    if let Some(z) = settings["zoomLevel"].as_f64().filter(|z| z.is_finite() && (0.25..=4.0).contains(z)) {
        rows.push(("opencode.global.dat".into(), "zoom".into(), json!(z).to_string()));
    }

    let mut frames: Vec<(String, WindowFrame)> = Vec::new();
    for wid in &window_ids {
        let ns = crate::window_namespace(wid);
        let mut win_rows: Vec<(String, String)> = Vec::new();
        if let Ok(mut stmt) = official.prepare("SELECT key, value FROM state WHERE name = ?1")
            && let Ok(found) = stmt.query_map([&ns], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        {
            win_rows.extend(found.flatten());
        }
        if let Some(url_json) = derived_last_active_url(&win_rows) {
            win_rows.push(("lastActiveUrl".into(), url_json));
        }
        for (key, value) in win_rows {
            rows.push((ns.clone(), key, value));
        }
        if let Some(frame) = std::fs::read_to_string(dir.join(format!("window-state-{wid}.json")))
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| WindowFrame::from_json(&v))
        {
            frames.push((wid.clone(), frame));
        }
    }

    let plugins: Vec<(String, bool)> = official.prepare("SELECT id, enabled FROM plugin").and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()).unwrap_or_default();
    drop(official);
    let result = (|| -> rusqlite::Result<Vec<(String, String, String)>> {
        let tx = conn.unchecked_transaction()?;
        let mut inserted = Vec::new();
        for (name, key, value) in rows {
            if tx.execute("INSERT INTO state (name, key, value, updated_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING", (&name, &key, &value, now_ms))? > 0 {
                inserted.push((name, key, value));
            }
        }
        for (id, frame) in frames {
            if tx.query_row("SELECT 1 FROM window_state WHERE id = ?1", [&id], |_| Ok(())).optional()?.is_none() {
                super::sql::put_window(&tx, &id, Some(&frame), now_ms)?;
            }
        }
        for (id, enabled) in plugins {
            tx.execute("INSERT INTO plugin (id, enabled) VALUES (?1, ?2) ON CONFLICT DO NOTHING", (id, enabled))?;
        }
        super::boot_record::rebuild(&tx)?;
        tx.commit()?;
        Ok(inserted)
    })();
    match result {
        Ok(inserted) => {
            let report = format!("seeded {} rows ({} windows) from Desktop's database in {}", inserted.len(), window_ids.len(), dir.display());
            (inserted, vec![report])
        }
        Err(e) => (Vec::new(), vec![format!("seed from Desktop failed: {e}")]),
    }
}

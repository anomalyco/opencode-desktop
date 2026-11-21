//! First-run migration from Official Desktop (`drafts.sqlite`, `opencode.settings`,
//! `window-state-<id>.json`), running before the first `BootPacket` on a fresh profile.
//!
//! Safety invariants:
//! 1. **Zero writes to Official Desktop's folder:** `drafts.sqlite` (+ `-wal` / `-shm`) is copied
//!    via plain shared file reads into a scratch directory inside OUR profile, recovered and
//!    verified with `PRAGMA integrity_check` there, and deleted on drop.
//! 2. **Explicit allow-list + credential scrubbing:** only approved settings, zoom, window frames,
//!    tabs, drafts + attachment blobs, prompt history, and secret-scrubbed server entries are
//!    imported (never `http.password`, tokens, `ssh.servers`, `bundledCliVersion`, or plugin
//!    stores such as `plugin.browser/sitePermissions` or `plugin.pairing`).
//! 3. **Atomic settle + bounded retries:** migrated data, rebuilt boot record, and the settled
//!    header flags (`groups | FLAG_SEED_SETTLED`) commit in one SQLite transaction. A crash before
//!    commit or a transient locked/torn read leaves migration unsettled and retries up to
//!    `MAX_SEED_ATTEMPTS` launches; once settled, migration never runs again.

mod drafts;
mod snapshot;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use opencode_kit::servers::{Route, TabKey};
use opencode_servers::tabs::Tab;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::{Map, Value, json};

use crate::WindowFrame;

use self::drafts::{
    MAX_SEED_TOTAL_BLOBS_BYTES, convert_composer_store, convert_prompt_history, load_session_docs,
    official_draft_doc_key, read_official_doc,
};
use self::snapshot::{
    MAX_SETTINGS_JSON_BYTES, MAX_WINDOW_STATE_BYTES, PickResult, open_snapshot, pick_dir,
    read_capped,
};
use super::{boot_record, schema, sql};

/// Maximum windows imported from an Official Desktop profile.
const MAX_SEED_WINDOWS: usize = 32;
/// Maximum value size for any single allow-listed `state` row (1 MiB).
const MAX_STATE_VALUE_BYTES: usize = 1024 * 1024;
/// Maximum total bytes across all copied `state` values (8 MiB).
const MAX_TOTAL_STATE_BYTES: usize = 8 * 1024 * 1024;
/// Bounded number of launches that retry after a transient lock / torn copy before settling.
pub(crate) const MAX_SEED_ATTEMPTS: u32 = 5;
/// Internal storage metadata namespace (ignored by fresh-DB checks).
pub(crate) const META_NS: &str = "rune.storage";
pub(crate) const ATTEMPTS_KEY: &str = "official_seed_attempts";

/// Allow-listed keys copied verbatim from Official's `opencode.global.dat` (`server` is scrubbed
/// separately via [`scrub_server_json`]).
const GLOBAL_ALLOW: &[&str] = &["language", "layout", "layout.scoped", "tips.queue", "settings.v3"];
/// Allow-listed keys copied from each `opencode.window.<id>.dat` namespace.
const WINDOW_ALLOW: &[&str] = &["tabs", "tabs.recent", "tabs.info", "tabs.closed", "tabs.panes", "lastActiveUrl"];
/// Allow-listed keys copied directly from `opencode.settings` (`windowIds`, `zoomLevel`, and
/// conditional `defaultServerUrl` are handled explicitly).
const SETTINGS_ALLOW: &[&str] = &["firstLaunchOnboardingComplete", "backgroundColor"];

/// Outcome of a first-run seed attempt.
pub(crate) enum SeedOutcome {
    /// No Official Desktop database existed in any candidate folder; flags settled atomically.
    NoSource,
    /// Migrated rows + boot record + settled flags committed in one atomic transaction.
    Seeded { rows: Vec<(String, String, String)>, report: Vec<String> },
    /// Official database existed but could not be read/verified/committed cleanly.
    /// When `exhausted` is false, `FLAG_SEED_SETTLED` remains unset so the next launch retries.
    Failed { exhausted: bool, report: String },
}

/// The tabs (`tabs`, `tabs.info`) of the window Desktop wrote last, read via a private scratch
/// copy so Official Desktop's folder is never written to.
pub fn recent_tabs_in(dirs: &[PathBuf]) -> Option<(Value, Value)> {
    let scratch = std::env::temp_dir().join("opencode-rune-seed");
    let PickResult::Picked(_, snap) = pick_dir(dirs, &scratch) else { return None };
    recent_tabs_from(snap.conn())
}

/// Legacy single-file helper kept safe via private scratch copy (never writes `-shm` beside `db`).
pub fn recent_tabs(db: &Path) -> Option<(Value, Value)> {
    let dir = db.parent()?;
    let scratch = std::env::temp_dir().join("opencode-rune-seed");
    let snap = open_snapshot(dir, &scratch).ok()?;
    recent_tabs_from(snap.conn())
}

fn recent_tabs_from(conn: &Connection) -> Option<(Value, Value)> {
    let name: String = conn
        .query_row(
            "SELECT name FROM state WHERE key = 'tabs' AND name LIKE 'opencode.window.%' ORDER BY updated_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .ok()??;
    let get = |key: &str| -> Value {
        conn.query_row("SELECT value FROM state WHERE name = ?1 AND key = ?2", [&name, key], |r| r.get::<_, String>(0))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Value::Null)
    };
    Some((get("tabs"), get("tabs.info")))
}

fn is_safe_window_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && !id.contains("..") && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn derived_last_active_url(win_rows: &[(String, String)], surviving_tabs: &[Tab]) -> Option<String> {
    if win_rows.iter().any(|(k, _)| k == "lastActiveUrl") {
        return None;
    }
    let recent_raw = win_rows.iter().find(|(k, _)| k == "tabs.recent")?.1.as_str();
    let recent_key = serde_json::from_str::<Value>(recent_raw).ok()?.get("key")?.as_str().and_then(TabKey::parse)?;
    if !surviving_tabs.iter().any(|t| t.key() == recent_key) {
        return None;
    }
    let href = match recent_key {
        TabKey::Session { server, session_id } => Route::Session { server, session_id }.href(),
        TabKey::Draft(id) => Route::Draft(id).href(),
    };
    Some(json!(href).to_string())
}

/// Scrubs secrets from Official's `opencode.global.dat / server` JSON: keeps `lastProjectID`,
/// `usedProjectIDs`, `hostnames`, `projects`, `hidden`, and server `list[]` entries (name, URL,
/// kind, username) while stripping `http.password`, `password`, `token`, `secret`, `authorization`,
/// and `headers`. Returns `(scrubbed_json, imported_server_urls)`.
fn scrub_server_json(raw: &str) -> Option<(String, HashSet<String>)> {
    let mut root = serde_json::from_str::<Value>(raw).ok()?;
    let obj = root.as_object_mut()?;
    let mut out = Map::new();
    for keep in ["lastProjectID", "usedProjectIDs", "hostnames", "projects", "hidden"] {
        if let Some(v) = obj.get(keep) {
            out.insert(keep.into(), v.clone());
        }
    }
    let mut imported_urls = HashSet::new();
    if let Some(list) = obj.get("list").and_then(Value::as_array) {
        let mut clean_list = Vec::with_capacity(list.len());
        for entry in list {
            let Some(em) = entry.as_object() else { continue };
            let mut clean = Map::new();
            for (k, v) in em {
                if is_secret_key(k) {
                    continue;
                }
                if k == "http" && let Some(hm) = v.as_object() {
                    let mut clean_http = Map::new();
                    for (hk, hv) in hm {
                        if is_secret_key(hk) {
                            continue;
                        }
                        if hk == "url" && let Some(u) = hv.as_str() {
                            imported_urls.insert(u.trim_end_matches('/').to_owned());
                        }
                        clean_http.insert(hk.clone(), hv.clone());
                    }
                    clean.insert("http".into(), Value::Object(clean_http));
                } else {
                    if k == "url" && let Some(u) = v.as_str() {
                        imported_urls.insert(u.trim_end_matches('/').to_owned());
                    }
                    clean.insert(k.clone(), v.clone());
                }
            }
            clean_list.push(Value::Object(clean));
        }
        out.insert("list".into(), Value::Array(clean_list));
    }
    Some((Value::Object(out).to_string(), imported_urls))
}

fn is_secret_key(k: &str) -> bool {
    let lower = k.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "password" | "token" | "secret" | "authorization" | "auth" | "headers" | "apikey" | "api_key" | "cookie" | "cookies"
    )
}

/// Returns how many prior failed seed attempts are recorded in `rune.storage`.
pub(crate) fn prior_attempts(conn: &Connection) -> u32 {
    conn.query_row("SELECT value FROM state WHERE name = ?1 AND key = ?2", [META_NS, ATTEMPTS_KEY], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0)
}

fn record_attempt(conn: &mut Connection, settle_flags: u32, now_ms: i64, why: String) -> SeedOutcome {
    let next = prior_attempts(conn).saturating_add(1);
    let exhausted = next >= MAX_SEED_ATTEMPTS;
    let _ = (|| -> rusqlite::Result<()> {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if exhausted {
            tx.execute("DELETE FROM state WHERE name = ?1 AND key = ?2", [META_NS, ATTEMPTS_KEY])?;
            schema::set_flags(&tx, settle_flags)?;
        } else {
            tx.execute(
                "INSERT INTO state (name, key, value, updated_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT (name, key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                (META_NS, ATTEMPTS_KEY, next.to_string(), now_ms),
            )?;
        }
        tx.commit()
    })();
    let suffix = if exhausted {
        format!("giving up after {next}/{MAX_SEED_ATTEMPTS} launches; official Desktop data untouched")
    } else {
        format!("attempt {next}/{MAX_SEED_ATTEMPTS}; will retry on next launch")
    };
    SeedOutcome::Failed { exhausted, report: format!("seed from Desktop deferred ({why}); {suffix}") }
}

/// On a retry launch (`prior_attempts > 0`), detects untouched blank default rows written by a
/// prior locked launch (a single empty Home window / empty `server` row) so the retry can replace
/// them without ever overwriting real user work (non-empty tabs, settings, servers, or drafts).
fn clear_blank_retry_defaults(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    let win_ids: Option<Vec<String>> = tx
        .query_row("SELECT value FROM state WHERE name = 'opencode.settings' AND key = 'windowIds'", [], |r| r.get::<_, String>(0))
        .optional()?
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok());
    if let Some(ids) = win_ids && ids.len() == 1 {
        let ns = crate::window_namespace(&ids[0]);
        let tabs_empty = tx
            .query_row("SELECT value FROM state WHERE name = ?1 AND key = 'tabs'", [&ns], |r| r.get::<_, String>(0))
            .optional()?
            .and_then(|s| serde_json::from_str::<Vec<Value>>(&s).ok())
            .is_none_or(|t| t.is_empty());
        let no_url = tx
            .query_row("SELECT value FROM state WHERE name = ?1 AND key = 'lastActiveUrl'", [&ns], |r| r.get::<_, String>(0))
            .optional()?
            .is_none_or(|u| u == "\"/\"" || u == "null");
        if tabs_empty && no_url {
            tx.execute("DELETE FROM state WHERE name = 'opencode.settings' AND key = 'windowIds'", [])?;
            tx.execute("DELETE FROM state WHERE name = ?1", [&ns])?;
            tx.execute("DELETE FROM window_state WHERE id = ?1", [&ids[0]])?;
        }
    }
    let srv_blank = tx
        .query_row("SELECT value FROM state WHERE name = 'opencode.global.dat' AND key = 'server'", [], |r| r.get::<_, String>(0))
        .optional()?
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .is_some_and(|v| {
            v["list"].as_array().is_none_or(Vec::is_empty)
                && v["usedProjectIDs"].as_object().is_none_or(|m| m.values().all(|a| a.as_array().is_none_or(Vec::is_empty)))
        });
    if srv_blank {
        tx.execute("DELETE FROM state WHERE name = 'opencode.global.dat' AND key = 'server'", [])?;
    }
    Ok(())
}

struct SeedBundle {
    rows: Vec<(String, String, String)>,
    docs: Vec<(String, String)>,
    blobs: Vec<(String, Vec<u8>)>,
    frames: Vec<(String, WindowFrame)>,
    window_count: usize,
    dropped_drafts: usize,
    notes: Vec<String>,
}

fn collect_bundle(dir: &Path, snap: &snapshot::Snapshot) -> Result<SeedBundle, String> {
    let official = snap.conn();
    let has_state = snap.has_table("state");
    let has_doc = snap.has_table("document");
    let has_blobs = snap.has_table("blob");

    let mut notes = Vec::new();
    let settings_val: Value = match read_capped(&dir.join("opencode.settings"), MAX_SETTINGS_JSON_BYTES) {
        Ok(Some(raw)) => serde_json::from_str(&raw).unwrap_or(Value::Null),
        Ok(None) => Value::Null,
        Err(e) => {
            notes.push(format!("skipped opencode.settings: {e}"));
            Value::Null
        }
    };

    let mut rows: Vec<(String, String, String)> = Vec::new();
    let mut state_bytes: usize = 0;
    let mut push_state = |name: String, key: String, val: String| {
        if val.len() <= MAX_STATE_VALUE_BYTES && state_bytes.saturating_add(val.len()) <= MAX_TOTAL_STATE_BYTES {
            state_bytes += val.len();
            rows.push((name, key, val));
        }
    };

    let mut imported_server_urls = HashSet::new();
    if has_state {
        if let Some(v3) = official
            .query_row("SELECT value FROM state WHERE name = 'default.dat' AND key = 'settings.v3'", [], |r| r.get::<_, String>(0))
            .optional()
            .map_err(|e| format!("read default.dat/settings.v3: {e}"))?
        {
            push_state("opencode.global.dat".into(), "settings.v3".into(), v3);
        }

        let mut stmt = official
            .prepare("SELECT key, value FROM state WHERE name = 'opencode.global.dat'")
            .map_err(|e| format!("prepare opencode.global.dat: {e}"))?;
        let found = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| format!("query opencode.global.dat: {e}"))?;
        for item in found {
            let (k, v) = item.map_err(|e| format!("read opencode.global.dat row: {e}"))?;
            if k == "server" {
                if let Some((scrubbed, urls)) = scrub_server_json(&v) {
                    imported_server_urls = urls;
                    push_state("opencode.global.dat".into(), "server".into(), scrubbed);
                }
            } else if GLOBAL_ALLOW.contains(&k.as_str()) {
                push_state("opencode.global.dat".into(), k, v);
            }
        }
    }

    let mut window_ids: Vec<String> = Vec::new();
    for id in settings_val["windowIds"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if is_safe_window_id(id) && !window_ids.iter().any(|x| x == id) {
            window_ids.push(id.to_owned());
            if window_ids.len() >= MAX_SEED_WINDOWS {
                break;
            }
        }
    }
    if window_ids.is_empty() && has_state {
        if let Some(name) = official
            .query_row(
                "SELECT name FROM state WHERE name LIKE 'opencode.window.%.dat' GROUP BY name ORDER BY MAX(updated_at) DESC LIMIT 1",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| format!("query newest window namespace: {e}"))?
            && let Some(id) = name.strip_prefix("opencode.window.").and_then(|s| s.strip_suffix(".dat"))
            && is_safe_window_id(id)
        {
            window_ids.push(id.to_owned());
        }
    }

    if let Some(obj) = settings_val.as_object() {
        for &k in SETTINGS_ALLOW {
            if let Some(v) = obj.get(k) {
                push_state("opencode.settings".into(), k.into(), v.to_string());
            }
        }
        if let Some(def_url) = obj.get("defaultServerUrl").and_then(Value::as_str) {
            let norm = def_url.trim_end_matches('/');
            if imported_server_urls.contains(norm) {
                push_state("opencode.settings".into(), "defaultServerUrl".into(), json!(def_url).to_string());
            }
        }
    }
    if !window_ids.is_empty() {
        push_state("opencode.settings".into(), "windowIds".into(), json!(&window_ids).to_string());
    }
    if let Some(z) = settings_val["zoomLevel"].as_f64().filter(|z| z.is_finite() && (0.25..=4.0).contains(z)) {
        push_state("opencode.global.dat".into(), "zoom".into(), json!(z).to_string());
    }

    let session_docs = if has_doc { load_session_docs(official)? } else { std::collections::HashMap::new() };
    let mut docs: Vec<(String, String)> = Vec::new();
    let mut blobs: Vec<(String, Vec<u8>)> = Vec::new();
    let mut blob_budget = MAX_SEED_TOTAL_BLOBS_BYTES;
    let mut frames: Vec<(String, WindowFrame)> = Vec::new();
    let mut dropped_drafts = 0usize;

    for wid in &window_ids {
        let ns = crate::window_namespace(wid);
        let mut win_rows: Vec<(String, String)> = Vec::new();
        if has_state {
            let mut stmt = official
                .prepare("SELECT key, value FROM state WHERE name = ?1")
                .map_err(|e| format!("prepare {ns}: {e}"))?;
            let found = stmt
                .query_map([&ns], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(|e| format!("query {ns}: {e}"))?;
            for item in found {
                let (k, v) = item.map_err(|e| format!("read {ns} row: {e}"))?;
                if WINDOW_ALLOW.contains(&k.as_str()) {
                    win_rows.push((k, v));
                }
            }
        }

        // Convert drafts for every tab in this window; drop any draft tab whose draft failed to
        // convert or missed a blob so we never open a blank tab in place of a user's draft.
        let mut surviving_tabs: Vec<Tab> = Vec::new();
        if let Some((_, tabs_raw)) = win_rows.iter_mut().find(|(k, _)| k == "tabs")
            && let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(tabs_raw)
        {
            let mut kept_raw = Vec::with_capacity(arr.len());
            for raw_tab in arr {
                let Some(tab) = Tab::from_json(&raw_tab) else { continue };
                match &tab {
                    Tab::Draft(draft) => {
                        let official_key = official_draft_doc_key(&draft.draft_id);
                        let doc_res = if has_doc {
                            read_official_doc(official, &official_key, has_blobs).and_then(|opt| match opt {
                                Some(raw) => convert_composer_store(&raw, official, has_blobs, &mut blobs, &mut blob_budget),
                                None => Ok(None),
                            })
                        } else {
                            Ok(None)
                        };
                        match doc_res {
                            Ok(Some(rune_doc)) => {
                                docs.push((crate::draft_document(wid, &tab.key().encode()), rune_doc));
                                surviving_tabs.push(tab);
                                kept_raw.push(raw_tab);
                            }
                            Ok(None) => {
                                surviving_tabs.push(tab);
                                kept_raw.push(raw_tab);
                            }
                            Err(why) => {
                                dropped_drafts += 1;
                                notes.push(format!("dropped draft tab {} (draft could not be migrated: {why})", draft.draft_id));
                            }
                        }
                    }
                    Tab::Session(sess) => {
                        if let Some(raw_doc) = session_docs.get(&sess.session_id)
                            && let Ok(Some(rune_doc)) = convert_composer_store(raw_doc, official, has_blobs, &mut blobs, &mut blob_budget)
                        {
                            docs.push((crate::draft_document(wid, &tab.key().encode()), rune_doc));
                        }
                        surviving_tabs.push(tab);
                        kept_raw.push(raw_tab);
                    }
                }
            }
            if dropped_drafts > 0 {
                *tabs_raw = Value::Array(kept_raw).to_string();
            }
        }

        if let Some(url_json) = derived_last_active_url(&win_rows, &surviving_tabs) {
            win_rows.push(("lastActiveUrl".into(), url_json));
        }
        for (k, v) in win_rows {
            push_state(ns.clone(), k, v);
        }

        match read_capped(&dir.join(format!("window-state-{wid}.json")), MAX_WINDOW_STATE_BYTES) {
            Ok(Some(raw)) => {
                if let Some(frame) = serde_json::from_str::<Value>(&raw).ok().and_then(|v| WindowFrame::from_json(&v)) {
                    frames.push((wid.clone(), frame));
                }
            }
            Ok(None) => {}
            Err(e) => notes.push(format!("skipped window-state-{wid}.json: {e}")),
        }
    }

    if has_doc && let Some(hist) = convert_prompt_history(official, has_blobs) {
        docs.push((crate::PROMPT_HISTORY_DOCUMENT.into(), hist));
    }

    Ok(SeedBundle { rows, docs, blobs, frames, window_count: window_ids.len(), dropped_drafts, notes })
}

/// Runs the first-run seed from Official Desktop inside ONE atomic transaction (migrated rows,
/// drafts, blobs, frames, boot record, and `settle_flags` commit together).
pub(crate) fn run(
    conn: &mut Connection,
    scratch_parent: &Path,
    dirs: &[PathBuf],
    settle_flags: u32,
    now_ms: i64,
) -> SeedOutcome {
    let (dir, snap) = match pick_dir(dirs, scratch_parent) {
        PickResult::None => {
            let _ = (|| -> rusqlite::Result<()> {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                tx.execute("DELETE FROM state WHERE name = ?1 AND key = ?2", [META_NS, ATTEMPTS_KEY])?;
                schema::set_flags(&tx, settle_flags)?;
                tx.commit()
            })();
            return SeedOutcome::NoSource;
        }
        PickResult::Failed(why) => return record_attempt(conn, settle_flags, now_ms, why),
        PickResult::Picked(d, s) => (d, s),
    };

    let bundle = match collect_bundle(&dir, &snap) {
        Ok(b) => b,
        Err(why) => {
            drop(snap);
            return record_attempt(conn, settle_flags, now_ms, why);
        }
    };
    // Drop & delete the scratch snapshot copy before writing into Rune's DB.
    drop(snap);

    let is_retry = prior_attempts(conn) > 0;
    let commit = (|| -> rusqlite::Result<Vec<(String, String, String)>> {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if is_retry {
            clear_blank_retry_defaults(&tx)?;
        }
        for (id, data) in &bundle.blobs {
            sql::insert_blob(&tx, id, data, now_ms)?;
        }
        for (key, value) in &bundle.docs {
            let (stored, chunks) = sql::encode_text(value);
            for (cid, cdata) in &chunks {
                sql::insert_blob(&tx, cid, cdata, now_ms)?;
            }
            tx.execute("INSERT INTO document (key, value) VALUES (?1, ?2) ON CONFLICT DO NOTHING", (key, stored))?;
        }
        let mut inserted = Vec::new();
        for (name, key, value) in bundle.rows {
            if tx.execute("INSERT INTO state (name, key, value, updated_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING", (&name, &key, &value, now_ms))? > 0 {
                inserted.push((name, key, value));
            }
        }
        for (id, frame) in &bundle.frames {
            if tx.query_row("SELECT 1 FROM window_state WHERE id = ?1", [id], |_| Ok(())).optional()?.is_none() {
                sql::put_window(&tx, id, Some(frame), now_ms)?;
            }
        }
        tx.execute("DELETE FROM state WHERE name = ?1 AND key = ?2", [META_NS, ATTEMPTS_KEY])?;
        boot_record::rebuild(&tx)?;
        schema::set_flags(&tx, settle_flags)?;

        // Opt-in crash-recovery proof hook (strictly requires RUNE_DIRS_ROOT so it can never fire
        // on a real user profile): terminates before tx.commit() so SQLite rolls back atomically.
        if std::env::var_os("RUNE_DIRS_ROOT").is_some_and(|v| !v.is_empty())
            && std::env::var_os("RUNE_TEST_SEED_CRASH_BEFORE_COMMIT").is_some_and(|v| v == "1")
        {
            std::process::exit(99);
        }

        tx.commit()?;
        Ok(inserted)
    })();

    match commit {
        Ok(inserted) => {
            let mut report = bundle.notes;
            report.push(format!(
                "seeded {} state rows, {} docs, {} blobs, {} frames ({} windows, {} dropped drafts) from {}",
                inserted.len(),
                bundle.docs.len(),
                bundle.blobs.len(),
                bundle.frames.len(),
                bundle.window_count,
                bundle.dropped_drafts,
                dir.display()
            ));
            SeedOutcome::Seeded { rows: inserted, report }
        }
        Err(e) => record_attempt(conn, settle_flags, now_ms, format!("commit failed: {e}")),
    }
}

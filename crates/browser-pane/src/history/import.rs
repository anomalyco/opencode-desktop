//! Read-only browser history snapshot importer (`UP-106` `#65073`,
//! `packages/gui-plugins/src/browser/profile/{history.ts,database.ts}`). Which browsers and
//! profiles exist is `cookies::sources` (official `sources.ts`); this reads one profile's history.
//!
//! Safety invariants:
//! - Called ONLY on explicit user action inside Settings > Browser, never on startup.
//! - Copies the target SQLite database (+ `-wal` / `-shm` if present) into a private scratch dir
//!   and opens the copy strictly read-only (`SQLITE_OPEN_READ_ONLY`, `PRAGMA query_only = ON`,
//!   `PRAGMA temp_store = MEMORY`). Source profiles are never modified or locked.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::config::DbConfig;
use rusqlite::{Connection, OpenFlags, params};

use super::page::{MAX_PAGES, MAX_URL_LENGTH, PageVisit, web_page};

const WINDOWS_EPOCH_MS: i64 = 11_644_473_600_000;
const MAC_EPOCH_SECONDS: i64 = 978_307_200;
const MOST_VISITS: i64 = 10_000_000;
const LATEST_VISIT_MS: i64 = 4_102_444_800_000;
const MAX_IMPORT_DB_BYTES: u64 = 256 << 20;
const SNAP_PREFIX: &str = "opencode-hist-snap-";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportEngine {
    Chromium,
    Firefox,
    Safari,
}

impl ImportEngine {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chromium => "chromium",
            Self::Firefox => "firefox",
            Self::Safari => "safari",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "chromium" => Some(Self::Chromium),
            "firefox" => Some(Self::Firefox),
            "safari" => Some(Self::Safari),
            _ => None,
        }
    }
}

/// Removes stale `opencode-hist-snap-*` directories left behind by an earlier process that exited
/// mid-import.
pub fn sweep_stale_import_dirs(scratch_base: &Path) {
    let Ok(entries) = fs::read_dir(scratch_base) else { return };
    let me = std::process::id();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(rest) = name.strip_prefix(SNAP_PREFIX) else { continue };
        let Some(pid) = rest.split('-').next().and_then(|p| p.parse::<u32>().ok()) else { continue };
        if pid != me {
            #[cfg(feature = "engine")]
            let alive = crate::background::pid_alive(pid);
            #[cfg(not(feature = "engine"))]
            let alive = false;
            if !alive {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
}

/// Reads up to [`MAX_PAGES`] web history rows from `db_path` by copying it into a private scratch
/// directory under `scratch_base` (or the OS temp dir if `None`) and opening the copy strictly
/// read-only with defensive SQLite hardening (`SQLITE_DBCONFIG_DEFENSIVE`, `trusted_schema = OFF`,
/// `type = 'table'` verification, and a 256 MiB file size cap).
pub fn read_profile_history_in(
    engine: ImportEngine,
    db_path: &Path,
    scratch_base: Option<&Path>,
) -> Result<Vec<PageVisit>, String> {
    let raw = db_path.to_str().ok_or_else(|| "History database path is not UTF-8".to_owned())?;
    let verified = browser_policy::verify_local_path(raw).map_err(|_| "History database path refused".to_owned())?;
    if !verified.is_file {
        return Err("History database not found".into());
    }
    let clean_db = PathBuf::from(&verified.path);
    let db_len = fs::metadata(&clean_db).map_err(|e| format!("stat history db: {e}"))?.len();
    if db_len == 0 || db_len > MAX_IMPORT_DB_BYTES {
        return Err("History database size out of bounds".into());
    }
    let base = scratch_base.map_or_else(std::env::temp_dir, Path::to_path_buf);
    fs::create_dir_all(&base).map_err(|e| format!("create scratch base: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&base, fs::Permissions::from_mode(0o700));
    }
    sweep_stale_import_dirs(&base);

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let scratch = base.join(format!("{SNAP_PREFIX}{}-{stamp}", std::process::id()));
    fs::create_dir_all(&scratch).map_err(|e| format!("create scratch: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&scratch, fs::Permissions::from_mode(0o700));
    }
    let _guard = ScratchDir(scratch.clone());

    let snap = scratch.join("snapshot.sqlite");
    fs::copy(&clean_db, &snap).map_err(|e| format!("copy history db: {e}"))?;
    for suffix in ["-wal", "-shm"] {
        let mut side = clean_db.as_os_str().to_os_string();
        side.push(suffix);
        let side_path = PathBuf::from(side);
        if let Some(side_str) = side_path.to_str()
            && let Ok(v) = browser_policy::verify_local_path(side_str)
            && v.is_file
            && fs::metadata(&v.path).is_ok_and(|m| m.len() <= MAX_IMPORT_DB_BYTES)
        {
            let mut dst = snap.as_os_str().to_os_string();
            dst.push(suffix);
            let _ = fs::copy(&v.path, PathBuf::from(dst));
        }
    }

    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(&snap, flags).map_err(|e| format!("open snapshot: {e}"))?;
    let _ = conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true);
    conn.execute_batch(
        "PRAGMA trusted_schema = OFF; PRAGMA query_only = ON; PRAGMA temp_store = MEMORY;",
    )
    .map_err(|e| format!("harden snapshot: {e}"))?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    match engine {
        ImportEngine::Chromium => read_chromium(&conn, now),
        ImportEngine::Firefox => read_firefox(&conn, now),
        ImportEngine::Safari => read_safari(&conn, now),
    }
}

/// Reads up to [`MAX_PAGES`] web history rows from `db_path` using the default scratch location.
pub fn read_profile_history(engine: ImportEngine, db_path: &Path) -> Result<Vec<PageVisit>, String> {
    read_profile_history_in(engine, db_path, None)
}

struct ScratchDir(PathBuf);
impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Verifies `table` is a real base table (`type = 'table'` in `sqlite_master`), refusing views
/// and virtual tables inside untrusted profile databases.
fn is_real_table(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
        [table],
        |r| r.get::<_, i64>(0),
    )
    .is_ok()
}

fn table_columns(conn: &Connection, table: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    if !is_real_table(conn, table) {
        return set;
    }
    let sql = format!("PRAGMA table_info({table})");
    if let Ok(mut stmt) = conn.prepare(&sql)
        && let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(1))
    {
        for name in rows.flatten() {
            set.insert(name);
        }
    }
    set
}

fn read_chromium(conn: &Connection, now: i64) -> Result<Vec<PageVisit>, String> {
    let cols = table_columns(conn, "urls");
    if !cols.contains("url") {
        return Ok(Vec::new());
    }
    let select = format!(
        "SELECT url,
           {} AS title,
           {} AS visits,
           {} AS typed,
           {} AS visited
         FROM urls WHERE {}",
        if cols.contains("title") { "title" } else { "NULL" },
        if cols.contains("visit_count") { "visit_count" } else { "1" },
        if cols.contains("typed_count") { "typed_count" } else { "0" },
        if cols.contains("last_visit_time") {
            format!("last_visit_time / 1000 - {WINDOWS_EPOCH_MS}")
        } else {
            "0".into()
        },
        if cols.contains("hidden") { "hidden = 0" } else { "1" },
    );
    collect_rows(conn, &select, now)
}

fn read_firefox(conn: &Connection, now: i64) -> Result<Vec<PageVisit>, String> {
    let cols = table_columns(conn, "moz_places");
    if !cols.contains("url") {
        return Ok(Vec::new());
    }
    let select = format!(
        "SELECT url,
           {} AS title,
           {} AS visits,
           {} AS typed,
           {} AS visited
         FROM moz_places WHERE {}",
        if cols.contains("title") { "title" } else { "NULL" },
        if cols.contains("visit_count") { "visit_count" } else { "1" },
        if cols.contains("typed") { "typed" } else { "0" },
        if cols.contains("last_visit_date") { "last_visit_date / 1000" } else { "0" },
        if cols.contains("hidden") { "hidden = 0" } else { "1" },
    );
    collect_rows(conn, &select, now)
}

fn read_safari(conn: &Connection, now: i64) -> Result<Vec<PageVisit>, String> {
    let items = table_columns(conn, "history_items");
    let visits = table_columns(conn, "history_visits");
    if !items.contains("id") || !items.contains("url") || !visits.contains("history_item") || !visits.contains("visit_time") {
        return Ok(Vec::new());
    }
    let select = format!(
        "SELECT i.url AS url,
           {} AS title,
           {} AS visits,
           0 AS typed,
           (max(v.visit_time) + {MAC_EPOCH_SECONDS}) * 1000 AS visited
         FROM history_items i JOIN history_visits v ON v.history_item = i.id GROUP BY i.id",
        if visits.contains("title") { "v.title" } else { "NULL" },
        if items.contains("visit_count") { "i.visit_count" } else { "count(*)" },
    );
    collect_rows(conn, &select, now)
}

fn collect_rows(conn: &Connection, select: &str, now: i64) -> Result<Vec<PageVisit>, String> {
    let sql = format!(
        "SELECT url, title, visits, typed, visited FROM (
           SELECT url,
             CASE WHEN typeof(title) = 'text' THEN title ELSE '' END AS title,
             CASE WHEN typeof(visits) = 'integer' THEN min(visits, {MOST_VISITS}) ELSE 0 END AS visits,
             CASE WHEN typeof(typed) = 'integer' THEN max(0, min(typed, {MOST_VISITS})) ELSE 0 END AS typed,
             CASE WHEN typeof(visited) IN ('integer', 'real') THEN CAST(max(0, min(visited, {LATEST_VISIT_MS})) AS INTEGER) ELSE 0 END AS visited
           FROM ({select})
           WHERE typeof(url) = 'text' AND length(url) <= {MAX_URL_LENGTH}
             AND (url LIKE 'http://%' OR url LIKE 'https://%')
         )
         WHERE visits > 0
         ORDER BY (visits + typed) * CASE
           WHEN ?1 - visited <= 345600000 THEN 100
           WHEN ?2 - visited <= 1209600000 THEN 70
           WHEN ?3 - visited <= 2678400000 THEN 50
           WHEN ?4 - visited <= 7776000000 THEN 30
           ELSE 10 END DESC, visited DESC
         LIMIT {MAX_PAGES}"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| format!("query snapshot: {e}"))?;
    let mut rows = stmt.query(params![now, now, now, now]).map_err(|e| format!("exec snapshot: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(|e| format!("read snapshot row: {e}"))? {
        let url: String = row.get(0).unwrap_or_default();
        if web_page(&url).is_none() {
            continue;
        }
        let title: String = row.get(1).unwrap_or_default();
        let visits: i64 = row.get(2).unwrap_or(0);
        let typed: i64 = row.get(3).unwrap_or(0);
        let visited: i64 = row.get(4).unwrap_or(0);
        out.push(PageVisit { url, title, visits, typed: typed.min(visits), visited });
        if out.len() >= MAX_PAGES {
            break;
        }
    }
    Ok(out)
}


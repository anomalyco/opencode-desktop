//! The open path, which is the product's critical path: open the file (no create probe on a
//! profile that has one), set the connection's pragmas, check the schema from the header, and
//! read everything the first frame draws in one read transaction. The steady state does no
//! directory scan and no `stat` of any old file: whether the import is needed is a header flag.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use rusqlite::{Connection, OpenFlags};

use super::import::{self, ImportSummary};
use super::schema::{self, FLAG_LEGACY_SETTLED, FLAG_SEED_SETTLED, SCHEMA_VERSION};
use super::{Backend, StorageConfig, sql, writer};
use crate::cache::{Cache, Doc, Namespace, OutboxCache};
use crate::{Storage, WindowFrame};

/// What the open did, each phase in nanoseconds on the monotonic clock (`Instant`).
#[derive(Clone, Debug, Default)]
pub struct BootReport {
    /// When the open started; subtract the process's own start for process-relative times.
    pub started: Option<Instant>,
    /// The file didn't exist and was created.
    pub created: bool,
    /// The database couldn't open or migrate: this run keeps state in memory.
    pub error: Option<String>,
    pub open_ns: u64,
    pub pragma_ns: u64,
    pub schema_ns: u64,
    pub import_ns: u64,
    pub read_ns: u64,
    pub total_ns: u64,
    /// Boot `state` rows read, and their value bytes.
    pub state_rows: usize,
    pub value_bytes: usize,
    pub document_keys: usize,
    /// Documents read in full (each window's visible draft).
    pub documents: usize,
    pub windows: usize,
    pub plugins: usize,
    /// Pages SQLite read from the file (page cache misses), and the page size.
    pub pages_read: i64,
    pub page_size: i64,
    pub migrated: Vec<&'static str>,
    pub import: ImportSummary,
}

/// The boot state as read, for [`StorageConfig::boot_documents`] to pick the visible drafts from.
pub struct BootView<'a> {
    rows: &'a HashMap<String, BTreeMap<String, Arc<str>>>,
}

impl BootView<'_> {
    pub fn namespaces(&self) -> impl Iterator<Item = &str> {
        self.rows.keys().map(String::as_str)
    }

    pub fn get(&self, name: &str, key: &str) -> Option<&str> {
        self.rows.get(name)?.get(key).map(|v| &**v)
    }
}

/// The default pick: each window's draft for the tab it shows (`tabs.recent`).
pub fn visible_drafts(view: &BootView<'_>) -> Vec<String> {
    view.namespaces()
        .filter(|n| n.starts_with("opencode.window."))
        .filter_map(|n| {
            let recent: serde_json::Value = serde_json::from_str(view.get(n, "tabs.recent")?).ok()?;
            Some(format!("{n}:draft:{}", recent["key"].as_str()?))
        })
        .collect()
}

const MMAP_SIZE: i64 = 64 * 1024 * 1024;

pub(super) fn open(config: StorageConfig) -> (Storage, BootReport) {
    let t0 = Instant::now();
    let mut report = BootReport { started: Some(t0), ..BootReport::default() };
    let lap = |since: &mut Instant| {
        let now = Instant::now();
        let ns = u64::try_from(now.duration_since(*since).as_nanos()).unwrap_or(u64::MAX);
        *since = now;
        ns
    };
    let mut at = t0;
    let result = (|| -> rusqlite::Result<(Connection, Cache, import::Deferred)> {
        let (mut conn, created) = connect(&config.path)?;
        report.created = created;
        report.open_ns = lap(&mut at);
        configure(&conn)?;
        report.pragma_ns = lap(&mut at);
        let version = schema::user_version(&conn)?;
        let flags = version & 0xff;
        if version >> 8 != SCHEMA_VERSION {
            conn.execute_batch("PRAGMA journal_mode = WAL")?;
            report.migrated = schema::migrate(&mut conn, (config.clock)())?;
            schema::set_flags(&conn, flags)?;
        }
        report.schema_ns = lap(&mut at);
        let deferred = match &config.import {
            Some(import) => {
                let (summary, deferred, settled) = import::run(&mut conn, import, &config.official, flags, (config.clock)());
                report.import = summary;
                if settled != flags {
                    schema::set_flags(&conn, settled)?;
                }
                deferred
            }
            None => {
                if flags != FLAG_LEGACY_SETTLED | FLAG_SEED_SETTLED {
                    schema::set_flags(&conn, FLAG_LEGACY_SETTLED | FLAG_SEED_SETTLED)?;
                }
                import::Deferred::default()
            }
        };
        report.import_ns = lap(&mut at);
        let mut cache = read(&conn, config.boot_documents, &mut report)?;
        cache.complete = report.created && report.import.rows == 0;
        if cache.complete {
            cache.outbox = OutboxCache::Loaded(BTreeMap::new());
        }
        report.read_ns = lap(&mut at);
        report.pages_read = sql::cache_misses(&conn);
        report.page_size = conn.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap_or(0);
        Ok((conn, cache, deferred))
    })();
    report.total_ns = u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX);
    match result {
        Ok((conn, cache, deferred)) => {
            let cache = super::shared(cache);
            let w = writer::Writer::new(conn, cache.clone(), config.clock, deferred);
            (Storage::from_parts(cache, Backend::parked(w)), report)
        }
        Err(e) => {
            report.error = Some(e.to_string());
            (Storage::memory(), report)
        }
    }
}

/// Opens an existing database without the create probe; creates it only when it isn't there.
fn connect(path: &Path) -> rusqlite::Result<(Connection, bool)> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_EXRESCODE;
    match Connection::open_with_flags(path, flags) {
        Ok(conn) => Ok((conn, false)),
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::CannotOpen => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).ok();
            }
            let conn = Connection::open_with_flags(path, flags | OpenFlags::SQLITE_OPEN_CREATE)?;
            conn.execute_batch("PRAGMA page_size = 4096")?;
            Ok((conn, true))
        }
        Err(e) => Err(e),
    }
}

/// The connection's own settings (none touch the file). `journal_mode = WAL` is stored in the
/// file and set once, with the schema.
fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(&format!(
        "PRAGMA synchronous = NORMAL; PRAGMA temp_store = MEMORY; PRAGMA cache_size = -8000; PRAGMA busy_timeout = 5000; \
         PRAGMA wal_autocheckpoint = 1000; PRAGMA journal_size_limit = 0; PRAGMA mmap_size = {MMAP_SIZE};"
    ))?;
    sql::persist_wal(conn);
    Ok(())
}

/// The boot read: every boot namespace's rows, the document keys, each visible draft, every
/// window frame, plugin override and trust, in one read transaction.
fn read(conn: &Connection, pick: fn(&BootView<'_>) -> Vec<String>, report: &mut BootReport) -> rusqlite::Result<Cache> {
    let tx = conn.unchecked_transaction()?;
    let mut rows: HashMap<String, BTreeMap<String, Arc<str>>> = HashMap::new();
    {
        let mut stmt = tx.prepare_cached(
            "SELECT name, key, value FROM state WHERE name IN ('opencode.settings', 'opencode.global.dat', 'plugin.file', 'plugin.browser', 'plugin.updater') \
             OR (name >= 'opencode.window.' AND name < 'opencode.window/')",
        )?;
        let mut q = stmt.query([])?;
        while let Some(r) = q.next()? {
            let (name, key, value): (String, String, String) = (r.get(0)?, r.get(1)?, r.get(2)?);
            report.state_rows += 1;
            report.value_bytes += value.len();
            rows.entry(name).or_default().insert(key, value.into());
        }
    }
    let mut cache = Cache::default();
    {
        let mut stmt = tx.prepare_cached("SELECT key FROM document")?;
        let mut q = stmt.query([])?;
        while let Some(r) = q.next()? {
            cache.docs.insert(r.get(0)?, Doc::Stored);
            report.document_keys += 1;
        }
    }
    for key in pick(&BootView { rows: &rows }) {
        if !matches!(cache.docs.get(&key), Some(Doc::Stored)) {
            continue;
        }
        if let Some(value) = sql::read_document(&tx, &key)? {
            report.value_bytes += value.len();
            report.documents += 1;
            cache.docs.insert(key, Doc::Present(value.into()));
        }
    }
    {
        let mut stmt = tx.prepare_cached("SELECT id, x, y, width, height, maximized, full_screen, display_x, display_y, display_w, display_h FROM window_state")?;
        let mut q = stmt.query([])?;
        while let Some(r) = q.next()? {
            let display = match (r.get::<_, Option<i32>>(7)?, r.get::<_, Option<i32>>(8)?, r.get::<_, Option<i32>>(9)?, r.get::<_, Option<i32>>(10)?) {
                (Some(x), Some(y), Some(w), Some(h)) => Some([x, y, w, h]),
                _ => None,
            };
            let frame = WindowFrame { x: r.get(1)?, y: r.get(2)?, width: r.get(3)?, height: r.get(4)?, maximized: r.get(5)?, full_screen: r.get(6)?, display };
            cache.windows.insert(r.get(0)?, frame);
            report.windows += 1;
        }
    }
    {
        let mut stmt = tx.prepare_cached("SELECT id, enabled FROM plugin")?;
        let mut q = stmt.query([])?;
        while let Some(r) = q.next()? {
            cache.plugins.insert(r.get(0)?, r.get(1)?);
            report.plugins += 1;
        }
        let mut stmt = tx.prepare_cached("SELECT id, sha256 FROM plugin_trust")?;
        let mut q = stmt.query([])?;
        while let Some(r) = q.next()? {
            cache.trust.insert(r.get(0)?, r.get(1)?);
        }
    }
    tx.commit()?;
    for (name, rows) in rows {
        cache.state.insert(name, Namespace::Loaded { rows, revision: 1 });
    }
    Ok(cache)
}

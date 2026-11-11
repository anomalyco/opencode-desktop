//! The schema. Desktop's tables where the data is the same, with Desktop's columns
//! (`packages/desktop/src/main/storage/schema.ts:4-40`: `state`, `document`, `blob`, `plugin`),
//! then Rune's own (`plugin_trust`, `window_state`, `outbox`, `session_preview`, `cache`, `boot`).
//!
//! Desktop never opens this file (its own is `drafts.sqlite`), so the layout serves the open:
//! - Small tables are `WITHOUT ROWID`, clustered on their key: a namespace's rows sit together
//!   and a lookup is one B-tree walk, with no separate primary-key index in the schema.
//! - `boot` is created first, so its root is page 2: [`super::boot_record`] reads its one row
//!   with a plain file read, without SQLite, when the WAL is empty.
//! - Desktop's downgrade view (`extension`) and its retired migration steps aren't carried:
//!   nothing older reads this file.
//!
//! The journal (`migration`, as Desktop's `migration.ts:5-33`) records every applied id; whether
//! anything is pending is answered without reading it: the header's `user_version` holds
//! [`SCHEMA_VERSION`] once the journal is complete.

use rusqlite::Connection;

/// Bumped with every migration added below.
pub const SCHEMA_VERSION: u32 = 1;

/// `user_version` = `SCHEMA_VERSION << 8 | flags`.
pub const FLAG_LEGACY_SETTLED: u32 = 1;
pub const FLAG_SEED_SETTLED: u32 = 2;
/// `boot`'s root is page 2, so the fast reader may trust it.
pub const FLAG_BOOT_PAGE2: u32 = 4;

/// Created before anything else in a new file, so it takes page 2.
const BOOT: &str = "CREATE TABLE IF NOT EXISTS boot (id INTEGER PRIMARY KEY, data BLOB NOT NULL)";

/// `(id, statements)`, applied in order inside one transaction.
pub const MIGRATIONS: &[(&str, &[&str])] = &[(
    "20261111000000_rune-storage",
    &[
        "CREATE TABLE state (name TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, updated_at INTEGER NOT NULL, PRIMARY KEY (name, key)) WITHOUT ROWID",
        "CREATE TABLE document (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID",
        // Large values: a rowid table keeps a blob's overflow chain out of the key's B-tree.
        "CREATE TABLE blob (id TEXT PRIMARY KEY, data BLOB NOT NULL, touched_at INTEGER NOT NULL DEFAULT 0)",
        "CREATE TABLE plugin (id TEXT PRIMARY KEY, enabled INTEGER NOT NULL DEFAULT 1, manifest TEXT, revision TEXT) WITHOUT ROWID",
        "CREATE TABLE plugin_trust (id TEXT PRIMARY KEY, sha256 TEXT NOT NULL, trusted_at INTEGER NOT NULL) WITHOUT ROWID",
        "CREATE TABLE window_state (id TEXT PRIMARY KEY, x INTEGER, y INTEGER, width INTEGER NOT NULL, height INTEGER NOT NULL, maximized INTEGER NOT NULL DEFAULT 0, \
         full_screen INTEGER NOT NULL DEFAULT 0, display_x INTEGER, display_y INTEGER, display_w INTEGER, display_h INTEGER, updated_at INTEGER NOT NULL) WITHOUT ROWID",
        "CREATE TABLE outbox (server TEXT NOT NULL, id TEXT NOT NULL, seq INTEGER NOT NULL, session TEXT NOT NULL, state TEXT NOT NULL, operation_json TEXT NOT NULL, \
         draft_json TEXT NOT NULL, prompt_json TEXT, attachment_blobs_json TEXT NOT NULL DEFAULT '[]', paths_json TEXT NOT NULL DEFAULT '[]', inbox INTEGER NOT NULL DEFAULT 0, \
         why TEXT NOT NULL DEFAULT '', after_check TEXT, restored INTEGER NOT NULL DEFAULT 0, durable INTEGER NOT NULL DEFAULT 1, legacy_json TEXT, updated_at INTEGER NOT NULL, \
         PRIMARY KEY (server, id)) WITHOUT ROWID",
        // What the first screen of a session draws (its latest visible messages, in the
        // timeline's own encoding), kept current by the app; the active tabs' go in the boot record.
        "CREATE TABLE session_preview (server TEXT NOT NULL, session TEXT NOT NULL, updated_at INTEGER NOT NULL, data BLOB NOT NULL, PRIMARY KEY (server, session))",
        // Discardable caches of computed inputs (compiled bytecode, shaped runs), keyed and versioned.
        "CREATE TABLE cache (key TEXT PRIMARY KEY, version INTEGER NOT NULL, data BLOB NOT NULL, touched_at INTEGER NOT NULL)",
    ],
)];

/// Applies what the journal lacks, in one transaction. Returns the ids applied and whether
/// `boot` has page 2.
pub fn migrate(conn: &mut Connection, now_ms: i64) -> rusqlite::Result<(Vec<&'static str>, bool)> {
    conn.execute_batch(&format!("{BOOT}; CREATE TABLE IF NOT EXISTS migration (id TEXT PRIMARY KEY, time_completed INTEGER NOT NULL) WITHOUT ROWID"))?;
    let page2 = conn.query_row("SELECT rootpage FROM sqlite_master WHERE name = 'boot'", [], |r| r.get::<_, i64>(0))? == 2;
    let applied: Vec<String> = conn.prepare("SELECT id FROM migration")?.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let pending: Vec<&(&str, &[&str])> = MIGRATIONS.iter().filter(|(id, _)| !applied.iter().any(|a| a == id)).collect();
    if pending.is_empty() {
        return Ok((Vec::new(), page2));
    }
    let tx = conn.transaction()?;
    for (id, statements) in &pending {
        for s in statements.iter() {
            tx.execute_batch(s)?;
        }
        tx.execute("INSERT INTO migration (id, time_completed) VALUES (?1, ?2)", (id, now_ms))?;
    }
    tx.commit()?;
    Ok((pending.iter().map(|(id, _)| *id).collect(), page2))
}

pub fn user_version(conn: &Connection) -> rusqlite::Result<u32> {
    conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).map(|v| u32::try_from(v).unwrap_or(0))
}

pub fn set_flags(conn: &Connection, flags: u32) -> rusqlite::Result<()> {
    conn.execute_batch(&format!("PRAGMA user_version = {}", (SCHEMA_VERSION << 8) | flags))
}

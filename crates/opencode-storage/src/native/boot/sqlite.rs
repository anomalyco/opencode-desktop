//! Stage B SQLite open, schema migration, draft journal reconciliation, early cache offer, and
//! boot-namespace cache hydration.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use rusqlite::{Connection, OpenFlags};

use super::owed::{Owe, attach_journal, record_owns_journal};
use super::{BootReport, ns};
use crate::cache::{Cache, Doc, Namespace, OutboxCache};
use crate::native::boot_record::{self, BootPacket, BootSource};
use crate::native::import;
use crate::native::schema::{self, FLAG_BITS, FLAG_BOOT_PAGE2, FLAG_MASK, FLAG_SEED_SETTLED, ImportGroup, SCHEMA_VERSION};
use crate::native::writer::OpenJournal;
use crate::native::{EarlyCache, StorageConfig, journal, sql};
use crate::WindowFrame;

const MMAP_SIZE: i64 = 64 * 1024 * 1024;

/// How stage B meets another process's lock: wait `busy_timeout`, or answer busy at once.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Lock {
    Wait,
    Probe,
}

/// SQLite's part. `send` is set when stage A still owes the UI its packet: then the journal is
/// made sure of and the packet sent (`owe.packet`), or the record is read through SQLite as soon
/// as the schema and import allow.
pub(super) fn stage_b(
    config: &StorageConfig,
    report: &mut BootReport,
    t0: Instant,
    mut send: Option<&dyn Fn(BootPacket)>,
    owe: &mut Owe,
    early: &mut Vec<EarlyCache>,
    lock: Lock,
) -> rusqlite::Result<(Connection, Cache, import::Deferred)> {
    let mut at = Instant::now();
    let lap = |at: &mut Instant| {
        let n = ns(*at);
        *at = Instant::now();
        n
    };
    let (mut conn, mut created) = connect(&config.path)?;
    report.open_ns = lap(&mut at);
    // A probe answers busy at once: the pragmas already read the file (and so take its lock).
    if lock == Lock::Probe {
        conn.busy_timeout(std::time::Duration::ZERO)?;
    }
    configure(&conn, lock == Lock::Wait)?;
    report.pragma_ns = lap(&mut at);
    let mut version = schema::user_version(&conn)?;
    // A file whose creation never finished (it gets its version last; a new file's first writes
    // have no journal, see below): nothing is in it yet, so it's made again.
    if version == 0 && !created {
        drop(conn);
        for suffix in ["", "-wal", "-shm"] {
            std::fs::remove_file(format!("{}{suffix}", config.path.display())).ok();
        }
        (conn, created) = connect(&config.path)?;
        configure(&conn, lock == Lock::Wait)?;
        version = 0;
    }
    report.created = created;
    // The first read took the lock (exclusive mode keeps it): the rest may wait as usual.
    if lock == Lock::Probe {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
    }
    let mut flags = version & FLAG_MASK;
    if version >> FLAG_BITS > SCHEMA_VERSION {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let new = version >> FLAG_BITS < schema::BASE_SCHEMA;
    rune::marks::mark("storage.sqlite");
    if new {
        // A new file's first tables and the journal's pages go straight into it: no WAL to write
        // them to and checkpoint them out of before the windows can have their packet. A kill
        // before the version is set leaves a file that's made again (above).
        conn.execute_batch("PRAGMA journal_mode = OFF")?;
    }
    // The draft journal exists before the windows get their packet (so before any key), and in a
    // new file before any other table, so its first page is inside the boot read's first 128 KiB;
    // a file with data and no place for it gets one after the first frame.
    if owe.want_journal && journal::SUPPORTED && !journal::attached() {
        owe.journal = OpenJournal::Settled;
        // The WAL holds frames here: the file alone may be behind, so SQLite confirms the row.
        let hinted = owe.hint.take().is_some_and(|loc| journal::row_ok(&conn, &loc) && attach_journal(config, report, owe, &loc, false));
        let existing = if hinted { None } else { journal::location(&conn)? };
        // A profile from before the journal gets one only when the switch says so.
        if !hinted && !new && existing.is_none() && !schema::upgrade_existing() {
            report.journal_error = Some("off for profiles made before it (RUNE_STORAGE_V2)".into());
        } else if !hinted {
            // A journal the boot record's writer didn't keep, in a record that changed since the
            // reservation: a run without the journal rebuilt it (an older build, or one where the
            // journal was off), so what the journal holds may be older than what that run saved.
            // Its records are dropped: the place is reserved again, zeroed. (A record nobody
            // rebuilt yet, the first launch after the update killed early, keeps them:
            // `journal::current`.)
            if let Some(loc) = existing.as_ref().filter(|_| !new)
                && !journal::current(&conn, record_owns_journal(&conn, loc.instance))
            {
                conn.execute_batch("DELETE FROM journal_meta")?;
            }
            let t = Instant::now();
            if !new && journal::kept(&conn)?.is_none() {
                // The first launch after the update that brought it: reserved after the first frame
                // (`writer::reserve`); the keys meanwhile go to the database, and are held for it.
                owe.journal = OpenJournal::Owed;
                rune::marks::mark("journal.owed");
            } else {
                match journal::provision(&conn, &config.path, config.clock) {
                    Ok((loc, fresh)) => {
                        report.journal_reserved = fresh;
                        report.journal_ns += ns(t);
                        // The record didn't name this place: the next checkpoint's record will.
                        owe.journal = OpenJournal::Reserved;
                        // A recorded place that no longer holds this database's journal (an
                        // outside tool rewrote the file) is reserved again, after the first frame.
                        if !attach_journal(config, report, owe, &loc, fresh) && !fresh {
                            conn.execute_batch("DELETE FROM journal_meta")?;
                            owe.journal = OpenJournal::Owed;
                        }
                    }
                    Err(e) => report.journal_error = Some(e.to_string()),
                }
            }
        }
    }
    // Decided: keys held meanwhile are in the journal, or (no journal) the database's alone; an
    // owed one keeps holding them for the writer.
    if owe.journal != OpenJournal::Owed {
        journal::decided();
    }
    if let (Some(s), Some(mut p)) = (send, owe.packet.take()) {
        p.apply_journal(&owe.recovered);
        report.stage_a_ns = ns(t0);
        owe.sent = Some(p.clone());
        s(p);
        send = None;
    }
    if new {
        // Nothing of the user's is in a new file yet: its switch to WAL needn't sync either.
        conn.execute_batch("PRAGMA synchronous = OFF; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        rune::marks::mark("storage.wal");
        let (migrated, page2) = schema::migrate(&mut conn, (config.clock)())?;
        report.migrated = migrated;
        flags = if page2 { flags | FLAG_BOOT_PAGE2 } else { flags & !FLAG_BOOT_PAGE2 };
        schema::set_flags(&conn, flags)?;
    }
    report.schema_ns = lap(&mut at);
    // The early cache rows go before the import (long on a first launch), unless the boot
    // packet is still owed: the first frame waits on that, not on these.
    let mut sums = Vec::new();
    if send.is_none() {
        sums = offer_early(&conn, early, report, t0);
        at = Instant::now();
    }
    let deferred = match &config.import {
        Some(import) => {
            let path = config.path.clone();
            let scratch_parent = config.path.parent().map_or_else(|| import.dir.clone(), Path::to_path_buf);
            let reopen = move || connect(&path).and_then(|(c, _)| configure(&c, true).map(|()| c));
            let (summary, deferred, settled) = import::run(&mut conn, &reopen, import, &config.official, scratch_parent, flags, (config.clock)(), owe.seed.take());
            report.import = summary;
            if settled != flags {
                schema::set_flags(&conn, settled)?;
            }
            deferred
        }
        None => {
            // No import asked for: nothing of any group will ever be read.
            let settled = ImportGroup::ALL.iter().fold(flags | FLAG_SEED_SETTLED, |f, g| f | g.flag());
            if flags != settled {
                schema::set_flags(&conn, settled)?;
            }
            import::Deferred::default()
        }
    };
    report.import_ns = lap(&mut at);
    if let Some(send) = send {
        let t = Instant::now();
        // The raw read refused (the WAL holds frames after a crash, a new file, a torn record):
        // the record on file may be stale, so it's rebuilt from the tables first.
        // Unsynced (the first frame waits on it; the quit's or the next synced checkpoint makes it
        // durable): a sync on a busy disk took seconds here.
        conn.execute_batch("PRAGMA synchronous = OFF; PRAGMA wal_autocheckpoint = 0")?;
        let rebuilt = rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate).and_then(|tx| {
            boot_record::rebuild(&tx)?;
            tx.commit()
        });
        conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA wal_autocheckpoint = 1000")?;
        rebuilt?;
        let stored = boot_record::from_sqlite(&conn)?;
        let packet = stored.and_then(BootPacket::parse).map(|mut p| {
            p.source = BootSource::Sqlite;
            p.apply_journal(&owe.recovered);
            p
        });
        report.boot_source = packet.as_ref().map_or(BootSource::Empty, |p| p.source);
        report.boot_bytes = packet.as_ref().map_or(0, BootPacket::len);
        report.plugin_bytes = packet.as_ref().map_or(0, BootPacket::plugin_bytes);
        report.decode_ns = ns(t);
        report.stage_a_ns = ns(t0);
        send(packet.unwrap_or_default());
        sums = offer_early(&conn, early, report, t0);
        at = Instant::now();
    }
    // The drafts the journal kept that the database doesn't have yet go into it now (and so
    // into the cache read next).
    if journal::attached() && !owe.recovered.is_empty() {
        let t = Instant::now();
        conn.execute_batch("PRAGMA synchronous = OFF; PRAGMA wal_autocheckpoint = 0")?;
        report.journal_written = journal::reconcile(&conn, &std::mem::take(&mut owe.recovered), (config.clock)()).len();
        conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA wal_autocheckpoint = 1000")?;
        report.journal_ns += ns(t);
        // What it wrote is in the WAL, and a shown draft may be in the boot record: both are
        // the writer's to checkpoint (and rebuild).
        if report.journal_written > 0 && owe.journal == OpenJournal::Settled { owe.journal = OpenJournal::Reserved; }
    }
    let mut cache = read(&conn, report)?;
    // What was offered is what's stored: putting the same bytes back writes nothing.
    cache.cache_sums.extend(sums);
    cache.complete = report.created && report.import.rows == 0 && report.import.seeded_rows == 0;
    if cache.complete {
        cache.outbox = OutboxCache::Loaded(BTreeMap::new());
    }
    report.read_ns = lap(&mut at);
    Ok((conn, cache, deferred))
}

/// Reads each early cache row and offers its bytes, here on the open's thread; returns their
/// sums for the write-behind's unchanged-write check.
fn offer_early(conn: &Connection, early: &mut Vec<EarlyCache>, report: &mut BootReport, t0: Instant) -> Vec<(String, (i64, u64))> {
    use rusqlite::OptionalExtension;
    let mut sums = Vec::new();
    for e in early.drain(..) {
        let row: Option<Vec<u8>> = conn.query_row("SELECT data FROM cache WHERE key = ?1 AND version = ?2", rusqlite::params![e.key, e.version], |r| r.get(0)).optional().ok().flatten();
        let Some(bytes) = row else { continue };
        report.early_cache_bytes += bytes.len();
        sums.push((e.key.to_owned(), (e.version, twox_hash::XxHash3_64::oneshot(&bytes))));
        (e.offer)(Arc::from(bytes));
        report.early_cache_ns = ns(t0);
    }
    sums
}

/// Opens an existing database without the create probe; creates it only when it isn't there.
/// `PERSIST_WAL` is set before any page is read, so `-wal` and `-shm` are never deleted and
/// created again (a new file costs an antivirus scan on Windows).
fn connect(path: &Path) -> rusqlite::Result<(Connection, bool)> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_EXRESCODE;
    match Connection::open_with_flags(path, flags) {
        Ok(conn) => {
            sql::persist_wal(&conn);
            Ok((conn, false))
        }
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::CannotOpen => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).ok();
            }
            let conn = Connection::open_with_flags(path, flags | OpenFlags::SQLITE_OPEN_CREATE)?;
            sql::persist_wal(&conn);
            // 16 KiB pages: a commit writes whole pages to the WAL, so a keystroke's kill-safe
            // commit (`Storage::commit_soon`) costs 16 KiB, not 64 (4 KiB would be 4, but a 5 MB
            // attachment's commit then takes 25,754 µs against 12,319: `proof\storage-2\blob-page.txt`).
            // The boot record continues on overflow pages inside the raw read's first 128 KiB.
            conn.execute_batch("PRAGMA page_size = 16384")?;
            Ok((conn, true))
        }
        Err(e) => Err(e),
    }
}

/// The connection's own settings (none touch the file). `journal_mode = WAL` is stored in the
/// file and set once, with the schema.
///
/// `locking_mode = EXCLUSIVE` comes first, before any read: SQLite then keeps the WAL index in
/// heap memory and never maps or locks `-shm`, which measured 1,184 µs of the first read's
/// 2,340 µs (p50, warm, `proof\storage-1\floor-1.txt`). The process is the file's only user (the
/// app is single-instance); an outside reader waits until it quits.
/// The connection's settings. `wait`: a held lock is waited for (`busy_timeout`, 5 s); else the
/// caller set its own (a probe's zero) first, since these pragmas read the file.
fn configure(conn: &Connection, wait: bool) -> rusqlite::Result<()> {
    let busy = if wait { "PRAGMA busy_timeout = 5000;" } else { "" };
    conn.execute_batch(&format!(
        "{busy} PRAGMA locking_mode = EXCLUSIVE; PRAGMA synchronous = NORMAL; PRAGMA temp_store = MEMORY; PRAGMA cache_size = -8000; \
         PRAGMA wal_autocheckpoint = 1000; PRAGMA journal_size_limit = 0; PRAGMA mmap_size = {MMAP_SIZE};"
    ))
}

/// Stage B's read: every boot namespace's rows in full, the document keys, each window's
/// visible draft, every window frame, plugin override and trust, in one read transaction.
fn read(conn: &Connection, report: &mut BootReport) -> rusqlite::Result<Cache> {
    let tx = conn.unchecked_transaction()?;
    let mut cache = Cache::default();
    let mut rows: HashMap<String, BTreeMap<String, Arc<str>>> = HashMap::new();
    let mut recent: Vec<String> = Vec::new();
    {
        let mut stmt = tx.prepare_cached(
            crate::boot_state_query(),
        )?;
        let mut q = stmt.query([])?;
        while let Some(r) = q.next()? {
            let (name, key, value) = (r.get_ref(0)?.as_str()?, r.get_ref(1)?.as_str()?, r.get_ref(2)?.as_str()?);
            report.state_rows += 1;
            report.value_bytes += value.len();
            if key == "tabs.recent"
                && let Some(k) = serde_json::from_str::<serde_json::Value>(value).ok().and_then(|v| v["key"].as_str().map(str::to_owned))
            {
                recent.push(format!("{name}:draft:{k}"));
            }
            rows.entry(name.to_owned()).or_default().insert(key.to_owned(), value.into());
        }
    }
    {
        let mut stmt = tx.prepare_cached("SELECT key FROM document")?;
        let mut q = stmt.query([])?;
        while let Some(r) = q.next()? {
            cache.docs.insert(r.get(0)?, Doc::Stored);
            report.document_keys += 1;
        }
    }
    for key in recent {
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

//! The open, in two stages on the storage thread, as the product's critical path:
//!
//! - **Stage A** (above-normal priority): the boot record, the one row everything the first
//!   frame is built from lives in, read with one positioned file read and no SQLite at all when
//!   the file is current ([`boot_record::read_raw`]), from SQLite otherwise. Handed to the UI
//!   thread as a [`BootPacket`] the moment it's read.
//! - **Stage B** (below-normal priority): SQLite opens (no create probe on a profile that has a
//!   file; the schema and import are answered by one header field), the import runs once if the
//!   profile has old files, and the cache is hydrated with every boot namespace in full, the
//!   document keys, frames, plugin overrides and trust. The UI never waits for it to paint.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

use rusqlite::{Connection, OpenFlags};

use super::boot_record::{self, BootPacket, BootSource, FastBoot};
use super::import::{self, ImportSummary};
use super::schema::{self, FLAG_BITS, FLAG_BOOT_PAGE2, FLAG_MASK, FLAG_SEED_SETTLED, ImportGroup, SCHEMA_VERSION};
use super::{Backend, StorageConfig, sql, writer};
use crate::cache::{Cache, Doc, Namespace, OutboxCache};
use crate::{Storage, WindowFrame};

/// What the open did, each phase in nanoseconds on the monotonic clock (`Instant`).
#[derive(Clone, Debug, Default)]
pub struct BootReport {
    /// When the open started; subtract the process's own start for process-relative times.
    pub started: Option<Instant>,
    /// Stage A: open start to the packet sent, and its parts.
    pub stage_a_ns: u64,
    pub boot_source: BootSource,
    pub boot_bytes: usize,
    /// Of which the plugin values (`plugins::boot_slice`).
    pub plugin_bytes: usize,
    pub raw_read_ns: u64,
    pub decode_ns: u64,
    /// Why the raw read didn't serve (when it didn't).
    pub raw_miss: Option<&'static str>,
    /// The file didn't exist and was created.
    pub created: bool,
    /// The database couldn't open or migrate: this run keeps state in memory.
    pub error: Option<String>,
    /// Opens retried because another process held the file.
    pub busy_retries: u32,
    /// The early cache rows (`StorageConfig::early_cache`): read and offered, open start to the
    /// last offer, and their bytes.
    pub early_cache_ns: u64,
    pub early_cache_bytes: usize,
    pub open_ns: u64,
    pub pragma_ns: u64,
    pub schema_ns: u64,
    pub import_ns: u64,
    pub read_ns: u64,
    /// Open start to stage B done.
    pub total_ns: u64,
    pub state_rows: usize,
    pub value_bytes: usize,
    pub document_keys: usize,
    pub documents: usize,
    pub windows: usize,
    pub plugins: usize,
    pub migrated: Vec<&'static str>,
    pub import: ImportSummary,
}

const MMAP_SIZE: i64 = 64 * 1024 * 1024;
/// Opens tried while another process holds the file, each waiting `busy_timeout` (5 s).
const BUSY_TRIES: u32 = 6;

fn is_busy(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _) if matches!(f.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked))
}

fn ns(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// The open on the calling thread, both stages; the packet comes back with the storage.
pub(super) fn open(config: StorageConfig) -> (Storage, BootReport, BootPacket) {
    let (tx, rx) = mpsc::channel();
    // Nobody to wait for: the caller takes everything at the end.
    let gate = Gate { go: None, wanted: Arc::new(std::sync::atomic::AtomicBool::new(true)) };
    let (storage, report) = run(
        config,
        &|p| {
            tx.send(p).ok();
        },
        &gate,
    );
    (storage, report, rx.try_recv().unwrap_or_default())
}

/// A [`crate::Storage::open_async`] in progress.
pub struct Opening {
    boot: Receiver<BootPacket>,
    join: Option<std::thread::JoinHandle<(Storage, BootReport)>>,
    go: Option<mpsc::Sender<()>>,
    wanted: Arc<std::sync::atomic::AtomicBool>,
}

impl Opening {
    pub(super) fn start(config: StorageConfig) -> Opening {
        let (tx, boot) = mpsc::channel();
        let (go, gate) = match config.after_claim {
            true => {
                let (go, rx) = mpsc::channel();
                (Some(go), Gate { go: Some(rx), ..Gate::default() })
            }
            false => (None, Gate::default()),
        };
        let wanted = gate.wanted.clone();
        let on_packet = config.on_packet;
        let join = std::thread::Builder::new()
            .name("opencode-storage-open".into())
            .spawn(move || {
                priority(1);
                run(
                    config,
                    &|p| {
                        if let Some(sent) = on_packet {
                            sent();
                        }
                        tx.send(p).ok();
                        priority(-1);
                    },
                    &gate,
                )
            })
            .ok();
        Opening { boot, join, go, wanted }
    }

    /// Stage A's packet: blocks only until stage A is done (the record read), never for stage B,
    /// and never for another process's lock (then it's the last checkpoint's, `BootSource::held`).
    pub fn boot(&self) -> BootPacket {
        self.wanted.store(true, std::sync::atomic::Ordering::Release);
        self.boot.recv().unwrap_or_default()
    }

    /// This process is the app's one instance: SQLite may open the file and take its lock
    /// (`StorageConfig::after_claim`; the record's raw read ran already).
    pub fn proceed(&self) {
        if let Some(go) = &self.go {
            go.send(()).ok();
        }
    }

    /// The storage if stage B is done, without waiting: poll it after the first frame. Taken
    /// once; later calls (and [`Opening::storage`] after it) answer none / memory.
    pub fn try_storage(&mut self) -> Option<(Storage, BootReport)> {
        if !self.join.as_ref().is_some_and(std::thread::JoinHandle::is_finished) {
            return None;
        }
        self.join.take().and_then(|j| j.join().ok())
    }

    /// The storage once stage B is done. A failed thread gives storage in memory.
    pub fn storage(self) -> (Storage, BootReport) {
        self.join.and_then(|j| j.join().ok()).unwrap_or_else(|| (Storage::memory(), BootReport { error: Some("the open thread didn't run".into()), ..BootReport::default() }))
    }
}

/// The thread's priority: 1 above normal (stage A), -1 below normal (stage B).
#[allow(unsafe_code)]
fn priority(level: i32) {
    #[cfg(windows)]
    // SAFETY: GetCurrentThread returns a pseudo-handle valid for this call; the priority value is
    // one of the documented THREAD_PRIORITY_* constants.
    unsafe {
        windows_sys::Win32::System::Threading::SetThreadPriority(windows_sys::Win32::System::Threading::GetCurrentThread(), level);
    }
    #[cfg(not(windows))]
    drop(level);
}

fn run(mut config: StorageConfig, send: &dyn Fn(BootPacket), gate: &Gate) -> (Storage, BootReport) {
    let t0 = Instant::now();
    let mut early = std::mem::take(&mut config.early);
    let mut report = BootReport { started: Some(t0), ..BootReport::default() };
    let mut packet: Option<BootPacket> = None;
    // What the first frame shows if another process holds the file: the last checkpoint's
    // record, or a first-run screen when the file can't even be read.
    let mut held: Option<BootPacket> = None;
    match boot_record::read_raw(&config.path) {
        FastBoot::Record(bytes) => {
            report.raw_read_ns = ns(t0);
            let t = Instant::now();
            report.boot_bytes = bytes.len();
            match BootPacket::parse(bytes) {
                Some(mut p) => {
                    report.decode_ns = ns(t);
                    report.boot_source = BootSource::Raw;
                    report.plugin_bytes = p.plugin_bytes();
                    p.source = BootSource::Raw;
                    p.read_ns = report.raw_read_ns;
                    p.decode_ns = report.decode_ns;
                    report.stage_a_ns = ns(t0);
                    packet = Some(p.clone());
                    send(p);
                }
                None => report.raw_miss = Some("torn record"),
            }
        }
        FastBoot::Checkpointed(bytes) => {
            report.raw_miss = Some("the WAL holds frames");
            held = BootPacket::parse(bytes).map(|mut p| {
                p.source = BootSource::Checkpoint;
                p
            });
        }
        FastBoot::Denied => {
            report.raw_miss = Some("another process holds the file");
            held = Some(BootPacket::held_empty());
        }
        // A new profile with nothing to import or seed from: the first frame is a first run, so
        // it needn't wait for the database to be made (its schema takes ~13,000 µs).
        FastBoot::UseSqlite(why) if why == "no database file" && nothing_to_import(&config) => {
            report.raw_miss = Some(why);
            report.stage_a_ns = ns(t0);
            packet = Some(BootPacket::default());
            send(BootPacket::default());
        }
        FastBoot::UseSqlite(why) => report.raw_miss = Some(why),
    }
    // SQLite takes the file's lock: not before this process is the app's one instance.
    if !gate.wait() {
        return (Storage::memory(), report);
    }
    let mut result = match held.take() {
        Some(fallback) => held_open(&config, &mut report, t0, send, &mut early, gate, fallback, &mut packet),
        None => stage_b(&config, &mut report, t0, if packet.is_some() { None } else { Some(send) }, &mut early, Lock::Wait),
    };
    // Another process holds the file (an instance still exiting, a tool reading it): wait for it
    // a while longer (busy_timeout is 5 s a try) rather than give the windows a store that can't
    // keep what they write. The first frame is built already (from the record), so the windows
    // work meanwhile; without a packet they'd wait, so for less.
    let mut tries = 1;
    while tries < if packet.is_some() { BUSY_TRIES } else { 2 } && result.as_ref().is_err_and(|e| is_held(e, &config)) {
        tries += 1;
        report.busy_retries += 1;
        result = stage_b(&config, &mut report, t0, if packet.is_some() { None } else { Some(send) }, &mut early, Lock::Wait);
    }
    // The first frame showed the checkpoint (or a first run): the database the lock let go of is
    // used only if it still holds what the windows were built from.
    if let (Ok((conn, ..)), Some(p)) = (&result, &packet)
        && p.source.held()
        && !shows_the_same(conn, p)
    {
        result = Err(rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY), Some("the saved state changed while another process held it".into())));
    }
    report.total_ns = ns(t0);
    match result {
        Ok((conn, cache, deferred)) => {
            let cache = super::shared(cache);
            // A run that read the record through SQLite leaves the WAL to checkpoint at quit.
            let w = writer::Writer::new(conn, cache.clone(), config.clock, deferred, report.boot_source != BootSource::Raw);
            (Storage::from_parts(cache, Backend::parked(w)), report)
        }
        Err(e) => {
            report.error = Some(e.to_string());
            let mut cache = crate::cache::Cache { complete: true, outbox: OutboxCache::Loaded(BTreeMap::new()), ..Default::default() };
            match &packet {
                // The windows were built from the record: memory keeps what they show.
                Some(p) => p.fill_cache(&mut cache),
                None => {
                    // Never empty: with no database yet, the files the user had, read into memory
                    // (and left where they are). A database that exists is the real state: no
                    // import guesses over it.
                    if let Some(import) = config.import.as_ref().filter(|_| !config.path.exists()) {
                        import::hydrate_memory(&mut cache, import);
                    }
                    send(boot_record::from_cache(&cache));
                }
            }
            eprintln!("opencode-storage: the database couldn't open ({}); this run keeps its changes in memory", report.error.as_deref().unwrap_or_default());
            (Storage::from_parts(super::shared(cache), Backend::memory()), report)
        }
    }
}

/// The open while another process holds the file and the record on file may be behind (the WAL
/// holds frames) or can't be read: SQLite is tried without waiting on the lock until the UI asks
/// for its packet, and then the UI gets `fallback` at once and the open goes on behind it. The
/// first frame never waits for a lock.
#[allow(clippy::too_many_arguments)]
fn held_open(
    config: &StorageConfig,
    report: &mut BootReport,
    t0: Instant,
    send: &dyn Fn(BootPacket),
    early: &mut Vec<super::EarlyCache>,
    gate: &Gate,
    fallback: BootPacket,
    packet: &mut Option<BootPacket>,
) -> rusqlite::Result<(Connection, Cache, import::Deferred)> {
    loop {
        // A file this process can't read at all isn't probed: Windows' open retries a sharing
        // violation for about 1.4 s before it fails.
        if fallback.source == BootSource::Checkpoint {
            let result = stage_b(config, report, t0, Some(send), early, Lock::Probe);
            if result.as_ref().is_ok() || result.as_ref().is_err_and(|e| !is_held(e, config)) {
                return result;
            }
        }
        if gate.wanted() || t0.elapsed() > HELD_GRACE {
            report.boot_source = fallback.source;
            report.boot_bytes = fallback.len();
            report.stage_a_ns = ns(t0);
            *packet = Some(fallback.clone());
            send(fallback);
            return stage_b(config, report, t0, None, early, Lock::Wait);
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Whether the database, now open, would show the first frame `shown` was: its rebuilt record
/// has the same entries.
fn shows_the_same(conn: &Connection, shown: &BootPacket) -> bool {
    let fresh = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
        .and_then(|tx| {
            boot_record::rebuild(&tx)?;
            tx.commit()
        })
        .and_then(|()| boot_record::from_sqlite(conn));
    match fresh {
        Ok(Some(bytes)) => BootPacket::parse(bytes).is_some_and(|p| p.body() == shown.body()),
        _ => false,
    }
}

/// No legacy file to import and no Desktop profile to seed from (a profile's first launch).
fn nothing_to_import(config: &StorageConfig) -> bool {
    config.import.as_ref().is_none_or(|i| import::shapes::scan(i).is_empty()) && config.official.iter().all(|d| !d.join("drafts.sqlite").exists())
}

/// Another process has the file: SQLite's lock, or a handle that shares nothing.
fn is_held(e: &rusqlite::Error, config: &StorageConfig) -> bool {
    is_busy(e) || (matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::CannotOpen) && config.path.exists())
}

/// How stage B meets another process's lock: wait `busy_timeout`, or answer busy at once.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lock {
    Wait,
    Probe,
}

/// How long the held open probes before it shows the fallback without being asked (a caller
/// that never takes the packet).
const HELD_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Where the open waits: SQLite's open (the exclusive lock) for the single-instance claim
/// (`StorageConfig::after_claim`, [`Opening::proceed`]), and whether the UI is waiting for its
/// packet ([`Opening::boot`]).
#[derive(Default)]
pub(crate) struct Gate {
    go: Option<Receiver<()>>,
    wanted: Arc<std::sync::atomic::AtomicBool>,
}

impl Gate {
    /// False when the process went another way (it handed off to a running instance).
    fn wait(&self) -> bool {
        self.go.as_ref().is_none_or(|go| go.recv().is_ok())
    }

    fn wanted(&self) -> bool {
        self.wanted.load(std::sync::atomic::Ordering::Acquire)
    }
}
/// SQLite's part. `send` is set when stage A still owes the UI its packet: then the record is
/// read through SQLite as soon as the schema and import allow.
fn stage_b(config: &StorageConfig, report: &mut BootReport, t0: Instant, send: Option<&dyn Fn(BootPacket)>, early: &mut Vec<super::EarlyCache>, lock: Lock) -> rusqlite::Result<(Connection, Cache, import::Deferred)> {
    let mut at = Instant::now();
    let lap = |at: &mut Instant| {
        let n = ns(*at);
        *at = Instant::now();
        n
    };
    let (mut conn, created) = connect(&config.path)?;
    report.created = created;
    report.open_ns = lap(&mut at);
    // A probe answers busy at once: the pragmas already read the file (and so take its lock).
    if lock == Lock::Probe {
        conn.busy_timeout(std::time::Duration::ZERO)?;
    }
    configure(&conn, lock == Lock::Wait)?;
    report.pragma_ns = lap(&mut at);
    let version = schema::user_version(&conn)?;
    // The first read took the lock (exclusive mode keeps it): the rest may wait as usual.
    if lock == Lock::Probe {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
    }
    let mut flags = version & FLAG_MASK;
    if version >> FLAG_BITS != SCHEMA_VERSION {
        conn.execute_batch("PRAGMA journal_mode = WAL")?;
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
            let reopen = move || connect(&path).and_then(|(c, _)| configure(&c, true).map(|()| c));
            let (summary, deferred, settled) = import::run(&mut conn, &reopen, import, &config.official, flags, (config.clock)());
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
        let tx = rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)?;
        boot_record::rebuild(&tx)?;
        tx.commit()?;
        let stored = boot_record::from_sqlite(&conn)?;
        let packet = stored.and_then(BootPacket::parse).map(|mut p| {
            p.source = BootSource::Sqlite;
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
    let mut cache = read(&conn, report)?;
    // What was offered is what's stored: putting the same bytes back writes nothing.
    cache.cache_sums.extend(sums);
    cache.complete = report.created && report.import.rows == 0;
    if cache.complete {
        cache.outbox = OutboxCache::Loaded(BTreeMap::new());
    }
    report.read_ns = lap(&mut at);
    Ok((conn, cache, deferred))
}

/// Reads each early cache row and offers its bytes, here on the open's thread; returns their
/// sums for the write-behind's unchanged-write check.
fn offer_early(conn: &Connection, early: &mut Vec<super::EarlyCache>, report: &mut BootReport, t0: Instant) -> Vec<(String, (i64, u64))> {
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
pub(super) fn connect(path: &Path) -> rusqlite::Result<(Connection, bool)> {
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
            // 64 KiB pages hold the boot record whole on page 2 (`boot_record`).
            conn.execute_batch("PRAGMA page_size = 65536")?;
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
pub(super) fn configure(conn: &Connection, wait: bool) -> rusqlite::Result<()> {
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

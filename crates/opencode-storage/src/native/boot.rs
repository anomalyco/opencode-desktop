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

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

use rusqlite::Connection;

use super::boot_record::{self, BootPacket, BootSource, FastBoot};
use super::import::{self, ImportSummary};
use super::writer::OpenJournal;
use super::{Backend, StorageConfig, journal, writer};

mod damaged;
mod owed;
mod profile;
mod sqlite;
use damaged::{adopt_shown, damaged, keep_damaged};
use owed::{Owe, attach_journal, owned_journal};
use sqlite::{Lock, stage_b};
use crate::cache::{Cache, OutboxCache};
use crate::Storage;

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
    /// The draft journal: mapping, reserving and reading it, ns.
    pub journal_ns: u64,
    /// It was reserved by this open (a new profile, the first launch after the update).
    pub journal_reserved: bool,
    /// Drafts it held at launch, and of them those newer than the database, written to it.
    pub journal_recovered: usize,
    pub journal_written: usize,
    /// True when an existing draft journal from the previous run attached and replayed cleanly.
    pub journal_replayed: bool,
    /// Why this run has no journal, when it hasn't.
    pub journal_error: Option<String>,
    /// The file couldn't be read (damaged: a power cut, a disk error) and was moved here; this
    /// run started a new one beside it.
    pub damaged: Option<std::path::PathBuf>,
}

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
        if journal::SUPPORTED {
            journal::pending();
        }
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
pub(super) fn priority(level: i32) {
    #[cfg(windows)]
    // SAFETY: GetCurrentThread returns a pseudo-handle valid for this call; the priority value is
    // one of the documented THREAD_PRIORITY_* constants.
    unsafe {
        windows_sys::Win32::System::Threading::SetThreadPriority(windows_sys::Win32::System::Threading::GetCurrentThread(), level);
    }
    #[cfg(not(windows))]
    let _ = level;
}

fn run(mut config: StorageConfig, send: &dyn Fn(BootPacket), gate: &Gate) -> (Storage, BootReport) {
    let t0 = Instant::now();
    let mut early = std::mem::take(&mut config.early);
    let mut report = BootReport { started: Some(t0), ..BootReport::default() };
    let mut packet: Option<BootPacket> = None;
    // What the first frame shows if another process holds the file: the last checkpoint's
    // record, or a first-run screen when the file can't even be read.
    let mut held: Option<BootPacket> = None;
    let mut owe = Owe { want_journal: true, ..Owe::default() };
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
                    owe.hint = owned_journal(&p);
                    // A record that names a journal waits for it (after the claim): what it holds
                    // may be newer than the record, and the first frame shows the newer. One that
                    // names none (the first launch after the update) has nothing to wait for: the
                    // journal is reserved beside the first frame, before the first key.
                    // A relaunch's read waits too: its old copy may still save (`profile::hold`).
                    if owe.hint.is_some() || profile::relaunch() {
                        owe.packet = Some(p);
                    } else {
                        packet = Some(p.clone());
                        send(p);
                    }
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
            owe.hint = held.as_ref().and_then(owned_journal);
        }
        FastBoot::Denied => {
            report.raw_miss = Some("another process holds the file");
            held = Some(BootPacket::held_empty());
        }
        // A new profile with nothing to import or seed from: the first frame is a first run, so
        // it needn't wait for the database to be made (its schema takes ~13,000 µs). The journal
        // is reserved beside it; keys before it are held for it (`journal::hold`).
        FastBoot::UseSqlite(why) if why == "no database file" && nothing_to_import(&config) => {
            report.raw_miss = Some(why);
            report.stage_a_ns = ns(t0);
            packet = Some(BootPacket::default());
            send(BootPacket::default());
        }
        FastBoot::UseSqlite(why) => report.raw_miss = Some(why),
    }
    // SQLite takes the file's lock, and the journal is written: not before this process is the
    // app's one instance.
    if !gate.wait() {
        if let Some(p) = owe.packet.take() {
            send(p);
        }
        journal::decided();
        return (Storage::memory(), report);
    }
    // Nor before it holds the profile's lease (`profile`): on macOS the claim admits every launch.
    match profile::hold(&config, &mut report, t0, send, &mut owe, &mut packet, &mut held) {
        profile::Hold::Go => {}
        profile::Hold::Memory(why) => return profile::in_memory(&mut report, t0, send, &mut owe, packet, held, why),
    }
    // The record names the journal's place: mapped here, so the windows get their packet without
    // SQLite in the way (another process may hold its lock). A place that doesn't hold it any
    // more: the packet goes as it is, and stage B reserves the journal again.
    if let Some(mut p) = owe.packet.take() {
        if journal::SUPPORTED
            && p.source == BootSource::Raw
            && let Some(loc) = owe.hint.take()
            && attach_journal(&config, &mut report, &mut owe, &loc, false)
        {
            p.apply_journal(&owe.recovered);
            journal::decided();
        }
        report.stage_a_ns = ns(t0);
        packet = Some(p.clone());
        send(p);
    }
    let mut result = match held.take() {
        Some(fallback) => held_open(&config, &mut report, t0, send, &mut owe, &mut early, gate, fallback, &mut packet),
        None => stage_b(&config, &mut report, t0, if packet.is_some() { None } else { Some(send) }, &mut owe, &mut early, Lock::Wait),
    };
    // The packet waited on stage B for its journal: the windows have it now (or, when stage B
    // couldn't open the file, get it without one).
    if let Some(p) = owe.sent.take().or_else(|| owe.packet.take().inspect(|p| send(p.clone()))) {
        packet = Some(p);
    }
    // Another process holds the file (an instance still exiting, a tool reading it): wait for it
    // a while longer (busy_timeout is 5 s a try) rather than give the windows a store that can't
    // keep what they write. The first frame is built already (from the record), so the windows
    // work meanwhile; without a packet they'd wait, so for less.
    let mut tries = 1;
    while tries < if packet.is_some() { BUSY_TRIES } else { 2 } && result.as_ref().is_err_and(|e| is_held(e, &config)) {
        tries += 1;
        report.busy_retries += 1;
        result = stage_b(&config, &mut report, t0, if packet.is_some() { None } else { Some(send) }, &mut owe, &mut early, Lock::Wait);
        if let Some(p) = owe.sent.take() {
            packet = Some(p);
        }
    }
    // A file SQLite can't read (a power cut or a disk error damaged it): moved aside, once, and a
    // new one made in its place. The windows are told where the old one is. The moved file is
    // never opened again, so a launch can't meet it twice.
    if result.as_ref().is_err_and(|e| !is_held(e, &config)) && damaged(&config.path) {
        journal::abandon();
        report.journal_replayed = false;
        match keep_damaged(&config.path, (config.clock)()) {
            Ok(kept) => {
                eprintln!("opencode-storage: the database was damaged; kept as {} and started a new one", kept.display());
                report.damaged = Some(kept);
                result = stage_b(&config, &mut report, t0, if packet.is_some() { None } else { Some(send) }, &mut owe, &mut early, Lock::Wait);
                if let Some(p) = owe.sent.take() {
                    packet = Some(p);
                }
                // The windows show what the damaged file's boot record held (it was still
                // readable): that much goes into the new file, so the next launch shows it too.
                if let (Ok((conn, cache, _)), Some(p)) = (&mut result, &packet)
                    && p.source == BootSource::Raw
                {
                    match adopt_shown(conn, p, (config.clock)()) {
                        Ok(()) => p.fill_cache(cache),
                        Err(e) => eprintln!("opencode-storage: couldn't keep what the damaged file showed ({e})"),
                    }
                }
            }
            Err(e) => eprintln!("opencode-storage: the database is damaged and couldn't be moved aside ({e})"),
        }
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
    if owe.journal != OpenJournal::Owed || result.is_err() { journal::decided(); }
    match result {
        Ok((conn, cache, deferred)) => {
            let cache = super::shared(cache);
            let wal_dirty = report.boot_source != BootSource::Raw
                || boot_record::wal_len(&std::path::PathBuf::from(format!("{}-wal", config.path.display()))) > 0;
            let w = writer::Writer::new(conn, cache.clone(), config.clock, deferred, wal_dirty, owe.journal, report.created);
            (Storage::from_parts(cache, Backend::parked(w)), report)
        }
        Err(e) => {
            report.error = Some(e.to_string());
            report.journal_replayed = false;
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
    owe: &mut Owe,
    early: &mut Vec<super::EarlyCache>,
    gate: &Gate,
    fallback: BootPacket,
    packet: &mut Option<BootPacket>,
) -> rusqlite::Result<(Connection, Cache, import::Deferred)> {
    loop {
        // A file this process can't read at all isn't probed: Windows' open retries a sharing
        // violation for about 1.4 s before it fails.
        if fallback.source == BootSource::Checkpoint {
            let result = stage_b(config, report, t0, Some(send), owe, early, Lock::Probe);
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
            // Nothing this run changes is saved: no journal either.
            owe.want_journal = false;
            journal::decided();
            return stage_b(config, report, t0, None, owe, early, Lock::Wait);
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
        Ok(Some(bytes)) => BootPacket::parse(bytes).is_some_and(|p| p.shown_entries().eq(shown.shown_entries())),
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

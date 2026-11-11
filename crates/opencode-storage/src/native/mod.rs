//! The native backend: one SQLite database (`rusqlite`, bundled SQLite), opened once per process.
//! [`crate::Storage::open`] opens it on the caller's thread and reads what the first frame
//! draws; the connection then waits, untouched, until the first write starts the
//! `opencode-storage` thread, which owns it from then on.

mod boot;
pub mod boot_record;
pub mod import;
mod schema;
pub mod seed;
mod sql;
mod writer;

use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use boot::{BootReport, Opening};
pub use boot_record::{BootPacket, BootSource};
pub use import::{ImportConfig, ImportGroup};
pub use schema::SCHEMA_VERSION;

use crate::{Cmd, FlushReason, FlushReport, Loaded, Storage};

/// Where the database is and what the open needs. Only this type names paths.
pub struct StorageConfig {
    /// `opencode.sqlite`, in the app's data folder.
    pub(crate) path: PathBuf,
    /// The folder the files Rune kept before the database live in (`%APPDATA%\rune-opencode`);
    /// none skips the import.
    pub(crate) import: Option<ImportConfig>,
    /// Desktop's `userData` folders to seed a fresh profile from, newest data wins.
    pub(crate) official: Vec<PathBuf>,
    /// Wall-clock milliseconds since the epoch, for `updated_at` (injected: the workspace forbids
    /// `SystemTime::now` outside its config).
    pub(crate) clock: fn() -> i64,
}

impl StorageConfig {
    pub fn native(path: impl Into<PathBuf>, clock: fn() -> i64) -> StorageConfig {
        StorageConfig { path: path.into(), import: None, official: Vec::new(), clock }
    }

    /// Imports the files Rune kept before the database, once, from `import.dir`.
    pub fn import(mut self, import: ImportConfig) -> StorageConfig {
        self.import = Some(import);
        self
    }

    /// Desktop's `userData` folders (`ai.opencode.desktop`, `.beta`, `.dev`) to seed a fresh
    /// profile from after the first frame. Opened read-only; never written.
    pub fn official(mut self, dirs: Vec<PathBuf>) -> StorageConfig {
        self.official = dirs;
        self
    }
}

impl Storage {
    /// Opens the database on the calling thread, both stages. Never fails: a database that
    /// can't open or migrate leaves state in memory for this run ([`BootReport::error`]).
    pub fn open(config: StorageConfig) -> (Storage, BootReport) {
        let (storage, report, _) = boot::open(config);
        (storage, report)
    }

    /// [`Storage::open`] with the boot packet stage A read.
    pub fn open_with_boot(config: StorageConfig) -> (Storage, BootReport, BootPacket) {
        boot::open(config)
    }

    /// The open on its own thread, started at once: call it first thing in `main`, take
    /// [`Opening::boot`] where the first frame is built and [`Opening::storage`] after it.
    pub fn open_async(config: StorageConfig) -> Opening {
        Opening::start(config)
    }
}
/// The writer thread's handle, as the backend sees it.
pub(crate) enum Writer {
    /// Opened and read; no write yet, so no thread.
    Parked(Box<writer::Writer>),
    Running { tx: Sender<writer::Msg>, join: std::thread::JoinHandle<()> },
    /// In memory only, or closed.
    Off,
}

pub(crate) struct Backend {
    writer: Mutex<Writer>,
}

impl Backend {
    pub(crate) fn memory() -> Backend {
        Backend { writer: Mutex::new(Writer::Off) }
    }

    pub(crate) fn parked(w: writer::Writer) -> Backend {
        Backend { writer: Mutex::new(Writer::Parked(Box::new(w))) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Writer> {
        self.writer.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The running writer's sender, starting the thread on first use.
    fn sender(&self) -> Option<Sender<writer::Msg>> {
        let mut w = self.lock();
        match std::mem::replace(&mut *w, Writer::Off) {
            Writer::Parked(parked) => {
                let (tx, rx) = mpsc::channel();
                let spawned = std::thread::Builder::new().name("opencode-storage".into()).spawn(move || parked.run(rx));
                match spawned {
                    Ok(join) => {
                        *w = Writer::Running { tx: tx.clone(), join };
                        Some(tx)
                    }
                    Err(_) => None,
                }
            }
            Writer::Running { tx, join } => {
                *w = Writer::Running { tx: tx.clone(), join };
                Some(tx)
            }
            Writer::Off => None,
        }
    }

    pub(crate) fn send(&self, cmd: Cmd) {
        match self.sender() {
            Some(tx) => {
                tx.send(writer::Msg::Cmd(cmd)).ok();
            }
            None => answer_offline(cmd),
        }
    }

    pub(crate) fn flush(&self, reason: FlushReason, deadline: Duration) -> FlushReport {
        let started = Instant::now();
        // Nothing was ever written (and the WAL holds nothing to checkpoint): nothing to land,
        // check or sync, and no thread is started for it.
        let quit = matches!(reason, FlushReason::Quit | FlushReason::SessionEnd | FlushReason::UpdateRestart);
        let parked_work = match &*self.lock() {
            Writer::Running { .. } => true,
            Writer::Parked(w) => quit && w.wal_dirty(),
            Writer::Off => false,
        };
        if !parked_work {
            return FlushReport { clean: true, ..FlushReport::default() };
        }
        let Some(tx) = self.sender() else { return FlushReport { clean: true, ..FlushReport::default() } };
        let (reply, answer) = mpsc::sync_channel(1);
        if tx.send(writer::Msg::Flush { reason, started, reply }).is_err() {
            return FlushReport { clean: true, ..FlushReport::default() };
        }
        answer.recv_timeout(deadline).unwrap_or(FlushReport { timed_out: true, micros: micros(started), ..FlushReport::default() })
    }

    pub(crate) fn first_frame(&self, done: Option<Loaded>) {
        // The deferred import or seed needs the writer; a settled profile starts nothing.
        let deferred = match &*self.lock() {
            Writer::Parked(w) => w.has_deferred(),
            Writer::Running { .. } => true,
            Writer::Off => false,
        };
        if !deferred {
            return;
        }
        if let Some(tx) = self.sender() {
            tx.send(writer::Msg::FirstFrame { done }).ok();
        }
    }

    pub(crate) fn close(&self) -> FlushReport {
        let report = self.flush(FlushReason::Quit, Duration::from_secs(2));
        let w = std::mem::replace(&mut *self.lock(), Writer::Off);
        if let Writer::Running { tx, join } = w {
            tx.send(writer::Msg::Stop).ok();
            join.join().ok();
        }
        report
    }
}

/// A command with no database behind it: callbacks still run, so a caller waiting on one never hangs.
fn answer_offline(cmd: Cmd) {
    match cmd {
        Cmd::Plugin { done: Some(done), .. } | Cmd::Trust { done: Some(done), .. } | Cmd::Outbox { done: Some(done), .. } => done(Ok(())),
        Cmd::LoadState { done: Some(done), .. } | Cmd::LoadDocs { done: Some(done), .. } | Cmd::LoadOutbox { done: Some(done) } => done(),
        Cmd::GetBlob { done, .. } => done(None),
        Cmd::GetPreview { done, .. } => done(None),
        Cmd::GetCache { done, .. } => done(None),
        _ => {}
    }
}

pub(crate) fn micros(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_micros()).unwrap_or(u64::MAX)
}

pub(crate) fn shared(cache: crate::cache::Cache) -> Arc<Mutex<crate::cache::Cache>> {
    Arc::new(Mutex::new(cache))
}

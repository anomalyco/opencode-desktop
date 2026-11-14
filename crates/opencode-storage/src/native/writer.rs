//! The `opencode-storage` thread: the only owner of the read-write connection once the first
//! write starts it. It drives the write-behind buffers (state and window frames at 250 ms,
//! documents and session previews at 500 ms, caches at 1 s), commits Tier 0 rows at once with
//! `synchronous = FULL`, keeps the boot record current in the same transaction as what it
//! summarizes, sweeps blobs, checkpoints the WAL when writes go quiet, and loads lazy rows.
//!
//! Every flush lands all buffers in one `BEGIN IMMEDIATE … COMMIT`.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use rune::machine::{Attempt, Effects, Seq};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};

use super::import::{self, Deferred};
use super::{boot_record, sql};
use crate::cache::{Cache, Doc, OutboxCache};
use crate::machine::checkpoint::{self, WalEffect, WalEvent, WalState};
use crate::machine::draft_gc::{self, DraftGcEffect, DraftGcEvent, DraftGcState, STARTUP_COLLECT_DELAY_MS};
use crate::machine::write_behind::{self as wb, WriteBehindEffect, WriteBehindEvent, WriteBehindState};
use crate::outbox;
use crate::{Cmd, FlushReason, FlushReport, Loaded, StorageError, WindowFrame};

mod rows;
use rows::{apply, cmd_name, note, settled, slow};

pub(crate) enum Msg {
    /// Commands are in the queue (`super::Queue`).
    Wake,
    Flush { reason: FlushReason, started: Instant, reply: SyncSender<FlushReport> },
    FirstFrame { done: Option<Loaded> },
    Stop,
}

type StateKey = (String, String);
type Value = Option<Arc<str>>;
/// A document's text and, for a draft the journal recorded, its revision (0: none).
type DocValue = (Value, u64);
type PreviewKey = (String, String);
/// A preview's `updated_at` and bytes.
pub(crate) type Preview = (i64, Arc<[u8]>);
/// A cache entry's version and bytes.
pub(crate) type CacheEntry = (i64, Arc<[u8]>);

/// A write batch a buffer's machine asked for, tagged with the buffer.
enum Batch {
    State(Attempt, IndexMap<StateKey, Value>),
    Docs(Attempt, IndexMap<String, DocValue>),
    Windows(Attempt, IndexMap<String, Option<WindowFrame>>),
    Previews(Attempt, IndexMap<PreviewKey, Option<Preview>>),
    Caches(Attempt, IndexMap<String, Option<CacheEntry>>),
}

impl Batch {
    fn len(&self) -> usize {
        match self {
            Batch::State(_, m) => m.len(),
            Batch::Docs(_, m) => m.len(),
            Batch::Windows(_, m) => m.len(),
            Batch::Previews(_, m) => m.len(),
            Batch::Caches(_, m) => m.len(),
        }
    }

    /// Whether it changes what the boot record holds.
    fn touches_boot(&self) -> bool {
        match self {
            Batch::State(_, m) => m.keys().any(|(name, key)| crate::in_boot_record(name, key)),
            Batch::Docs(_, m) => m.keys().any(|k| k.contains(":draft:")),
            Batch::Windows(..) | Batch::Previews(..) => true,
            Batch::Caches(..) => false,
        }
    }
}

/// The page size a file is rewritten to (`Writer::repage`), and the largest file rewritten.
const REPAGE_TO: i64 = 16384;
const REPAGE_MAX: i64 = 64 * 1024 * 1024;
/// An unsynced commit (`commit_soon`, an outbox row): SQLite's checkpoint off for it, as it would
/// run at OFF (`Writer::bound_unsynced`); and back.
const UNSYNCED: &str = "PRAGMA synchronous = OFF; PRAGMA wal_autocheckpoint = 0";
const SYNCED: &str = "PRAGMA synchronous = NORMAL; PRAGMA wal_autocheckpoint = 1000";
/// The longest unsynced commits pile up, and the most WAL they fill, before a checkpoint syncs them.
const UNSYNCED_FOR: Duration = Duration::from_secs(30);
const UNSYNCED_BYTES: u64 = 16 * 1024 * 1024;
const STATE_MS: u64 = 250;
const DOCS_MS: u64 = 500;
const WINDOWS_MS: u64 = 250;
const PREVIEWS_MS: u64 = 500;
const CACHES_MS: u64 = 1000;

/// A timer a machine armed: its attempt and when it's due.
type Armed = Option<(Attempt, Instant)>;

pub(crate) struct Writer {
    conn: Connection,
    /// A `Cmd::CommitSoon` came: commit the buffered rows once the queue is drained.
    commit_due: bool,
    /// The WAL, opened once for `sync_wal` and `bound_unsynced`.
    wal_file: Option<std::fs::File>,
    /// Since when the WAL holds commits nothing has synced (`bound_unsynced`).
    unsynced_since: Option<Instant>,
    cache: Arc<Mutex<Cache>>,
    search: super::search::State,
    clock: fn() -> i64,
    seq: Seq,
    state: WriteBehindState<StateKey, Value>,
    docs: WriteBehindState<String, DocValue>,
    windows: WriteBehindState<String, Option<WindowFrame>>,
    previews: WriteBehindState<PreviewKey, Option<Preview>>,
    caches: WriteBehindState<String, Option<CacheEntry>>,
    gc: DraftGcState,
    wal: WalState,
    timers: HashMap<&'static str, Armed>,
    deferred: Deferred,
    /// When this run last touched each chunk, so an unchanged chunk of a long draft isn't
    /// rewritten on every save (a blob's touch rewrites its whole record).
    touched: HashMap<String, i64>,
    /// Rows written since the last flush, for its report.
    written: usize,
}

/// Steps one buffer's machine: timers and reports are carried out here, write batches returned.
#[allow(clippy::too_many_arguments)]
fn step_buffer<K: Hash + Eq + Clone, V: Clone>(seq: &mut Seq, machine: &mut WriteBehindState<K, V>, event: WriteBehindEvent<K, V>, delay: u64, timer: &'static str, timers: &mut HashMap<&'static str, Armed>) -> Vec<(Attempt, IndexMap<K, V>)> {
    let mut out = Vec::new();
    for effect in wb::step(seq, machine, event, delay) {
        match effect {
            WriteBehindEffect::ArmTimer { attempt, delay_ms } => {
                timers.insert(timer, Some((attempt, Instant::now() + Duration::from_millis(delay_ms))));
            }
            WriteBehindEffect::CancelTimer => {
                timers.insert(timer, None);
            }
            WriteBehindEffect::WriteBatch { attempt, batch } => out.push((attempt, batch)),
            WriteBehindEffect::ReportError(e) => eprintln!("opencode-storage: {timer} write failed, retrying: {e}"),
            WriteBehindEffect::RejectClosed => {}
        }
    }
    out
}

impl Writer {
    /// `wal_dirty`: the WAL holds frames to checkpoint; `boot_stale`: the boot record is behind
    /// the tables (the open reserved the draft journal, whose place the record names).
    pub(crate) fn new(conn: Connection, cache: Arc<Mutex<Cache>>, clock: fn() -> i64, deferred: Deferred, wal_dirty: bool, boot_stale: bool) -> Writer {
        let mut seq = Seq::default();
        let wal = if wal_dirty || boot_stale { WalState::Dirty { attempt: seq.next(), boot_stale } } else { WalState::Clean };
        Writer {
            conn,
            cache,
            search: super::search::State::BeforeFrame,
            clock,
            seq,
            state: WriteBehindState::Idle,
            docs: WriteBehindState::Idle,
            windows: WriteBehindState::Idle,
            previews: WriteBehindState::Idle,
            caches: WriteBehindState::Idle,
            gc: DraftGcState::Closed,
            wal,
            timers: HashMap::new(),
            deferred,
            touched: HashMap::new(),
            written: 0,
            commit_due: false,
            wal_file: None,
            unsynced_since: None,
        }
    }

    /// The WAL holds frames to checkpoint (quit has work even with nothing queued).
    pub(crate) fn wal_dirty(&self) -> bool {
        self.wal != WalState::Clean
    }

    pub(crate) fn run(mut self, rx: Receiver<Msg>, queue: super::SharedQueue, urgent: Arc<std::sync::atomic::AtomicU64>) {
        let (gc, effects) = draft_gc::initial(&mut self.seq, (self.clock)(), STARTUP_COLLECT_DELAY_MS);
        self.gc = gc;
        self.gc_effects(effects);
        loop {
            // Take what's queued, then say whether a buffer's timer will wake this thread: until
            // it does, coalesced writes are queued without a wake.
            loop {
                let armed = self.buffer_armed();
                let cmds = {
                    let mut q = super::lock_queue(&queue);
                    q.armed = armed;
                    q.commit_queued = false;
                    std::mem::take(&mut q.cmds)
                };
                if cmds.is_empty() {
                    break;
                }
                for cmd in cmds {
                    let (what, t) = (cmd_name(&cmd), Instant::now());
                    self.handle(cmd);
                    slow(what, t);
                }
                let t = Instant::now();
                self.commit_if_due();
                slow("the kill-safe commit", t);
            }
            let admitted = urgent.load(std::sync::atomic::Ordering::Acquire);
            let msg = match rx.try_recv() {
                Ok(m) => Ok(m),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(RecvTimeoutError::Disconnected),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    let query = {
                        let mut q = super::lock_queue(&queue);
                        if !q.cmds.is_empty() { continue }
                        if matches!(self.search, super::search::State::BeforeFrame) { None } else { q.queries.pop() }
                    };
                    if let Some(query) = query {
                        match &self.search {
                            super::search::State::Ready(_) => if let super::search::query::Completion::Yield(q) = query.run(&mut self.conn, &urgent, admitted) { super::lock_queue(&queue).queries.replace(q) },
                            super::search::State::Unavailable(why) => query.unavailable(why.clone()),
                            super::search::State::BeforeFrame => unreachable!(),
                        }
                        self.fire_due();
                        continue;
                    }
                    if matches!(self.search, super::search::State::Ready(super::search::Work::Pending)) {
                        use super::search::index::Step;
                        // Derived data: unsynced like every commit here (`commit`).
                        self.conn.execute_batch(UNSYNCED).ok();
                        let stepped = super::search::index::step(&mut self.conn, &urgent);
                        self.conn.execute_batch(SYNCED).ok();
                        if matches!(stepped, Ok(Step::Done { .. })) {
                            self.bound_unsynced();
                        }
                        match stepped {
                            Ok(Step::Done { key, work_us, commit_us }) => {
                                if work_us + commit_us >= 250_000 {
                                    eprintln!("opencode-storage: indexing {key:?} for search took {work_us} us, its commit {commit_us} us");
                                }
                                self.wal_event(WalEvent::Committed { boot: false });
                                self.fire_due();
                                continue;
                            }
                            Ok(Step::Yielded) => continue,
                            Ok(Step::Empty) => self.search = super::search::State::Ready(super::search::Work::Drained),
                            Err(e) => self.search = super::search::State::Unavailable(e.to_string()),
                        }
                    }
                    match self.timers.values().filter_map(|t| t.map(|(_, due)| due)).min() {
                Some(due) => rx.recv_timeout(due.saturating_duration_since(Instant::now())),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                    }
                }
            };
            // Commands made before the message (a flush, the first frame, a stop) come first.
            let cmds = {
                let mut q = super::lock_queue(&queue);
                q.commit_queued = false;
                std::mem::take(&mut q.cmds)
            };
            for cmd in cmds {
                self.handle(cmd);
            }
            self.commit_if_due();
            match msg {
                Ok(Msg::Wake) => {}
                Ok(Msg::Flush { reason, started, reply }) => {
                    rune::marks::mark("writer.flush");
                    let report = self.flush(reason, started);
                    reply.send(report).ok();
                    rune::marks::mark("writer.flushed");
                    self.quit_checkpoint(reason);
                    rune::marks::mark("writer.checkpointed");
                }
                Ok(Msg::FirstFrame { done }) => {
                    let t = Instant::now();
                    self.first_frame(done);
                    slow("the work that waited for the first frame", t);
                }
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => {
                    rune::marks::mark("writer.stop");
                    self.flush(FlushReason::Quit, Instant::now());
                    self.quit_checkpoint(FlushReason::Quit);
                    self.repage();
                    self.close_machines();
                    rune::marks::mark("writer.stopped");
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.fire_due();
        }
    }

    /// The write-behind buffers whose timer is armed (it wakes this thread, which takes the queue).
    fn buffer_armed(&self) -> u8 {
        [("state", crate::BUFFER_STATE), ("docs", crate::BUFFER_DOCS), ("windows", crate::BUFFER_WINDOWS), ("previews", crate::BUFFER_PREVIEWS), ("caches", crate::BUFFER_CACHES)]
            .iter()
            .filter(|(n, _)| self.timers.get(n).is_some_and(Option::is_some))
            .fold(0, |m, (_, b)| m | b)
    }

    fn fire_due(&mut self) {
        let now = Instant::now();
        let due: Vec<(&'static str, Attempt)> = self.timers.iter().filter_map(|(n, t)| t.filter(|(_, d)| *d <= now).map(|(a, _)| (*n, a))).collect();
        for (name, attempt) in due {
            let t = Instant::now();
            self.fire(name, attempt);
            slow(name, t);
        }
    }

    fn fire(&mut self, name: &'static str, attempt: Attempt) {
        self.timers.insert(name, None);
        match name {
            "state" => self.state_event(WriteBehindEvent::TimerFired { attempt }),
            "docs" => self.docs_event(WriteBehindEvent::TimerFired { attempt }),
            "windows" => self.windows_event(WriteBehindEvent::TimerFired { attempt }),
            "previews" => self.previews_event(WriteBehindEvent::TimerFired { attempt }),
            "caches" => self.caches_event(WriteBehindEvent::TimerFired { attempt }),
            "gc" => {
                let effects = draft_gc::step(&mut self.seq, &mut self.gc, DraftGcEvent::TimerFired { attempt, at_ms: (self.clock)() });
                self.gc_effects(effects);
            }
            _ => self.wal_event(WalEvent::TimerFired { attempt }),
        }
    }

    fn state_event(&mut self, event: WriteBehindEvent<StateKey, Value>) {
        let out = step_buffer(&mut self.seq, &mut self.state, event, STATE_MS, "state", &mut self.timers);
        self.commit(out.into_iter().map(|(a, m)| Batch::State(a, m)).collect());
    }

    fn docs_event(&mut self, event: WriteBehindEvent<String, DocValue>) {
        let out = step_buffer(&mut self.seq, &mut self.docs, event, DOCS_MS, "docs", &mut self.timers);
        self.commit(out.into_iter().map(|(a, m)| Batch::Docs(a, m)).collect());
    }

    fn windows_event(&mut self, event: WriteBehindEvent<String, Option<WindowFrame>>) {
        let out = step_buffer(&mut self.seq, &mut self.windows, event, WINDOWS_MS, "windows", &mut self.timers);
        self.commit(out.into_iter().map(|(a, m)| Batch::Windows(a, m)).collect());
    }

    fn previews_event(&mut self, event: WriteBehindEvent<PreviewKey, Option<Preview>>) {
        let out = step_buffer(&mut self.seq, &mut self.previews, event, PREVIEWS_MS, "previews", &mut self.timers);
        self.commit(out.into_iter().map(|(a, m)| Batch::Previews(a, m)).collect());
    }

    fn caches_event(&mut self, event: WriteBehindEvent<String, Option<CacheEntry>>) {
        let out = step_buffer(&mut self.seq, &mut self.caches, event, CACHES_MS, "caches", &mut self.timers);
        self.commit(out.into_iter().map(|(a, m)| Batch::Caches(a, m)).collect());
    }

    /// Writes `batches` in one transaction, the boot record rebuilt in it when they touch it,
    /// then tells each buffer's machine how its batch went.
    fn commit(&mut self, batches: Vec<Batch>) {
        if batches.is_empty() {
            return;
        }
        let now = (self.clock)();
        let boot = batches.iter().any(Batch::touches_boot);
        let docs = batches.iter().any(|b| matches!(b, Batch::Docs(..)));
        let changed_search = batches.iter().any(|b| match b { Batch::Docs(_, rows) => rows.keys().any(|k| k.contains(".dat:draft:")), Batch::Caches(_, rows) => rows.keys().any(|k| k.starts_with("rune.copy\n")), _=>false });
        let mut fresh: Vec<String> = Vec::new();
        let revs: Vec<(String, u64)> = batches
            .iter()
            .flat_map(|b| match b {
                Batch::Docs(_, rows) => rows.iter().filter(|(_, (_, rev))| *rev > 0).map(|(k, (_, rev))| (k.clone(), *rev)).collect(),
                _ => Vec::new(),
            })
            .collect();
        // Every write-behind commit is unsynced, as the kill-safe tier's: at NORMAL the first
        // commit after a checkpoint syncs the WAL's new header, and on a busy disk that one sync
        // held the writer (and every commit behind it) for seconds. The synced checkpoint every
        // 30 s / 16 MB (`bound_unsynced`), when writes go quiet and at quit bounds a power cut's
        // loss as before.
        self.conn.execute_batch(UNSYNCED).ok();
        let result = (|| -> rusqlite::Result<()> {
            let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
            for b in &batches {
                apply(&tx, b, now, &self.touched, &mut fresh)?;
            }
            tx.commit()
        })();
        self.conn.execute_batch(SYNCED).ok();
        if result.is_ok() {
            self.bound_unsynced();
        }
        note(&result);
        let ok = result.is_ok();
        let error = result.err().map(|e| e.to_string());
        for b in batches {
            let rows = b.len();
            match b {
                Batch::State(a, _) => self.state_event(settled(a, &error)),
                Batch::Docs(a, _) => self.docs_event(settled(a, &error)),
                Batch::Windows(a, _) => self.windows_event(settled(a, &error)),
                Batch::Previews(a, _) => self.previews_event(settled(a, &error)),
                Batch::Caches(a, _) => self.caches_event(settled(a, &error)),
            }
            if ok {
                self.written += rows;
            }
        }
        if !ok {
            return;
        }
        if changed_search { self.search.changed(); }
        super::journal::settle(revs.iter().map(|(k, rev)| (k.as_str(), *rev)));
        for id in fresh {
            self.touched.insert(id, now);
        }
        self.wal_event(WalEvent::Committed { boot });
        if docs {
            let effects = draft_gc::step(&mut self.seq, &mut self.gc, DraftGcEvent::DocumentsWritten { at_ms: now });
            self.gc_effects(effects);
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::CommitSoon => self.commit_due = true,
            Cmd::State { name, key, value } => self.state_event(WriteBehindEvent::Set { key: (name, key), value }),
            Cmd::ClearState { name } => {
                let n = name.clone();
                self.state_event(WriteBehindEvent::Drop(Box::new(move |k: &StateKey, _: &Value| k.0 == n)));
                let boot = crate::in_boot_record(&name, "");
                let done = (|| -> rusqlite::Result<()> {
                    let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
                    tx.execute("DELETE FROM state WHERE name = ?1", [&name])?;
                    tx.commit()
                })();
                if done.is_ok() {
                    self.wal_event(WalEvent::Committed { boot });
                }
            }
            Cmd::Doc { key, value, rev } => self.docs_event(WriteBehindEvent::Set { key, value: (value, rev) }),
            Cmd::Window { id, frame } => self.windows_event(WriteBehindEvent::Set { key: id, value: frame }),
            Cmd::Preview { server, session, value } => self.previews_event(WriteBehindEvent::Set { key: (server, session), value }),
            Cmd::Cache { key, value } => self.caches_event(WriteBehindEvent::Set { key, value }),
            Cmd::Blob { id, data } => {
                let now = (self.clock)();
                let inserted = sql::insert_blob(&self.conn, &id, &data, now);
                note(&inserted);
                if inserted.is_ok() {
                    self.touched.insert(id, now);
                    self.wal_event(WalEvent::Committed { boot: false });
                    let effects = draft_gc::step(&mut self.seq, &mut self.gc, DraftGcEvent::BlobUploaded { at_ms: now });
                    self.gc_effects(effects);
                }
            }
            Cmd::PluginRecord { id, record, done } => {
                let r = self.durable(true, |c, now| crate::plugins::native::write(c, &id, &record, now));
                answer(done, r);
            }
            Cmd::Outbox { op, done } => match outbox::native::prepare(op) {
                outbox::native::Prepared::Write(w, blobs, clear) => {
                    // Committed to the WAL (kill-safe), answered, then synced: the request leaves
                    // on the answer, and only a power cut in the sync's few ms is left open.
                    let r = self.committed(false, |c, now| outbox::native::write(c, &w, &blobs, &clear, now));
                    if r.is_ok() {
                        outbox::cached(&mut self.cache().outbox, &w);
                        super::journal::settle(clear.iter().filter(|(_, rev)| *rev > 0).map(|(k, rev)| (k.as_str(), *rev)));
                        self.search.changed();
                    }
                    let ok = r.is_ok();
                    answer(done, r);
                    if ok {
                        self.sync_wal();
                    }
                }
                outbox::native::Prepared::Read { server, adopt, done } => {
                    let adopted = match outbox::native::has_adoptees(&self.conn, &adopt) {
                        Ok(true) => self.durable(false, |c, now| outbox::native::adopt(c, &server, &adopt, now)).map(|()| true),
                        Ok(false) => Ok(false),
                        Err(e) => Err(StorageError(e.to_string())),
                    };
                    if adopted == Ok(true) {
                        // Re-read when next asked: the adopted rows moved.
                        self.cache().outbox = OutboxCache::Unloaded;
                    }
                    done(adopted.and_then(|_| outbox::native::read(&self.conn, &server).map_err(|e| StorageError(e.to_string()))));
                }
            },
            Cmd::LoadState { names, done } => {
                for name in names {
                    if let Ok(rows) = sql::read_namespace(&self.conn, &name) {
                        self.cache().hydrate(&name, rows);
                    }
                }
                ran(done);
            }
            Cmd::LoadDocs { keys, done } => {
                for key in keys {
                    if !matches!(self.cache().docs.get(&key), Some(Doc::Stored)) {
                        continue;
                    }
                    if let Ok(Some(text)) = sql::read_document(&self.conn, &key) {
                        let mut cache = self.cache();
                        // A write since the request already put the newer text in memory.
                        if matches!(cache.docs.get(&key), Some(Doc::Stored)) {
                            cache.docs.insert(key, Doc::Present(text.into()));
                        }
                    }
                }
                ran(done);
            }
            Cmd::LoadOutbox { done } => {
                if let Ok(all) = sql::read_outbox(&self.conn) {
                    let mut cache = self.cache();
                    if matches!(cache.outbox, OutboxCache::Unloaded) {
                        cache.outbox = OutboxCache::Loaded(all);
                    }
                }
                ran(done);
            }
            Cmd::GetBlob { id, done } => done(sql::read_blob(&self.conn, &id).ok().flatten()),
            Cmd::GetPreview { server, session, done } => {
                let row = self.conn.prepare_cached("SELECT updated_at, data FROM session_preview WHERE server = ?1 AND session = ?2").and_then(|mut s| s.query_row([&server, &session], |r| Ok((r.get(0)?, r.get(1)?))).optional());
                done(row.ok().flatten());
            }
            Cmd::GetCache { key, version, done } => {
                let row: Option<Vec<u8>> = self.conn.prepare_cached("SELECT data FROM cache WHERE key = ?1 AND version = ?2").and_then(|mut s| s.query_row(rusqlite::params![key, version], |r| r.get(0)).optional()).ok().flatten();
                done(row);
            }
        }
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A kill-safe write: committed to the WAL (handed to the operating system, no sync) before
    /// it returns; `sync_wal` makes it survive a power cut too.
    fn committed(&mut self, boot: bool, f: impl FnOnce(&Connection, i64) -> rusqlite::Result<()>) -> Result<(), StorageError> {
        let now = (self.clock)();
        let result = (|| {
            // Unsynced, the WAL header included (see `commit_if_due`); `sync_wal` follows.
            self.conn.execute_batch(UNSYNCED)?;
            let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
            f(&tx, now)?;
            tx.commit()
        })();
        note(&result);
        self.conn.execute_batch(SYNCED).ok();
        match result {
            Ok(()) => {
                self.wal_event(WalEvent::Committed { boot });
                Ok(())
            }
            Err(e) => Err(StorageError(e.to_string())),
        }
    }

    /// The WAL's frames to the disk (what `synchronous = FULL` does at each commit), through a
    /// handle of its own kept open; a failed sync waits for the next checkpoint's.
    fn sync_wal(&mut self) {
        if self.wal_file.is_none() {
            let path = self.conn.path().map(|p| format!("{p}-wal"));
            self.wal_file = path.and_then(|p| std::fs::OpenOptions::new().write(true).open(p).ok());
        }
        if let Some(f) = &self.wal_file
            && f.sync_data().is_ok()
        {
            self.unsynced_since = None;
        }
        rune::marks::mark("outbox.synced");
    }

    /// Unsynced commits leave SQLite's own checkpoint off (`UNSYNCED`): one run at
    /// `synchronous = OFF` copies unsynced frames into the database file without a sync first,
    /// which a power cut could leave half-written. This runs it at NORMAL instead (the WAL synced,
    /// then copied, then the file synced) once unsynced commits have piled up for
    /// `UNSYNCED_FOR` or the WAL has passed `UNSYNCED_BYTES`, which bounds what a power cut can
    /// lose while changes keep coming; a quiet spell's checkpoint (`IDLE_CHECKPOINT_MS`) and the
    /// quit's come sooner.
    fn bound_unsynced(&mut self) {
        let since = *self.unsynced_since.get_or_insert_with(Instant::now);
        if self.wal_file.is_none() {
            let path = self.conn.path().map(|p| format!("{p}-wal"));
            self.wal_file = path.and_then(|p| std::fs::OpenOptions::new().write(true).open(p).ok());
        }
        let bytes = self.wal_file.as_ref().and_then(|f| f.metadata().ok()).map_or(0, |m| m.len());
        if since.elapsed() < UNSYNCED_FOR && bytes < UNSYNCED_BYTES {
            return;
        }
        self.conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(())).ok();
        self.unsynced_since = None;
    }

    /// A Tier 0 write: committed and synced before it returns.
    fn durable(&mut self, boot: bool, f: impl FnOnce(&Connection, i64) -> rusqlite::Result<()>) -> Result<(), StorageError> {
        let now = (self.clock)();
        let result = (|| {
            self.conn.execute_batch("PRAGMA synchronous = FULL")?;
            let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
            f(&tx, now)?;
            tx.commit()
        })();
        note(&result);
        self.conn.execute_batch("PRAGMA synchronous = NORMAL").ok();
        match result {
            Ok(()) => {
                self.wal_event(WalEvent::Committed { boot });
                Ok(())
            }
            Err(e) => Err(StorageError(e.to_string())),
        }
    }

    fn flush(&mut self, reason: FlushReason, started: Instant) -> FlushReport {
        let quit = matches!(reason, FlushReason::Quit | FlushReason::SessionEnd | FlushReason::UpdateRestart);
        let pending = wb::has_pending(&self.state) || wb::has_pending(&self.docs) || wb::has_pending(&self.windows) || wb::has_pending(&self.previews) || wb::has_pending(&self.caches);
        if !pending && (!quit || self.wal == WalState::Clean) {
            return FlushReport { clean: true, micros: super::micros(started), ..FlushReport::default() };
        }
        self.written = 0;
        let mut batches: Vec<Batch> = Vec::new();
        batches.extend(step_buffer(&mut self.seq, &mut self.state, WriteBehindEvent::FlushRequested, STATE_MS, "state", &mut self.timers).into_iter().map(|(a, m)| Batch::State(a, m)));
        batches.extend(step_buffer(&mut self.seq, &mut self.docs, WriteBehindEvent::FlushRequested, DOCS_MS, "docs", &mut self.timers).into_iter().map(|(a, m)| Batch::Docs(a, m)));
        batches.extend(step_buffer(&mut self.seq, &mut self.windows, WriteBehindEvent::FlushRequested, WINDOWS_MS, "windows", &mut self.timers).into_iter().map(|(a, m)| Batch::Windows(a, m)));
        batches.extend(step_buffer(&mut self.seq, &mut self.previews, WriteBehindEvent::FlushRequested, PREVIEWS_MS, "previews", &mut self.timers).into_iter().map(|(a, m)| Batch::Previews(a, m)));
        batches.extend(step_buffer(&mut self.seq, &mut self.caches, WriteBehindEvent::FlushRequested, CACHES_MS, "caches", &mut self.timers).into_iter().map(|(a, m)| Batch::Caches(a, m)));
        self.commit(batches);
        // At quit the WAL goes back into the file (`quit_checkpoint`) after the caller has its
        // answer: the rows are committed now, and the synced checkpoint overlaps the windows
        // closing; closing the storage waits for it.
        let checkpointed = quit && self.wal != WalState::Clean;
        FlushReport { clean: false, rows: self.written, micros: super::micros(started), checkpointed, timed_out: false }
    }

    /// A file made before 16 KiB pages is rewritten with them as the storage closes, once, while
    /// it's small enough to take a few ms (`REPAGE_MAX`): a commit writes whole pages to the WAL,
    /// and the kill-safe tier commits a keystroke's draft at a time. A bigger file keeps its 64 KiB
    /// pages. Rewritten with `VACUUM` under the rollback journal, so a kill midway leaves the old
    /// file; the boot record's raw read is kept only if its table's root is still page 2.
    fn repage(&mut self) {
        let get = |pragma: &str| self.conn.query_row(&format!("PRAGMA {pragma}"), [], |r| r.get::<_, i64>(0)).unwrap_or(0);
        let (page, pages) = (get("page_size"), get("page_count"));
        if page <= REPAGE_TO || page * pages > REPAGE_MAX {
            return;
        }
        let t = Instant::now();
        // VACUUM moves every page: the draft journal's place goes with it, so it's reserved
        // again at the next launch (closed and clean by now: `journal::finish`).
        self.conn.execute_batch("DELETE FROM journal_meta; DELETE FROM journal;").ok();
        let done = self.conn.execute_batch(&format!("PRAGMA journal_mode = DELETE; PRAGMA page_size = {REPAGE_TO}; VACUUM; PRAGMA journal_mode = WAL;"));
        // The WAL back to empty: the next launch reads the record raw.
        self.conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())).ok();
        let root: i64 = self.conn.query_row("SELECT rootpage FROM sqlite_schema WHERE name = 'boot'", [], |r| r.get(0)).unwrap_or(0);
        if root != 2
            && let Ok(version) = super::schema::user_version(&self.conn)
        {
            super::schema::set_flags(&self.conn, version & super::schema::FLAG_MASK & !super::schema::FLAG_BOOT_PAGE2).ok();
        }
        eprintln!("opencode-storage: {} {} pages of {page} B into {REPAGE_TO} B pages in {} us", if done.is_ok() { "rewrote" } else { "couldn't rewrite" }, pages, t.elapsed().as_micros());
    }

    /// The kill-safe tier (`Storage::commit_soon`): once the queue is drained, the state,
    /// document and window rows waiting in their buffers go to the WAL in one commit (no sync).
    fn commit_if_due(&mut self) {
        if !std::mem::take(&mut self.commit_due) {
            return;
        }
        let mut batches: Vec<Batch> = Vec::new();
        batches.extend(step_buffer(&mut self.seq, &mut self.state, WriteBehindEvent::FlushRequested, STATE_MS, "state", &mut self.timers).into_iter().map(|(a, m)| Batch::State(a, m)));
        batches.extend(step_buffer(&mut self.seq, &mut self.docs, WriteBehindEvent::FlushRequested, DOCS_MS, "docs", &mut self.timers).into_iter().map(|(a, m)| Batch::Docs(a, m)));
        batches.extend(step_buffer(&mut self.seq, &mut self.windows, WriteBehindEvent::FlushRequested, WINDOWS_MS, "windows", &mut self.timers).into_iter().map(|(a, m)| Batch::Windows(a, m)));
        // No sync at all, not even the WAL header's after a checkpoint (1,400 µs at NORMAL
        // against 145-165 µs, `proof\storage-2\first-commit.txt`): the frames are the operating
        // system's once written; the next checkpoint (at NORMAL) syncs them (`commit`).
        self.commit(batches);
    }

    /// The quit's checkpoint, after its flush answered: the next open finds the WAL empty and
    /// reads the boot record raw.
    fn quit_checkpoint(&mut self, reason: FlushReason) {
        if matches!(reason, FlushReason::Quit | FlushReason::SessionEnd | FlushReason::UpdateRestart) {
            self.wal_event(WalEvent::QuitRequested);
        }
    }

    fn close_machines(&mut self) {
        let effects = draft_gc::step(&mut self.seq, &mut self.gc, DraftGcEvent::CloseRequested);
        self.gc_effects(effects);
        self.state_event(WriteBehindEvent::CloseRequested);
        self.docs_event(WriteBehindEvent::CloseRequested);
        self.windows_event(WriteBehindEvent::CloseRequested);
        self.previews_event(WriteBehindEvent::CloseRequested);
        self.caches_event(WriteBehindEvent::CloseRequested);
    }

    fn first_frame(&mut self, done: Option<Loaded>) {
        let now = (self.clock)();
        if matches!(self.search, super::search::State::BeforeFrame) {
            self.conn.execute_batch(UNSYNCED).ok();
            let initialized = super::search::initialize(&mut self.conn, now);
            self.conn.execute_batch(SYNCED).ok();
            self.bound_unsynced();
            self.search = match initialized {
                Ok(()) => super::search::State::Ready(super::search::Work::Pending),
                Err(e) => super::search::State::Unavailable(e.to_string()),
            };
        }
        if !self.deferred.pending() { ran(done); return }
        let seeded = import::after_first_frame(&mut self.conn, &mut self.deferred, now);
        if !seeded.is_empty() {
            let mut cache = self.cache();
            for (name, key, value) in seeded {
                if matches!(cache.get(&name, &key), crate::Lookup::Ready(None)) {
                    cache.set(&name, key, Some(value.into()));
                }
            }
        }
        self.wal_event(WalEvent::Committed { boot: false });
        ran(done);
    }

    fn gc_effects(&mut self, effects: Effects<DraftGcEffect>) {
        for effect in effects {
            match effect {
                DraftGcEffect::ArmTimer { attempt, delay_ms } => {
                    self.timers.insert("gc", Some((attempt, Instant::now() + Duration::from_millis(u64::try_from(delay_ms).unwrap_or(0)))));
                }
                DraftGcEffect::CancelTimer => {
                    self.timers.insert("gc", None);
                }
                DraftGcEffect::Collect { attempt, at_ms, before_ms } => {
                    let event = match sql::collect_blobs(&self.conn, before_ms) {
                        Ok(oldest) => {
                            self.touched.retain(|_, t| *t >= before_ms);
                            DraftGcEvent::CollectSucceeded { attempt, at_ms, oldest_orphan_ms: oldest }
                        }
                        Err(e) => DraftGcEvent::CollectFailed { attempt, at_ms, error: e.to_string() },
                    };
                    let next = draft_gc::step(&mut self.seq, &mut self.gc, event);
                    self.gc_effects(next);
                }
                DraftGcEffect::ReportError(e) => eprintln!("opencode-storage: blob sweep failed: {e}"),
            }
        }
    }

    fn wal_event(&mut self, event: WalEvent) {
        let mut queue: Vec<WalEffect> = checkpoint::step(&mut self.seq, &mut self.wal, event).into_iter().collect();
        while let Some(effect) = queue.pop() {
            match effect {
                WalEffect::ArmTimer { attempt, delay_ms } => {
                    self.timers.insert("wal", Some((attempt, Instant::now() + Duration::from_millis(delay_ms))));
                }
                WalEffect::CancelTimer => {
                    self.timers.insert("wal", None);
                }
                WalEffect::Checkpoint { rebuild_boot } => {
                    // The record goes current in the transaction right before the checkpoint, so
                    // whenever the WAL is empty the record on page 2 is the truth.
                    let rebuilt = !rebuild_boot
                        || (|| -> rusqlite::Result<()> {
                            let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
                            boot_record::rebuild(&tx)?;
                            tx.commit()
                        })()
                        .is_ok();
                    rune::marks::mark("writer.rebuilt");
                    let busy = if rebuilt { self.conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get::<_, i64>(0)).unwrap_or(1) } else { 1 };
                    if busy == 0 {
                        self.unsynced_since = None;
                    }
                    let event = if busy == 0 { WalEvent::Checkpointed } else { WalEvent::CheckpointFailed };
                    queue.extend(checkpoint::step(&mut self.seq, &mut self.wal, event));
                }
            }
        }
    }
}

fn ran(done: Option<Loaded>) {
    if let Some(done) = done {
        done();
    }
}

fn answer(done: Option<crate::Done>, result: Result<(), StorageError>) {
    if let Some(done) = done {
        done(result);
    }
}

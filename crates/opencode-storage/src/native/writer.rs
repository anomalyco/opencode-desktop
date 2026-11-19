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
use rune::machine::{Attempt, Seq};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};

use super::import::{self, Deferred};
use super::sql;
use crate::cache::{Cache, Doc, OutboxCache};
use crate::machine::draft_gc::{self, DraftGcEvent, DraftGcState, STARTUP_COLLECT_DELAY_MS};
use crate::machine::write_behind::{self as wb, WriteBehindEffect, WriteBehindEvent, WriteBehindState};
use crate::outbox;
use crate::{Cmd, FlushReason, FlushReport, Loaded, StorageError, WindowFrame};

mod durability;
mod reserve;
mod rows;
use durability::{SYNCED, UNSYNCED, Wal, WalEvent, WalFile};
pub(crate) use reserve::OpenJournal;
use reserve::{Reservation, ReserveStep};
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

const STATE_MS: u64 = 250;
const DOCS_MS: u64 = 500;
const WINDOWS_MS: u64 = 250;
const PREVIEWS_MS: u64 = 500;
const CACHES_MS: u64 = 1000;

/// A timer a machine armed: its attempt and when it's due.
type Armed = Option<(Attempt, Instant)>;

pub(crate) struct Writer {
    conn: Connection,
    /// The WAL, opened once for `sync_wal` and `wal_unsynced` (`SQLITE_FCNTL_PERSIST_WAL` keeps
    /// `-wal` on disk across checkpoints, so the handle outlives `Wal::Dirty`).
    wal_file: WalFile,
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
    wal: Wal,
    timers: HashMap<&'static str, Armed>,
    deferred: Deferred,
    /// When this run last touched each chunk, so an unchanged chunk of a long draft isn't
    /// rewritten on every save (a blob's touch rewrites its whole record).
    touched: HashMap<String, i64>,
    /// Rows written since the last flush, for its report.
    written: usize,
    reservation: Reservation,
    origin: super::search::Origin,
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
    /// the tables (the open reserved the draft journal, whose place the record names); `created`:
    /// this open made the file, so schema 2 is its own (`search::Origin`).
    pub(crate) fn new(conn: Connection, cache: Arc<Mutex<Cache>>, clock: fn() -> i64, deferred: Deferred, wal_dirty: bool, journal: OpenJournal, created: bool) -> Writer {
        let seq = Seq::default();
        let boot_stale = journal == OpenJournal::Reserved;
        let mut writer = Writer {
            conn,
            cache,
            reservation: match journal {
                OpenJournal::Settled => Reservation::Durable,
                OpenJournal::Reserved => Reservation::Unsynced,
                OpenJournal::Owed => Reservation::Owed(ReserveStep::AfterFrame),
            },
            origin: if created { super::search::Origin::Made } else { super::search::Origin::Found },
            search: super::search::State::BeforeFrame,
            clock,
            seq,
            state: WriteBehindState::Idle,
            docs: WriteBehindState::Idle,
            windows: WriteBehindState::Idle,
            previews: WriteBehindState::Idle,
            caches: WriteBehindState::Idle,
            gc: DraftGcState::Closed,
            wal: Wal::Clean,
            timers: HashMap::new(),
            deferred,
            touched: HashMap::new(),
            written: 0,
            wal_file: WalFile::Unopened,
        };
        if wal_dirty || boot_stale {
            writer.wal_event(WalEvent::OpenedDirty { boot_stale, now: Instant::now() });
        }
        writer
    }

    /// The WAL holds frames to checkpoint (quit has work even with nothing queued).
    pub(crate) fn wal_dirty(&self) -> bool {
        !self.wal.is_clean()
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
                let mut commit_due = false;
                for cmd in cmds {
                    let (what, t) = (cmd_name(&cmd), Instant::now());
                    commit_due |= self.handle(cmd);
                    slow(what, t);
                }
                if commit_due {
                    let t = Instant::now();
                    self.commit_buffered();
                    slow("the kill-safe commit", t);
                }
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
                            super::search::State::Ready(work) => {
                                use super::search::query::Completion;
                                match query.run(&mut self.conn, &urgent, admitted, matches!(work, super::search::Work::Pending)) {
                                    Completion::Done => {},
                                    Completion::Yield(q) => super::lock_queue(&queue).queries.replace(q),
                                    Completion::AwaitIndex(q) => super::lock_queue(&queue).queries.wait(q),
                                }
                            }
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
                        match stepped {
                            Ok(Step::Done { key, work_us, commit_us }) => {
                                self.wal_unsynced(false);
                                if work_us + commit_us >= 250_000 {
                                    eprintln!("opencode-storage: indexing {key:?} for search took {work_us} us, its commit {commit_us} us");
                                }
                                self.fire_due();
                                continue;
                            }
                            Ok(Step::Yielded) => continue,
                            Ok(Step::Empty) => self.search = super::search::State::Ready(super::search::Work::Drained),
                            Err(e) => self.search = super::search::State::Unavailable(e.to_string()),
                        }
                        // A still-open query whose answer was partial gets a final answer (or
                        // unavailable) without a timer, another keystroke or one wake per row.
                        if super::lock_queue(&queue).queries.indexing_finished() { continue; }
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
            let mut commit_due = false;
            for cmd in cmds {
                commit_due |= self.handle(cmd);
            }
            if commit_due {
                self.commit_buffered();
            }
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
                    self.reserve_stopped();
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
            "reserve" => self.reserve_step(),
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
            self.wal_unsynced(boot);
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
        if docs {
            let effects = draft_gc::step(&mut self.seq, &mut self.gc, DraftGcEvent::DocumentsWritten { at_ms: now });
            self.gc_effects(effects);
        }
    }

    /// Runs `cmd`; returns `true` when it asks for a kill-safe commit (`Cmd::CommitSoon`) after
    /// the current batch drains.
    fn handle(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::CommitSoon => return true,
            Cmd::State { name, key, value } => self.state_event(WriteBehindEvent::Set { key: (name, key), value }),
            Cmd::ClearState { name } => {
                let n = name.clone();
                self.state_event(WriteBehindEvent::Drop(Box::new(move |k: &StateKey, _: &Value| k.0 == n)));
                let boot = crate::in_boot_record(&name, "");
                let done = (|| -> rusqlite::Result<usize> {
                    let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
                    let deleted = tx.execute("DELETE FROM state WHERE name = ?1", [&name])?;
                    tx.commit()?;
                    Ok(deleted)
                })();
                note(&done);
                if matches!(done, Ok(n) if n > 0) {
                    self.wal_unsynced(boot);
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
                if let Ok(added) = inserted {
                    self.touched.insert(id, now);
                    if added {
                        self.wal_unsynced(false);
                    }
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
                    // Tier 0: committed and synced (`synchronous = FULL`) before settling the
                    // draft journal or telling the caller the prompt is saved.
                    let r = self.durable(false, |c, now| outbox::native::write(c, &w, &blobs, &clear, now));
                    if r.is_ok() {
                        outbox::cached(&mut self.cache().outbox, &w);
                        super::journal::settle(clear.iter().filter(|(_, rev)| *rev > 0).map(|(k, rev)| (k.as_str(), *rev)));
                        self.search.changed();
                        rune::marks::mark("outbox.synced");
                    }
                    answer(done, r);
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
        false
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn first_frame(&mut self, done: Option<Loaded>) {
        if self.reservation == Reservation::Unsynced {
            self.sync_reservation();
        }
        self.reserve_after_frame();
        let now = (self.clock)();
        if matches!(self.search, super::search::State::BeforeFrame) {
            let before = super::schema::user_version(&self.conn).ok();
            self.conn.execute_batch(UNSYNCED).ok();
            let initialized = super::search::initialize(&mut self.conn, now, self.origin);
            self.conn.execute_batch(SYNCED).ok();
            // `initialize` commits a schema-2 migration transaction iff `user_version` advances.
            if matches!(initialized, Ok(true)) && super::schema::user_version(&self.conn).ok() != before {
                self.wal_unsynced(false);
            }
            self.search = match initialized {
                Ok(true) => super::search::State::Ready(super::search::Work::Pending),
                Ok(false) => super::search::State::Unavailable("schema 2 is off for this profile (RUNE_STORAGE_V2)".into()),
                Err(e) => super::search::State::Unavailable(e.to_string()),
            };
        }
        if !self.deferred.pending() { ran(done); return }
        let seeded = import::after_first_frame(&mut self.conn, &mut self.deferred, now);
        let boot = seeded.iter().any(|(name, key, _)| crate::in_boot_record(name, key));
        if !seeded.is_empty() {
            let mut cache = self.cache();
            for (name, key, value) in seeded {
                if matches!(cache.get(&name, &key), crate::Lookup::Ready(None)) {
                    cache.set(&name, key, Some(value.into()));
                }
            }
        }
        self.wal_unsynced(boot);
        ran(done);
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

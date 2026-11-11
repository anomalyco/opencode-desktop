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
use crate::machine::draft_gc::{self, BLOB_GRACE_MS, DraftGcEffect, DraftGcEvent, DraftGcState, STARTUP_COLLECT_DELAY_MS};
use crate::machine::write_behind::{self as wb, WriteBehindEffect, WriteBehindEvent, WriteBehindState};
use crate::{Cmd, FlushReason, FlushReport, Loaded, OutboxOp, StorageError, WindowFrame};

pub(crate) enum Msg {
    Cmd(Cmd),
    Flush { reason: FlushReason, started: Instant, reply: SyncSender<FlushReport> },
    FirstFrame { done: Option<Loaded> },
    Stop,
}

type StateKey = (String, String);
type Value = Option<Arc<str>>;
type PreviewKey = (String, String);
/// A preview's `updated_at` and bytes.
pub(crate) type Preview = (i64, Arc<[u8]>);
/// A cache entry's version and bytes.
pub(crate) type CacheEntry = (i64, Arc<[u8]>);

/// A write batch a buffer's machine asked for, tagged with the buffer.
enum Batch {
    State(Attempt, IndexMap<StateKey, Value>),
    Docs(Attempt, IndexMap<String, Value>),
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
            Batch::State(_, m) => m.keys().any(|(name, _)| crate::is_boot_namespace(name)),
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
    cache: Arc<Mutex<Cache>>,
    clock: fn() -> i64,
    seq: Seq,
    state: WriteBehindState<StateKey, Value>,
    docs: WriteBehindState<String, Value>,
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
    pub(crate) fn new(conn: Connection, cache: Arc<Mutex<Cache>>, clock: fn() -> i64, deferred: Deferred, wal_dirty: bool) -> Writer {
        let mut seq = Seq::default();
        let wal = if wal_dirty { WalState::Dirty { attempt: seq.next(), boot_stale: false } } else { WalState::Clean };
        Writer {
            conn,
            cache,
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
        }
    }

    /// The WAL holds frames to checkpoint (quit has work even with nothing queued).
    pub(crate) fn wal_dirty(&self) -> bool {
        self.wal != WalState::Clean
    }

    pub(crate) fn has_deferred(&self) -> bool {
        self.deferred.pending()
    }

    pub(crate) fn run(mut self, rx: Receiver<Msg>) {
        let (gc, effects) = draft_gc::initial(&mut self.seq, (self.clock)(), STARTUP_COLLECT_DELAY_MS);
        self.gc = gc;
        self.gc_effects(effects);
        loop {
            let msg = match self.timers.values().filter_map(|t| t.map(|(_, due)| due)).min() {
                Some(due) => rx.recv_timeout(due.saturating_duration_since(Instant::now())),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match msg {
                Ok(Msg::Cmd(cmd)) => self.handle(cmd),
                Ok(Msg::Flush { reason, started, reply }) => {
                    let report = self.flush(reason, started);
                    reply.send(report).ok();
                }
                Ok(Msg::FirstFrame { done }) => self.first_frame(done),
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => {
                    self.flush(FlushReason::Quit, Instant::now());
                    self.close_machines();
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.fire_due();
        }
    }

    fn fire_due(&mut self) {
        let now = Instant::now();
        let due: Vec<(&'static str, Attempt)> = self.timers.iter().filter_map(|(n, t)| t.filter(|(_, d)| *d <= now).map(|(a, _)| (*n, a))).collect();
        for (name, attempt) in due {
            self.timers.insert(name, None);
            let fired = WriteBehindEvent::TimerFired { attempt };
            match name {
                "state" => self.state_event(fired),
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
    }

    fn state_event(&mut self, event: WriteBehindEvent<StateKey, Value>) {
        let out = step_buffer(&mut self.seq, &mut self.state, event, STATE_MS, "state", &mut self.timers);
        self.commit(out.into_iter().map(|(a, m)| Batch::State(a, m)).collect());
    }

    fn docs_event(&mut self, event: WriteBehindEvent<String, Value>) {
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
        let mut fresh: Vec<String> = Vec::new();
        let result = (|| -> rusqlite::Result<()> {
            let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
            for b in &batches {
                apply(&tx, b, now, &self.touched, &mut fresh)?;
            }
            tx.commit()
        })();
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
            Cmd::State { name, key, value } => self.state_event(WriteBehindEvent::Set { key: (name, key), value }),
            Cmd::ClearState { name } => {
                let n = name.clone();
                self.state_event(WriteBehindEvent::Drop(Box::new(move |k: &StateKey, _: &Value| k.0 == n)));
                let boot = crate::is_boot_namespace(&name);
                let done = (|| -> rusqlite::Result<()> {
                    let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
                    tx.execute("DELETE FROM state WHERE name = ?1", [&name])?;
                    tx.commit()
                })();
                if done.is_ok() {
                    self.wal_event(WalEvent::Committed { boot });
                }
            }
            Cmd::Doc { key, value } => self.docs_event(WriteBehindEvent::Set { key, value }),
            Cmd::Window { id, frame } => self.windows_event(WriteBehindEvent::Set { key: id, value: frame }),
            Cmd::Preview { server, session, value } => self.previews_event(WriteBehindEvent::Set { key: (server, session), value }),
            Cmd::Cache { key, value } => self.caches_event(WriteBehindEvent::Set { key, value }),
            Cmd::Blob { id, data } => {
                let now = (self.clock)();
                if sql::insert_blob(&self.conn, &id, &data, now).is_ok() {
                    self.touched.insert(id, now);
                    self.wal_event(WalEvent::Committed { boot: false });
                    let effects = draft_gc::step(&mut self.seq, &mut self.gc, DraftGcEvent::BlobUploaded { at_ms: now });
                    self.gc_effects(effects);
                }
            }
            Cmd::Plugin { id, enabled, done } => {
                let r = self.durable(true, |c, _| sql::put_plugin(c, &id, enabled));
                answer(done, r);
            }
            Cmd::Trust { id, sha256, done } => {
                let r = self.durable(true, |c, now| sql::put_trust(c, &id, sha256.as_deref(), now));
                answer(done, r);
            }
            Cmd::Outbox { op, done } => {
                let r = self.durable(false, |c, now| match &op {
                    OutboxOp::Put { row, blobs } => {
                        for (id, data) in blobs {
                            sql::insert_blob(c, id, data, now)?;
                        }
                        sql::put_outbox(c, row, now, true).map(drop)
                    }
                    OutboxOp::Delete { server, id } => sql::delete_outbox(c, server, id),
                });
                answer(done, r);
            }
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

    /// A Tier 0 write: committed and synced before it returns.
    fn durable(&mut self, boot: bool, f: impl FnOnce(&Connection, i64) -> rusqlite::Result<()>) -> Result<(), StorageError> {
        let now = (self.clock)();
        let result = (|| {
            self.conn.execute_batch("PRAGMA synchronous = FULL")?;
            let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
            f(&tx, now)?;
            tx.commit()
        })();
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
        let checkpointed = quit && self.wal != WalState::Clean;
        if quit {
            self.wal_event(WalEvent::QuitRequested);
        }
        FlushReport { clean: false, rows: self.written, micros: super::micros(started), checkpointed, timed_out: false }
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
                    let busy = if rebuilt { self.conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get::<_, i64>(0)).unwrap_or(1) } else { 1 };
                    let event = if busy == 0 { WalEvent::Checkpointed } else { WalEvent::CheckpointFailed };
                    queue.extend(checkpoint::step(&mut self.seq, &mut self.wal, event));
                }
            }
        }
    }
}

/// One batch's rows. Documents' long strings become chunks here (never on the caller's thread);
/// a chunk this run touched within half the grace period isn't touched again.
fn apply(tx: &Connection, batch: &Batch, now: i64, touched: &HashMap<String, i64>, fresh: &mut Vec<String>) -> rusqlite::Result<()> {
    match batch {
        Batch::State(_, rows) => {
            for ((name, key), value) in rows {
                sql::put_state(tx, name, key, value.as_deref(), now)?;
            }
        }
        Batch::Windows(_, rows) => {
            for (id, frame) in rows {
                sql::put_window(tx, id, frame.as_ref(), now)?;
            }
        }
        Batch::Docs(_, rows) => {
            for (key, value) in rows {
                let Some(text) = value else {
                    sql::put_document(tx, key, None)?;
                    continue;
                };
                let (stored, chunks) = sql::encode_text(text);
                for (id, data) in &chunks {
                    if touched.get(id).is_some_and(|t| now - t < BLOB_GRACE_MS / 2) {
                        continue;
                    }
                    if !sql::insert_blob(tx, id, data, now)? {
                        sql::touch_blob(tx, id, now)?;
                    }
                    fresh.push(id.clone());
                }
                sql::put_document(tx, key, Some(&stored))?;
            }
        }
        Batch::Previews(_, rows) => {
            for ((server, session), value) in rows {
                match value {
                    Some((at, data)) => tx
                        .prepare_cached("INSERT INTO session_preview (server, session, updated_at, data) VALUES (?1, ?2, ?3, ?4) ON CONFLICT (server, session) DO UPDATE SET updated_at = excluded.updated_at, data = excluded.data")?
                        .execute(rusqlite::params![server, session, at, &**data])?,
                    None => tx.prepare_cached("DELETE FROM session_preview WHERE server = ?1 AND session = ?2")?.execute([server, session])?,
                };
            }
        }
        Batch::Caches(_, rows) => {
            for (key, value) in rows {
                match value {
                    Some((version, data)) => tx
                        .prepare_cached("INSERT INTO cache (key, version, data, touched_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT (key) DO UPDATE SET version = excluded.version, data = excluded.data, touched_at = excluded.touched_at")?
                        .execute(rusqlite::params![key, version, &**data, now])?,
                    None => tx.prepare_cached("DELETE FROM cache WHERE key = ?1")?.execute([key])?,
                };
            }
        }
    }
    Ok(())
}

/// How a batch's write went, as its machine's event.
fn settled<K, V>(attempt: Attempt, error: &Option<String>) -> WriteBehindEvent<K, V> {
    match error {
        None => WriteBehindEvent::WriteSucceeded { attempt },
        Some(e) => WriteBehindEvent::WriteFailed { attempt, error: e.clone() },
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

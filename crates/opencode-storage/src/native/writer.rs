//! The `opencode-storage` thread: the only owner of the read-write connection once the first
//! write starts it. It drives the write-behind buffers (`state` and window frames at 250 ms,
//! documents at 500 ms), commits Tier 0 rows at once with `synchronous = FULL`, sweeps blobs,
//! checkpoints the WAL when writes go quiet, and loads lazy rows into the cache.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use rune::machine::{Attempt, Effects, Seq};
use rusqlite::{Connection, TransactionBehavior};

use super::import::{self, Deferred};
use super::sql;
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

const STATE_DELAY_MS: u64 = 250;
const DOCS_DELAY_MS: u64 = 500;
const WINDOWS_DELAY_MS: u64 = 250;

type StateKey = (String, String);
type Value = Option<Arc<str>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Buf {
    State,
    Docs,
    Windows,
}

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

impl Writer {
    pub(crate) fn new(conn: Connection, cache: Arc<Mutex<Cache>>, clock: fn() -> i64, deferred: Deferred) -> Writer {
        Writer {
            conn,
            cache,
            clock,
            seq: Seq::default(),
            state: WriteBehindState::Idle,
            docs: WriteBehindState::Idle,
            windows: WriteBehindState::Idle,
            gc: DraftGcState::Closed,
            wal: WalState::Clean,
            timers: HashMap::new(),
            deferred,
            touched: HashMap::new(),
            written: 0,
        }
    }

    pub(crate) fn has_deferred(&self) -> bool {
        self.deferred.pending()
    }

    pub(crate) fn run(mut self, rx: Receiver<Msg>) {
        let (gc, effects) = draft_gc::initial(&mut self.seq, (self.clock)(), STARTUP_COLLECT_DELAY_MS);
        self.gc = gc;
        self.gc_effects(effects);
        loop {
            let msg = match self.next_due() {
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

    fn next_due(&self) -> Option<Instant> {
        self.timers.values().filter_map(|t| t.map(|(_, due)| due)).min()
    }

    fn arm(&mut self, name: &'static str, attempt: Attempt, delay_ms: u64) {
        self.timers.insert(name, Some((attempt, Instant::now() + Duration::from_millis(delay_ms))));
    }

    fn fire_due(&mut self) {
        let now = Instant::now();
        let due: Vec<(&'static str, Attempt)> = self.timers.iter().filter_map(|(n, t)| t.filter(|(_, d)| *d <= now).map(|(a, _)| (*n, a))).collect();
        for (name, attempt) in due {
            self.timers.insert(name, None);
            match name {
                "state" => self.state_event(WriteBehindEvent::TimerFired { attempt }),
                "docs" => self.docs_event(WriteBehindEvent::TimerFired { attempt }),
                "windows" => self.windows_event(WriteBehindEvent::TimerFired { attempt }),
                "gc" => {
                    let effects = draft_gc::step(&mut self.seq, &mut self.gc, DraftGcEvent::TimerFired { attempt, at_ms: (self.clock)() });
                    self.gc_effects(effects);
                }
                _ => self.wal_event(WalEvent::TimerFired { attempt }),
            }
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::State { name, key, value } => self.state_event(WriteBehindEvent::Set { key: (name, key), value }),
            Cmd::ClearState { name } => {
                let n = name.clone();
                self.state_event(WriteBehindEvent::Drop(Box::new(move |k: &StateKey, _: &Value| k.0 == n)));
                if self.conn.execute("DELETE FROM state WHERE name = ?1", [&name]).is_ok() {
                    self.wal_event(WalEvent::Committed);
                }
            }
            Cmd::Doc { key, value } => self.docs_event(WriteBehindEvent::Set { key, value }),
            Cmd::Window { id, frame } => self.windows_event(WriteBehindEvent::Set { key: id, value: frame }),
            Cmd::Blob { id, data } => {
                let now = (self.clock)();
                if sql::insert_blob(&self.conn, &id, &data, now).is_ok() {
                    self.touched.insert(id, now);
                    self.wal_event(WalEvent::Committed);
                    let effects = draft_gc::step(&mut self.seq, &mut self.gc, DraftGcEvent::BlobUploaded { at_ms: now });
                    self.gc_effects(effects);
                }
            }
            Cmd::Plugin { id, enabled, done } => {
                let r = self.durable(|c, _| sql::put_plugin(c, &id, enabled));
                answer(done, r);
            }
            Cmd::Trust { id, sha256, done } => {
                let r = self.durable(|c, now| sql::put_trust(c, &id, sha256.as_deref(), now));
                answer(done, r);
            }
            Cmd::Outbox { op, done } => {
                let r = self.durable(|c, now| match &op {
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
                if let Some(done) = done {
                    done();
                }
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
                if let Some(done) = done {
                    done();
                }
            }
            Cmd::LoadOutbox { done } => {
                if let Ok(all) = sql::read_outbox(&self.conn) {
                    let mut cache = self.cache();
                    if matches!(cache.outbox, OutboxCache::Unloaded) {
                        cache.outbox = OutboxCache::Loaded(all);
                    }
                }
                if let Some(done) = done {
                    done();
                }
            }
            Cmd::GetBlob { id, done } => done(sql::read_blob(&self.conn, &id).ok().flatten()),
        }
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn state_event(&mut self, event: WriteBehindEvent<StateKey, Value>) {
        self.buffer(Buf::State, |w| &mut w.state, Writer::write_state, event);
    }

    fn docs_event(&mut self, event: WriteBehindEvent<String, Value>) {
        self.buffer(Buf::Docs, |w| &mut w.docs, Writer::write_docs, event);
    }

    fn windows_event(&mut self, event: WriteBehindEvent<String, Option<WindowFrame>>) {
        self.buffer(Buf::Windows, |w| &mut w.windows, Writer::write_windows, event);
    }

    /// Steps one write-behind buffer and carries its effects out; a batch is written at once
    /// and its result fed back.
    fn buffer<K: std::hash::Hash + Eq + Clone, V: Clone>(
        &mut self,
        buf: Buf,
        machine: fn(&mut Writer) -> &mut WriteBehindState<K, V>,
        write: fn(&mut Writer, &IndexMap<K, V>) -> rusqlite::Result<()>,
        event: WriteBehindEvent<K, V>,
    ) {
        let (timer, delay) = match buf {
            Buf::State => ("state", STATE_DELAY_MS),
            Buf::Docs => ("docs", DOCS_DELAY_MS),
            Buf::Windows => ("windows", WINDOWS_DELAY_MS),
        };
        let mut events = vec![event];
        while let Some(event) = events.pop() {
            let mut state = std::mem::take(machine(self));
            let effects = wb::step(&mut self.seq, &mut state, event, delay);
            *machine(self) = state;
            for effect in effects {
                match effect {
                    WriteBehindEffect::ArmTimer { attempt, delay_ms } => self.arm(timer, attempt, delay_ms),
                    WriteBehindEffect::CancelTimer => {
                        self.timers.insert(timer, None);
                    }
                    WriteBehindEffect::WriteBatch { attempt, batch } => {
                        let result = write(self, &batch);
                        events.push(match result {
                            Ok(()) => {
                                self.written += batch.len();
                                self.wal_event(WalEvent::Committed);
                                WriteBehindEvent::WriteSucceeded { attempt }
                            }
                            Err(e) => WriteBehindEvent::WriteFailed { attempt, error: e.to_string() },
                        });
                    }
                    WriteBehindEffect::ReportError(e) => eprintln!("opencode-storage: {timer} write failed, retrying: {e}"),
                    WriteBehindEffect::RejectClosed => {}
                }
            }
        }
    }

    fn write_state(&mut self, batch: &IndexMap<StateKey, Value>) -> rusqlite::Result<()> {
        let now = (self.clock)();
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for ((name, key), value) in batch {
            sql::put_state(&tx, name, key, value.as_deref(), now)?;
        }
        tx.commit()
    }

    fn write_windows(&mut self, batch: &IndexMap<String, Option<WindowFrame>>) -> rusqlite::Result<()> {
        let now = (self.clock)();
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (id, frame) in batch {
            sql::put_window(&tx, id, frame.as_ref(), now)?;
        }
        tx.commit()
    }

    /// Documents, their long strings split into chunks here (never on the caller's thread).
    /// A chunk this run touched within half the grace period isn't touched again.
    fn write_docs(&mut self, batch: &IndexMap<String, Value>) -> rusqlite::Result<()> {
        let now = (self.clock)();
        let mut fresh: Vec<String> = Vec::new();
        {
            let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            for (key, value) in batch {
                let Some(text) = value else {
                    sql::put_document(&tx, key, None)?;
                    continue;
                };
                let (stored, chunks) = sql::encode_text(text);
                for (id, data) in &chunks {
                    if self.touched.get(id).is_some_and(|t| now - t < BLOB_GRACE_MS / 2) {
                        continue;
                    }
                    if !sql::insert_blob(&tx, id, data, now)? {
                        sql::touch_blob(&tx, id, now)?;
                    }
                    fresh.push(id.clone());
                }
                sql::put_document(&tx, key, Some(&stored))?;
            }
            tx.commit()?;
        }
        for id in fresh {
            self.touched.insert(id, now);
        }
        let effects = draft_gc::step(&mut self.seq, &mut self.gc, DraftGcEvent::DocumentsWritten { at_ms: now });
        self.gc_effects(effects);
        Ok(())
    }

    /// A Tier 0 write: committed and synced before it returns.
    fn durable(&mut self, f: impl FnOnce(&Connection, i64) -> rusqlite::Result<()>) -> Result<(), StorageError> {
        let now = (self.clock)();
        let result = (|| {
            self.conn.execute_batch("PRAGMA synchronous = FULL")?;
            let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            f(&tx, now)?;
            tx.commit()
        })();
        self.conn.execute_batch("PRAGMA synchronous = NORMAL").ok();
        match result {
            Ok(()) => {
                self.wal_event(WalEvent::Committed);
                Ok(())
            }
            Err(e) => Err(StorageError(e.to_string())),
        }
    }

    fn flush(&mut self, reason: FlushReason, started: Instant) -> FlushReport {
        let quit = matches!(reason, FlushReason::Quit | FlushReason::SessionEnd | FlushReason::UpdateRestart);
        let pending = wb::has_pending(&self.state) || wb::has_pending(&self.docs) || wb::has_pending(&self.windows);
        if !pending && (!quit || self.wal == WalState::Clean) {
            return FlushReport { clean: true, micros: super::micros(started), ..FlushReport::default() };
        }
        self.written = 0;
        self.state_event(WriteBehindEvent::FlushRequested);
        self.docs_event(WriteBehindEvent::FlushRequested);
        self.windows_event(WriteBehindEvent::FlushRequested);
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
        self.wal_event(WalEvent::Committed);
        if let Some(done) = done {
            done();
        }
    }

    fn gc_effects(&mut self, effects: Effects<DraftGcEffect>) {
        for effect in effects {
            match effect {
                DraftGcEffect::ArmTimer { attempt, delay_ms } => self.arm("gc", attempt, u64::try_from(delay_ms).unwrap_or(0)),
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
                WalEffect::ArmTimer { attempt, delay_ms } => self.arm("wal", attempt, delay_ms),
                WalEffect::CancelTimer => {
                    self.timers.insert("wal", None);
                }
                WalEffect::Checkpoint => {
                    let busy = self.conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get::<_, i64>(0)).unwrap_or(1);
                    let event = if busy == 0 { WalEvent::Checkpointed } else { WalEvent::CheckpointFailed };
                    queue.extend(checkpoint::step(&mut self.seq, &mut self.wal, event));
                }
            }
        }
    }
}

fn answer(done: Option<crate::Done>, result: Result<(), StorageError>) {
    if let Some(done) = done {
        done(result);
    }
}

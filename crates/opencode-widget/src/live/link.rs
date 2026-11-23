//! What a live program's thread, its client and the shared clock thread share ([`Link`]), and
//! the program's run as an explicit machine:
//!
//! ```text
//!  start ─▶ Alive ──main returns / throws──────────────────────────────▶ Ended
//!            │ ├──paused 30 s──▶ Saving ──saved──▶ Saved ──resume──▶ Alive
//!            │ │                   └──can't save──▶ Discarding ──▶ Discarded ──resume──▶ Alive (from main)
//!            │ ├──50 ms without awaiting──────────────────────────────▶ Ended(Failed)
//!            │ └──stop──▶ Stopping ──────────────────────────────────────▶ Ended(Stopped)
//! ```
//!
//! Time is the program's running time: wall time less pauses. Timers are due in it, so a pause
//! freezes them with what they had left, and `now()` reads it.
//!
//! The 50 ms rule is the VM's (`Limits::slice_ms`, on the program thread's CPU time): a run that
//! computes that long without awaiting the host, host calls left out, ends with `SliceExceeded`.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::task::Waker;
use std::time::{Duration, Instant};

use psychoscript::{DiagnosticKind, HostResult, HostValue, Interrupted, ProgramOptions, RunOutcome, Snapshot, Suspender};
use serde_json::{Value, json};

use super::fetch::Rate;
use super::gpu::RunGpu;

mod queue;

use queue::Wait;
use super::{BURST, Ended, LiveProgram, Post, SAVE_AFTER, SLICE_MS, Setup};

/// The program's thread's stack: the run's own budget (512 KB) and room for the host's frames.
const STACK: usize = 4 << 20;

pub(super) enum Run {
    /// Its thread runs it, paused or not.
    Alive,
    /// Paused long: being saved to a snapshot.
    Saving,
    /// Saved; no thread.
    Saved(Snapshot),
    /// Couldn't be saved: being stopped, to start again from `main` on resume.
    Discarding,
    /// No thread; starts again from `main` on resume.
    Discarded,
    /// The client stopped it.
    Stopping,
    Ended(Ended),
}

impl Run {
    /// A thread runs (or is winding down).
    fn threaded(&self) -> bool {
        matches!(self, Run::Alive | Run::Saving | Run::Discarding | Run::Stopping)
    }
}

pub(super) struct St {
    pub run: Run,
    /// The thread's era: futures of an earlier one are let go.
    pub era: u64,
    /// Host calls throw the interrupt that ends the run.
    pub interrupt: bool,
    pub started: Instant,
    pub paused_total: Duration,
    pub pause: Option<Instant>,
    pub inbox: VecDeque<Value>,
    /// Receives waiting, in call order.
    pub waiters: VecDeque<(u64, Waker)>,
    pub tickets: u64,
    /// Each sleep's due time (running ms) by its call key, kept across a save.
    pub dues: HashMap<String, f64>,
    pub timers: HashMap<String, Waker>,
    /// Futures that came due while paused.
    pub held: Vec<Waker>,
    pub fetches: HashMap<u64, Waker>,
    pub fetched: HashMap<u64, HostResult>,
    pub fetch_ids: u64,
    pub rate: Rate,
    pub outbox: Vec<Post>,
    /// The post rate's bucket: its tokens, as of when (wall time: a pause doesn't stop it).
    pub bucket: (f64, Instant),
    /// What waits for the post rate, oldest first: `render` and `draw` posts for a token, and
    /// GPU batches behind them.
    pub waiting: VecDeque<Wait>,
    pub suspender: Option<Suspender>,
    /// Where in the program it threw, when it ended so (1-based line and column).
    pub failed_at: Option<(u32, u32)>,
    /// Native: the last rendered view's canvases by `String(key)`, their ids and whether `wgsl`
    /// draws them (kept across a save, as the program's view is).
    pub canvases: HashMap<String, (String, crate::tree::Drawn)>,
    /// A post broke the rules (a view of Keys and depth, an invalid render or draw): the run was
    /// interrupted to end with it.
    pub post_failure: Option<Ended>,
}

impl St {
    /// A future polled while paused waits here until the resume (once, however often it's polled).
    pub fn hold(&mut self, waker: &Waker) {
        if !self.held.iter().any(|w| w.will_wake(waker)) {
            self.held.push(waker.clone());
        }
    }

    /// The program's running time at `now`, in ms.
    pub fn running(&self, now: Instant) -> f64 {
        let mut d = now.saturating_duration_since(self.started).saturating_sub(self.paused_total);
        if let Some(p) = self.pause {
            d = d.saturating_sub(now.saturating_duration_since(p));
        }
        d.as_secs_f64() * 1000.0
    }

    /// The bucket refilled up to `now`, at `per_second`.
    fn refill(&mut self, now: Instant, per_second: f64) {
        let (tokens, at) = self.bucket;
        self.bucket = ((tokens + now.saturating_duration_since(at).as_secs_f64() * per_second).min(BURST), now);
    }

    /// Whether the bucket has a token (fractional tokens: within 1e-9 of one counts).
    fn token(&self) -> bool {
        self.bucket.0 >= 1.0 - 1e-9
    }

    /// `post` goes to the client; whether it's the first waiting there (the client is to hear).
    fn out(&mut self, post: Post) -> bool {
        let was_empty = self.outbox.is_empty();
        // A view or a canvas drawn again before the client took the last one replaces it.
        let replaces = match (self.outbox.last(), &post) {
            (Some(Post::Render(_)), Post::Render(_)) => true,
            (Some(Post::Draw { id: a, .. }), Post::Draw { id: b, .. }) => a == b,
            _ => false,
        };
        if replaces {
            self.outbox.pop();
        }
        self.outbox.push(post);
        was_empty
    }

    /// Wakes every future (to see the interrupt, or a pause end).
    fn wake_all(&mut self) {
        for (_, w) in &self.waiters {
            w.wake_by_ref();
        }
        for w in self.timers.values().chain(self.fetches.values()).chain(self.held.iter()) {
            w.wake_by_ref();
        }
        self.held.clear();
    }
}

pub(super) struct Link {
    st: Mutex<St>,
    pub news: AtomicBool,
    pub program: Arc<LiveProgram>,
    pub data: Value,
    pub setup: Setup,
    /// The post rate, tokens a second.
    per_second: f64,
    /// The run's GPU canvases, made on its thread when it starts from `main`.
    gpu: Mutex<Option<Arc<RunGpu>>>,
    /// Held while GPU work is applied, so batches replay in the order they were posted, whether
    /// at once, from the queue, or at the run's end.
    order: Mutex<()>,
    me: Weak<Link>,
}


impl Link {
    pub fn new(program: Arc<LiveProgram>, data: Value, setup: Setup, per_second: f64) -> Arc<Link> {
        let now = Instant::now();
        let st = St {
            run: Run::Discarded,
            era: 0,
            interrupt: false,
            started: now,
            paused_total: Duration::ZERO,
            pause: None,
            inbox: VecDeque::new(),
            waiters: VecDeque::new(),
            tickets: 0,
            dues: HashMap::new(),
            timers: HashMap::new(),
            held: Vec::new(),
            fetches: HashMap::new(),
            fetched: HashMap::new(),
            fetch_ids: 0,
            rate: Rate::default(),
            outbox: Vec::new(),
            bucket: (BURST, now),
            waiting: VecDeque::new(),
            suspender: None,
            failed_at: None,
            canvases: HashMap::new(),
            post_failure: None,
        };
        Arc::new_cyclic(|me| Link { st: Mutex::new(st), news: AtomicBool::new(false), program, data, setup, per_second, gpu: Mutex::default(), order: Mutex::default(), me: me.clone() })
    }

    pub fn lock(&self) -> MutexGuard<'_, St> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn signal(&self) {
        self.news.store(true, Ordering::Release);
        (self.setup.notify)();
    }

    /// Starts the program from `main`.
    pub fn start(&self) {
        let mut st = self.lock();
        self.spawn(&mut st, None);
    }

    /// A thread for the run: from `main`, or from `snapshot`.
    fn spawn(&self, st: &mut St, snapshot: Option<Snapshot>) {
        let Some(link) = self.me.upgrade() else { return };
        // Built for another protocol: it never runs.
        if let Err(end) = &self.program.compiled {
            st.run = Run::Ended(end.clone());
            return;
        }
        let now = Instant::now();
        if snapshot.is_none() {
            // `main` again: its clock and its timers start over.
            st.started = now;
            st.paused_total = Duration::ZERO;
            st.dues.clear();
            st.canvases.clear();
            // The last run's GPU work goes with it.
            st.waiting.retain(|w| matches!(w, Wait::Counted(..)));
        }
        st.era += 1;
        st.run = Run::Alive;
        st.interrupt = false;
        st.suspender = None;
        st.post_failure = None;
        let era = st.era;
        let spawned = std::thread::Builder::new().name("widget-live".into()).stack_size(STACK).spawn(move || run(link, era, snapshot));
        if let Err(e) = spawned {
            st.run = Run::Ended(Ended::Broke(format!("couldn't start the widget: {e}")));
            return;
        }
        clock().watch(self.me.clone());
    }

    /// The run of `era` ended: with its outcome, saved, or interrupted.
    fn finished(&self, era: u64, result: Result<RunOutcome, Interrupted>, failure: Option<String>) {
        // The last view is never lost: what waits for the rate goes out before the end (the GPU's
        // batches replayed), unless the client stopped the run or it's only being saved.
        let pending = {
            let mut st = self.lock();
            if st.era != era {
                return;
            }
            let keeps = matches!(st.run, Run::Stopping | Run::Discarding) || (matches!(st.run, Run::Saving) && matches!(result, Ok(RunOutcome::Suspended(_))));
            if keeps { VecDeque::new() } else { std::mem::take(&mut st.waiting) }
        };
        self.flush(pending);
        let mut st = self.lock();
        if st.era != era {
            return;
        }
        let mut at = None;
        let mut ended = |result: Result<RunOutcome, Interrupted>| match result {
            Ok(RunOutcome::Done(o)) => match o.json() {
                Ok(_) => Ended::Returned,
                // Over the heap cap: a program error, where it happened is no line of its own.
                Err(d) if d.kind == DiagnosticKind::MemoryLimitExceeded => Ended::Failed(crate::runner::heap_exceeded()),
                // Over the slice: no line of its own either.
                Err(d) if d.kind == DiagnosticKind::SliceExceeded => Ended::Failed(format!("The widget ran for more than {SLICE_MS} ms without awaiting, so it was stopped.")),
                Err(d) => {
                    at = d.location;
                    Ended::Failed(d.message_lossy())
                }
            },
            Ok(RunOutcome::Suspended(_)) => Ended::Broke("the widget stopped".into()),
            Err(_) => Ended::Stopped,
        };
        let run = std::mem::replace(&mut st.run, Run::Discarded);
        st.run = match (run, result) {
            _ if failure.is_some() => Run::Ended(Ended::Broke(failure.unwrap_or_default())),
            _ if st.post_failure.is_some() => Run::Ended(st.post_failure.take().unwrap_or(Ended::Stopped)),
            (Run::Saving, Ok(RunOutcome::Suspended(s))) => Run::Saved(s.state),
            (Run::Discarding, _) => Run::Discarded,
            (Run::Stopping, _) => Run::Ended(Ended::Stopped),
            (_, result) => Run::Ended(ended(result)),
        };
        if matches!(st.run, Run::Ended(Ended::Stopped)) {
            st.waiting.clear();
        }
        st.failed_at = at;
        st.suspender = None;
        st.waiters.clear();
        st.timers.clear();
        st.fetches.clear();
        st.fetched.clear();
        st.held.clear();
        // Resumed while it was being saved or let go: it goes on at once.
        if st.pause.is_none() {
            match std::mem::replace(&mut st.run, Run::Discarded) {
                Run::Saved(s) => self.spawn(&mut st, Some(s)),
                Run::Discarded => {
                    st.inbox.clear();
                    self.spawn(&mut st, None);
                }
                other => st.run = other,
            }
        }
        drop(st);
        self.signal();
    }

    /// A message for the program; it wakes the first receive waiting, unless paused.
    pub fn deliver(&self, message: Value) {
        let mut st = self.lock();
        if matches!(st.run, Run::Ended(_)) {
            return;
        }
        st.inbox.push_back(message);
        if st.pause.is_none()
            && let Some((_, w)) = st.waiters.front()
        {
            w.wake_by_ref();
        }
    }

    /// The program posted `post` (on its thread).
    /// A render's view broke Keys and depth: the run ends at once with it (phase `view`). What
    /// `post` returns to stop the run.
    pub fn fail_view(&self, message: String) -> HostValue {
        self.lock().post_failure = Some(Ended::View(message));
        HostValue::Interrupt
    }

    /// The program posted an invalid render or draw (`message` says why): a program error that
    /// ends the run at once (`widgets-live.md`). What `post` throws to stop the run.
    pub fn fail_post(&self, message: String) -> HostValue {
        self.lock().post_failure = Some(Ended::Failed(message));
        HostValue::Interrupt
    }

    /// The program posted `post`, not counted toward the post rate: it goes to the client at once,
    /// ahead of what waits for the rate. A `submit` or `dismiss` lets what waits go first; a frame
    /// request while one waits for the client changes nothing.
    pub fn post(&self, post: Post) {
        if matches!(post, Post::Submit(_) | Post::Dismiss) {
            let pending = std::mem::take(&mut self.lock().waiting);
            self.flush(pending);
        }
        let mut st = self.lock();
        let first = st.outbox.is_empty();
        if post == Post::Frame && st.outbox.contains(&Post::Frame) {
            return;
        }
        st.out(post);
        drop(st);
        if first {
            self.signal();
        }
    }

    pub fn take(&self) -> Vec<Post> {
        let mut st = self.lock();
        self.news.store(false, Ordering::Release);
        std::mem::take(&mut st.outbox)
    }

    pub fn ended(&self) -> Option<Ended> {
        match &self.lock().run {
            Run::Ended(e) => Some(e.clone()),
            _ => None,
        }
    }

    pub fn failed_at(&self) -> Option<(u32, u32)> {
        self.lock().failed_at
    }

    pub fn is_paused(&self) -> bool {
        self.lock().pause.is_some()
    }

    pub fn now_ms(&self) -> f64 {
        self.lock().running(Instant::now())
    }

    pub fn pause(&self) {
        let mut st = self.lock();
        if st.pause.is_some() || matches!(st.run, Run::Ended(_)) {
            return;
        }
        st.pause = Some(Instant::now());
        drop(st);
        clock().poke();
    }

    pub fn resume(&self) {
        let mut st = self.lock();
        let now = Instant::now();
        let Some(since) = st.pause.take() else { return };
        let paused = now.saturating_duration_since(since);
        st.paused_total += paused;
        let resumed = json!({ "type": "resumed", "pausedMs": paused.as_secs_f64() * 1000.0 });
        match std::mem::replace(&mut st.run, Run::Discarded) {
            Run::Saved(s) => {
                st.inbox.push_front(resumed);
                self.spawn(&mut st, Some(s));
            }
            Run::Discarded => {
                st.inbox.clear();
                self.spawn(&mut st, None);
            }
            run @ Run::Ended(_) => st.run = run,
            run => {
                st.run = run;
                st.inbox.push_front(resumed);
                let held = std::mem::take(&mut st.held);
                for w in held {
                    w.wake();
                }
                if let Some((_, w)) = st.waiters.front() {
                    w.wake_by_ref();
                }
            }
        }
        drop(st);
        clock().poke();
    }

    pub fn stop(&self) {
        let mut st = self.lock();
        match st.run {
            Run::Ended(_) | Run::Stopping => return,
            Run::Saved(_) | Run::Discarded => {
                st.run = Run::Ended(Ended::Stopped);
                st.waiting.clear();
            }
            _ => st.run = Run::Stopping,
        }
        st.interrupt = true;
        st.wake_all();
    }

    /// The clock's look at `now`: due timers fire, a long pause saves the program. When it wants to
    /// look again.
    fn tick(&self, now: Instant) -> Option<Instant> {
        let mut st = self.lock();
        if !st.run.threaded() {
            return None;
        }
        let mut next: Option<Instant> = None;
        let mut soon = |at: Instant| next = Some(next.map_or(at, |n: Instant| n.min(at)));
        if let Some(since) = st.pause {
            // Paused, timers don't fire. A run paused that long is saved at its next step: no host
            // call completes for it while paused, so it's awaiting one, or the VM stopped it.
            if matches!(st.run, Run::Alive) {
                let at = since + SAVE_AFTER;
                if now >= at {
                    st.run = Run::Saving;
                    self.ask(&mut st);
                } else {
                    soon(at);
                }
            }
        }
        let running = st.running(now);
        let due: Vec<String> = if st.pause.is_some() { Vec::new() } else { st.timers.keys().filter(|k| st.dues.get(*k).is_none_or(|d| *d <= running)).cloned().collect() };
        for k in due {
            if let Some(w) = st.timers.remove(&k) {
                w.wake();
            }
        }
        if let Some(ms) = st.timers.keys().filter_map(|k| st.dues.get(k)).copied().reduce(f64::min).filter(|_| st.pause.is_none()) {
            soon(now + Duration::from_secs_f64(((ms - running) / 1000.0).max(0.0)));
        }
        next
    }

    /// Asks the run to suspend at its next step, to save it, off this thread.
    fn ask(&self, st: &mut St) {
        let Some(suspender) = st.suspender.clone() else {
            if matches!(st.run, Run::Saving) {
                self.discard(st);
            }
            return;
        };
        let link = self.me.clone();
        let asked = std::thread::Builder::new().name("widget-suspend".into()).spawn(move || {
            if suspender.suspend(None).is_err()
                && let Some(link) = link.upgrade()
            {
                let mut st = link.lock();
                if matches!(st.run, Run::Saving) {
                    link.discard(&mut st);
                }
            }
        });
        if asked.is_err() && matches!(st.run, Run::Saving) {
            self.discard(st);
        }
    }

    /// Saving failed: the run is stopped, to start again from `main` on resume.
    fn discard(&self, st: &mut St) {
        st.run = Run::Discarding;
        st.interrupt = true;
        st.wake_all();
    }

    /// A fetch came back: its future wakes, or is held while paused.
    pub fn fetched(&self, id: u64, result: HostResult, failure: Option<String>) {
        if let Some(message) = failure {
            self.post(Post::Failed { phase: "fetch", message });
        }
        let mut st = self.lock();
        st.rate.in_flight = st.rate.in_flight.saturating_sub(1);
        let Some(w) = st.fetches.remove(&id) else { return };
        st.fetched.insert(id, result);
        if st.pause.is_some() {
            st.held.push(w);
        } else {
            w.wake();
        }
    }
}

/// The program's thread: runs it (from `main` or a snapshot) until it ends or is suspended.
fn run(link: Arc<Link>, era: u64, snapshot: Option<Snapshot>) {
    // Its GPU canvases: a device the first time, the same again for a run restored from its
    // snapshot.
    match (&snapshot, link.gpu()) {
        (None, _) => link.start_gpu(),
        (Some(_), Some(gpu)) => gpu.restore(),
        (Some(_), None) => {}
    }
    let runtime = super::host::runtime(&link, era);
    let program = match &snapshot {
        Some(s) => match runtime.resume(s) {
            Ok(p) => p,
            Err(e) => {
                link.finished(era, Err(Interrupted), Some(format!("The widget couldn't continue where it paused: {e}")));
                return;
            }
        },
        None => match &link.program.compiled {
            Ok(compiled) => runtime.program(compiled, &ProgramOptions::new()),
            Err(_) => return,
        },
    };
    {
        let mut st = link.lock();
        if st.era == era {
            st.suspender = Some(program.suspender());
            // Asked to save before it could be asked: it is asked now.
            if matches!(st.run, Run::Saving) {
                link.ask(&mut st);
            }
        }
    }
    let result = program.run_resumable();
    drop(runtime);
    // Saved to a snapshot: its GPU goes with the VM (`widgets-gpu.md`, A run restored from a
    // snapshot).
    let saving = matches!(result, Ok(RunOutcome::Suspended(_))) && {
        let st = link.lock();
        st.era == era && matches!(st.run, Run::Saving)
    };
    if saving && let Some(gpu) = link.gpu() {
        gpu.save();
    }
    link.finished(era, result, None);
}

/// The one thread that keeps every live program's time: timers, saves, the post rate.
struct Clock {
    m: Mutex<(Vec<Weak<Link>>, bool)>,
    cv: Condvar,
}

fn clock() -> &'static Clock {
    static CLOCK: OnceLock<&'static Clock> = OnceLock::new();
    CLOCK.get_or_init(|| {
        let c: &'static Clock = Box::leak(Box::new(Clock { m: Mutex::new((Vec::new(), false)), cv: Condvar::new() }));
        std::thread::Builder::new().name("widget-clock".into()).spawn(move || c.run()).ok();
        c
    })
}

impl Clock {
    fn lock(&self) -> MutexGuard<'_, (Vec<Weak<Link>>, bool)> {
        self.m.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn watch(&self, link: Weak<Link>) {
        let mut g = self.lock();
        if !g.0.iter().any(|l| l.ptr_eq(&link)) {
            g.0.push(link);
        }
        g.1 = true;
        self.cv.notify_one();
    }

    /// Look again now: a timer or a pause changed.
    pub fn poke(&self) {
        self.lock().1 = true;
        self.cv.notify_one();
    }

    fn run(&self) {
        loop {
            let links: Vec<Arc<Link>> = {
                let mut g = self.lock();
                g.1 = false;
                g.0.retain(|l| l.strong_count() > 0);
                g.0.iter().filter_map(Weak::upgrade).collect()
            };
            let now = Instant::now();
            let mut next: Option<Instant> = None;
            for l in &links {
                for at in [l.release_due(now), l.tick(now)].into_iter().flatten() {
                    next = Some(next.map_or(at, |n| n.min(at)));
                }
            }
            drop(links);
            let g = self.lock();
            if g.1 {
                continue;
            }
            let _g = match next {
                Some(at) => self.cv.wait_timeout(g, at.saturating_duration_since(Instant::now())).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0),
                None => self.cv.wait(g).unwrap_or_else(PoisonError::into_inner),
            };
        }
    }
}

pub(super) fn poke() {
    clock().poke();
}

//! The updater as a host service: plain data in, plain data out, so any UI can drive it (today
//! the app's own Settings section and sidebar pill; later a Luau plugin drawing Desktop's
//! updater UI).
//!
//! - **State** ([`Snapshot`]): the machine's view (`UpdaterState`: disabled, idle, checking,
//!   downloading, up to date, ready, installing, error), the download's [`Progress`], and the
//!   running version and feed channel. [`UpdaterService::revision`] grows whenever any of it
//!   changes.
//! - **Actions**: [`UpdaterService::check`] and [`UpdaterService::install`] each return a
//!   [`Ticket`]; [`UpdaterService::take_answer`] gives its [`Answer`] once the machine settles it
//!   (what the check found, or whether the install handed off), which is what Desktop's toasts
//!   report.
//! - **Lifecycle**: [`UpdaterService::poll`] on the app's thread feeds the worker's results to the
//!   machine and costs nothing at rest; [`UpdaterService::take_quit`] says the app must quit into
//!   the update; [`UpdaterService::quitting`] installs a ready update as the app quits.
//!
//! One worker thread runs the platform's blocking work and the schedule. It first makes the
//! platform updater (it reads the install, and on Windows the running exe's signature, so none
//! of that is on the UI thread), then runs feed reads, downloads, verification, the `ready`
//! record and the handoff, with the first check [`feed::initial_delay`] after it's up, then every
//! [`feed::INTERVAL`].

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use rune::time::Instant;

use rune::Waker;

use super::feed;
use super::machine::{self, Attempt, Effect, Event, Plan, State, View};
use super::platform::Platform;

/// A caller's check or install, answered by [`UpdaterService::take_answer`].
pub type Ticket = u64;

/// Makes the platform updater on the worker thread, with the folder it keeps its records in;
/// `None` when this build has no updater.
pub type MakePlatform = Box<dyn FnOnce() -> Option<(Arc<dyn Platform>, PathBuf)> + Send>;

/// The download under way: bytes so far of `total`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub downloaded: u64,
    pub total: u64,
}

/// Everything a UI draws of the updater.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub state: View,
    /// While downloading.
    pub progress: Option<Progress>,
    /// The running build's version, as the feed names releases.
    pub version: String,
    /// The feed channel (`dev`, `stable`).
    pub channel: String,
}

/// How a caller's request settled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// What the check found: up to date, ready, an error, or the work already under way.
    Checked(View),
    /// The install handed off and the app quits (`Ok`), or why it couldn't.
    Installed(Result<(), String>),
}

enum Job {
    Check(Attempt),
    Stage(Attempt),
    Install(Attempt),
    /// Records the staged release's version (`ready`), or forgets it.
    Persist(Option<String>),
}

enum Message {
    /// The worker made the platform updater; the event is `Event::Started`.
    Started(Arc<dyn Platform>, Event),
    /// The schedule's check is due.
    Tick,
    Event(Event),
}

pub struct UpdaterService {
    state: State,
    snapshot: Snapshot,
    revision: u64,
    /// Set once, by `Message::Started`: the machine is past `Starting` and this build updates.
    platform: OnceCell<Arc<dyn Platform>>,
    jobs: Sender<Job>,
    messages: Receiver<Message>,
    /// Bytes downloaded and the total, written by the worker.
    progress: Arc<[AtomicU64; 2]>,
    wakers: Arc<Mutex<Vec<Waker>>>,
    next: Ticket,
    /// The service's own requests (the schedule's checks, auto-install): nobody takes their answers.
    own: HashSet<Ticket>,
    answers: HashMap<Ticket, Answer>,
    quit: bool,
}

impl UpdaterService {
    /// The service, starting: its worker makes the platform updater with `make` off the UI
    /// thread, and `waker` hears when it's up. `version` is this build's until then.
    pub fn start(make: MakePlatform, version: &str, channel: String, waker: Option<Waker>) -> Self {
        let state = machine::init();
        let (jobs, job_rx) = std::sync::mpsc::channel();
        let (msg_tx, messages) = std::sync::mpsc::channel();
        let progress = Arc::new([AtomicU64::new(0), AtomicU64::new(0)]);
        let wakers = Arc::new(Mutex::new(waker.into_iter().collect()));
        let worker = Worker { messages: msg_tx, wakers: wakers.clone(), progress: progress.clone(), last_wake: Mutex::new(Instant::now()) };
        std::thread::Builder::new().name("opencode-updater".into()).spawn(move || worker.run(make, job_rx)).ok();
        let snapshot = Snapshot { state: machine::view(&state), progress: None, version: version.to_owned(), channel };
        UpdaterService { state, snapshot, revision: 0, platform: OnceCell::new(), jobs, messages, progress, wakers, next: 0, own: HashSet::new(), answers: HashMap::new(), quit: false }
    }

    /// Wakes waker's event loop whenever the worker has news (each window's).
    pub fn add_waker(&self, waker: Waker) {
        if let Ok(mut w) = self.wakers.lock() {
            w.push(waker);
        }
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// Grows whenever the snapshot changes or an answer lands.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Checks the feed now; a ready update stays ready while the check runs.
    pub fn check(&mut self) -> Ticket {
        let t = self.ticket();
        self.dispatch(Event::CheckRequested(t));
        t
    }

    /// Restarts into the ready update (a last check first, which may download a newer one).
    pub fn install(&mut self) -> Ticket {
        let t = self.ticket();
        self.dispatch(Event::InstallRequested(t));
        t
    }

    /// Ticket `t`'s answer, once.
    pub fn take_answer(&mut self, t: Ticket) -> Option<Answer> {
        self.answers.remove(&t)
    }

    /// Stops waiting on ticket `t`; the shared work carries on.
    pub fn forget(&mut self, t: Ticket) {
        self.answers.remove(&t);
        self.dispatch(Event::WaiterCancelled(t));
    }

    /// Whether an install handed off, without taking it.
    pub fn quit_pending(&self) -> bool {
        self.quit
    }

    /// True once: an install handed off, so the app must quit now.
    pub fn take_quit(&mut self) -> bool {
        std::mem::take(&mut self.quit)
    }

    /// Whether that quit starts the app again ([`Platform::relaunches`]).
    pub fn relaunches(&self) -> bool {
        self.platform.get().is_some_and(|p| p.relaunches())
    }

    /// Feeds the worker's results to the machine; true when the snapshot changed. At rest: one
    /// empty channel read and two atomic loads, no allocation.
    pub fn poll(&mut self) -> bool {
        let before = self.revision;
        loop {
            match self.messages.try_recv() {
                Ok(Message::Started(platform, event)) => {
                    self.snapshot.version = platform.version().to_owned();
                    self.platform.set(platform).ok();
                    self.revision += 1;
                    self.dispatch(event);
                }
                Ok(Message::Tick) => {
                    let t = self.ticket();
                    self.own.insert(t);
                    self.dispatch(Event::CheckRequested(t));
                }
                Ok(Message::Event(event)) => self.dispatch(event),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        let progress = matches!(self.snapshot.state, View::Downloading(_)).then(|| Progress { downloaded: self.progress[0].load(Ordering::Relaxed), total: self.progress[1].load(Ordering::Relaxed) });
        if progress != self.snapshot.progress {
            self.snapshot.progress = progress;
            self.revision += 1;
        }
        if self.platform.get().is_some_and(|p| p.auto_install()) && matches!(self.snapshot.state, View::Ready(_)) {
            let t = self.ticket();
            self.own.insert(t);
            self.dispatch(Event::InstallRequested(t));
        }
        self.revision != before || self.quit
    }

    /// The app quits: a ready update not already installing goes in on the way out.
    pub fn quitting(&mut self) {
        if matches!(self.state, State::Ready { refresh: None, .. })
            && let Some(platform) = self.platform.get()
        {
            platform.install_on_quit();
        }
        self.dispatch(Event::Disposed);
    }

    fn ticket(&mut self) -> Ticket {
        self.next += 1;
        self.next
    }

    fn dispatch(&mut self, event: Event) {
        let (next, effects) = machine::transition(&self.state, event);
        self.state = next;
        let view = machine::view(&self.state);
        if view != self.snapshot.state && !matches!(self.state, State::Disposed(_)) {
            eprintln!("updater: {:?} -> {view:?}", self.snapshot.state);
            if matches!(view, View::Downloading(_)) {
                self.progress[0].store(0, Ordering::Relaxed);
                self.progress[1].store(0, Ordering::Relaxed);
            }
            self.snapshot.state = view;
            self.revision += 1;
        }
        for effect in effects {
            self.run(effect);
        }
    }

    fn run(&mut self, effect: Effect) {
        match effect {
            Effect::Check(a) => self.send(Job::Check(a)),
            Effect::Stage(a) => self.send(Job::Stage(a)),
            Effect::Restart(a) => self.send(Job::Install(a)),
            // A blocking platform call can't be stopped; its stale result is fenced by its attempt.
            Effect::Cancel(_) => {}
            Effect::PersistSet(version) => self.send(Job::Persist(Some(version))),
            Effect::PersistClear => self.send(Job::Persist(None)),
            Effect::ResolveChecks(tickets, view) => self.answer(tickets, |_| Answer::Checked(view.clone())),
            Effect::ResolveInstalls(tickets) => {
                self.answer(tickets, |_| Answer::Installed(Ok(())));
                if matches!(self.state, State::Installing { plan: Plan::Quitting, .. }) {
                    eprintln!("updater: handed off to the update; quitting");
                    self.quit = true;
                }
            }
            Effect::RejectInstalls(tickets, why) => {
                eprintln!("updater: install failed: {why}");
                self.answer(tickets, |_| Answer::Installed(Err(why.clone())));
            }
            Effect::Log(_, message) => eprintln!("updater: {message}"),
        }
    }

    fn answer(&mut self, tickets: Vec<Ticket>, answer: impl Fn(Ticket) -> Answer) {
        for t in tickets {
            if !self.own.remove(&t) {
                self.answers.insert(t, answer(t));
                self.revision += 1;
            }
        }
    }

    fn send(&self, job: Job) {
        self.jobs.send(job).ok();
    }
}

/// The worker thread: makes the platform updater, then runs its blocking calls and the check
/// schedule.
struct Worker {
    messages: Sender<Message>,
    wakers: Arc<Mutex<Vec<Waker>>>,
    progress: Arc<[AtomicU64; 2]>,
    last_wake: Mutex<Instant>,
}

impl Worker {
    fn run(self, make: MakePlatform, jobs: Receiver<Job>) {
        let started = Instant::now();
        let Some((platform, updates)) = make() else {
            eprintln!("updater: none in this build (found in {} µs, off the UI thread)", started.elapsed().as_micros());
            self.send(Message::Event(Event::Unavailable));
            return;
        };
        let persisted = std::fs::read_to_string(updates.join("ready")).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        eprintln!("updater: up in {} µs, off the UI thread", started.elapsed().as_micros());
        self.send(Message::Started(platform.clone(), Event::Started { current: platform.version().to_owned(), persisted }));
        let mut next_check = Instant::now() + feed::initial_delay();
        loop {
            match jobs.recv_timeout(next_check.saturating_duration_since(Instant::now())) {
                Err(RecvTimeoutError::Timeout) => {
                    self.send(Message::Tick);
                    next_check = Instant::now() + feed::INTERVAL;
                }
                Err(RecvTimeoutError::Disconnected) => return,
                Ok(Job::Check(a)) => {
                    let event = match platform.check() {
                        Ok(target) => Event::CheckSucceeded(a, target),
                        Err(e) => Event::CheckFailed(a, e),
                    };
                    self.send(Message::Event(event));
                }
                Ok(Job::Stage(a)) => {
                    let event = match platform.stage(&|done, total| self.downloaded(done, total)) {
                        Ok(()) => Event::StageSucceeded(a),
                        Err(e) => Event::StageFailed(a, e),
                    };
                    self.send(Message::Event(event));
                }
                Ok(Job::Install(a)) => {
                    let event = match platform.install() {
                        Ok(()) => Event::RestartSucceeded(a),
                        Err(e) => Event::RestartFailed(a, e),
                    };
                    self.send(Message::Event(event));
                }
                Ok(Job::Persist(Some(version))) => {
                    std::fs::create_dir_all(&updates).ok();
                    std::fs::write(updates.join("ready"), format!("{version}\n")).ok();
                }
                Ok(Job::Persist(None)) => {
                    std::fs::remove_file(updates.join("ready")).ok();
                }
            }
        }
    }
    /// Records the download's progress, waking the app at most ten times a second.
    fn downloaded(&self, done: u64, total: u64) {
        self.progress[0].store(done, Ordering::Relaxed);
        self.progress[1].store(total, Ordering::Relaxed);
        let Ok(mut last) = self.last_wake.lock() else { return };
        if last.elapsed() >= Duration::from_millis(100) || done == total {
            *last = Instant::now();
            self.wake();
        }
    }

    fn send(&self, message: Message) {
        self.messages.send(message).ok();
        self.wake();
    }

    fn wake(&self) {
        if let Ok(wakers) = self.wakers.lock() {
            wakers.iter().for_each(|w| w());
        }
    }
}

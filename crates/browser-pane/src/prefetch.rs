//! The one engine download job of the process, shared by every window's host: started quietly
//! once a launch is over and finds no engine this build accepts (`Mode::Launch`), by a page that
//! waits for the engine (`Mode::Urgent`), or once a run after the first page opens to fetch a
//! newer release (`Mode::Update`), and never twice. A page that opens while it runs joins it and
//! shows where it is.
//!
//! A launch's fetch runs on a background-priority thread and says nothing to the windows: it
//! waits while the OS reports a metered connection or its battery saver, leaves the disk some
//! room, waits for the network when the source is out of reach, and stops for the run when a try
//! can't help (no room, bytes that don't check out twice). An update also runs throttled and
//! waits on battery power. When a page waits the job goes on at full speed and normal priority,
//! and what it does shows in the pane.
//!
//! The job reads the source's index, then streams the archive into place: the bytes land in a
//! partial file that a later try (or a later run) resumes, unpack as they arrive, and the folder
//! moves into place only once the archive's hash and the trust rule pass. The partial file is
//! locked while it downloads, so a second copy of the app never downloads it too: it follows the
//! first one's progress instead. A connection that goes silent is dropped (`download::Cut::Stalled`)
//! and the download goes on from the partial file: at once the first time, and at once for a page
//! that began to wait meanwhile, with the backoff after that.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::Duration;
use rune::time::Instant;

use rune::Waker;

use crate::background;
use crate::download::{Body, Cut, Pace};
use crate::installer::{self, ENGINE_BUNDLE_NAME, EngineStatus, Failure, Fault, err_write};
use crate::machine::{EngineEffect, EngineEvent, SharedEngine};
use crate::source::{self, Release};
use crate::trust::Verifier;
use crate::unpack;

/// An update's bandwidth: 2 MB/s, about a minute for the whole engine.
const BACKGROUND_RATE: u64 = 2_000_000;
/// How often a running quiet download asks the OS whether to pause.
const PAUSE_CHECK: Duration = Duration::from_secs(5);
/// How often a paused download asks again.
const PAUSED_RECHECK: Duration = Duration::from_secs(60);
/// A quiet job's tries at a source that answers with errors before it waits for the next run.
const MAX_ATTEMPTS: u32 = 6;
/// What a launch's fetch leaves free on the disk beyond what the engine needs: 1 GiB. A page
/// that waits installs with less.
const LAUNCH_HEADROOM: u64 = 1 << 30;
/// How long a running app waits between two reads of an index that publishes nothing for it.
const PROBE_RECHECK: Duration = Duration::from_secs(60 * 60);
/// How often a page that waits reads the progress of another app's download.
const FOLLOW_EVERY: Duration = Duration::from_millis(500);
/// How long the keyboard and mouse must have been still before a new engine's files are loaded.
const WARM_QUIET: Duration = Duration::from_secs(2);
/// How long the warm-up waits for that stillness before it leaves the first load to a page.
const WARM_PATIENCE: Duration = Duration::from_secs(60);
/// How long the warm-up's process may take (it takes a few seconds on a slow disk).
const WARM_LIMIT: Duration = Duration::from_secs(60);
/// How often a quiet job looks whether another app's download ended.
const BUSY_RECHECK: Duration = Duration::from_secs(5);
/// How long another app's download may show no new byte before this job stops following it.
const FOLLOW_STALLED: Duration = Duration::from_secs(90);

static STAGING_SEQ: AtomicU64 = AtomicU64::new(1);

/// What a job is for, which sets its pace and what it reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// A page waits for the engine: full speed, normal priority, no pausing, one try, and every
    /// step shows in the pane.
    Urgent,
    /// The app is up and no engine this build accepts is installed: full speed on a
    /// background-priority thread, quiet (no status, no wake), retried for as long as the
    /// network is the reason.
    Launch,
    /// An engine runs; a newer release installs beside it for its next start, throttled, without
    /// touching the engine's status.
    Update,
}

/// The job's state (one per process).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Running(Mode),
}

#[derive(Clone, Copy, Debug)]
enum Event {
    Start(Mode),
    /// The source lists no engine for this build: the job stays, quietly, to read it again.
    Unlisted,
    Finished,
}

#[derive(Debug, PartialEq)]
enum Effect {
    Spawn(Mode),
    /// Wake the running job's waits.
    Nudge,
}

fn step(state: State, event: Event) -> (State, Option<Effect>) {
    match (state, event) {
        (State::Idle, Event::Start(mode)) => (State::Running(mode), Some(Effect::Spawn(mode))),
        (State::Idle, _) => (State::Idle, None),
        (State::Running(_), Event::Start(Mode::Urgent)) => (State::Running(Mode::Urgent), Some(Effect::Nudge)),
        (State::Running(_), Event::Unlisted) => (State::Running(Mode::Launch), None),
        (_, Event::Finished) => (State::Idle, None),
        (other, _) => (other, None),
    }
}

/// A host whose page waits for the job: it hears every status.
struct Watcher {
    shared: Arc<Mutex<SharedEngine>>,
    waker: Option<Waker>,
}

struct Ctl {
    run: State,
    watchers: Vec<Watcher>,
    /// Every host that asked this run (a window that closed drops out): they hear whether the
    /// source publishes an engine, and that a quiet job installed one.
    hosts: Vec<(Weak<Mutex<SharedEngine>>, Option<Waker>)>,
    /// Where the job is: what a page that joins now shows.
    shown: EngineStatus,
    /// Counts the network's changes: a wait for the network ends at the next one.
    kicks: u64,
    /// A launch asked: a source that publishes nothing is read again every hour.
    launched: bool,
}

impl Ctl {
    /// Steps every live host with `event`; the wakers of those it changed.
    fn each_host(&mut self, event: impl Fn() -> EngineEvent) -> Vec<Waker> {
        self.hosts.retain(|(host, _)| host.strong_count() > 0);
        self.hosts
            .iter()
            .filter_map(|(host, waker)| {
                let host = host.upgrade()?;
                tell(&host, waker.as_ref(), event())
            })
            .collect()
    }

    /// Gives every waiting page `status` and lets them go; the wakers of those it changed.
    fn release_watchers(&mut self, status: &EngineStatus) -> Vec<Waker> {
        std::mem::take(&mut self.watchers)
            .iter()
            .filter_map(|w| tell(&w.shared, w.waker.as_ref(), EngineEvent::InstallStatus(status.clone())))
            .collect()
    }
}

pub(crate) struct Prefetch {
    ctl: Mutex<Ctl>,
    /// Wakes the job's waits early (a page waits now, the network changed).
    nudge: Condvar,
}

impl Default for Prefetch {
    fn default() -> Self {
        let ctl = Ctl { run: State::Idle, watchers: Vec::new(), hosts: Vec::new(), shown: EngineStatus::NotInstalled, kicks: 0, launched: false };
        Self { ctl: Mutex::new(ctl), nudge: Condvar::new() }
    }
}

impl Prefetch {
    /// The process's job: every window's host starts and joins the same one.
    pub(crate) fn global() -> Arc<Self> {
        static JOB: OnceLock<Arc<Prefetch>> = OnceLock::new();
        JOB.get_or_init(Arc::default).clone()
    }

    fn lock(&self) -> MutexGuard<'_, Ctl> {
        self.ctl.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts a job for `mode`, or when one runs and a page waits, joins it: the job goes on at
    /// full speed and the page shows where it is. Without a source, a page waiting hears the
    /// browser isn't available, and nothing else starts.
    pub(crate) fn start(self: &Arc<Self>, mode: Mode, shared: &Arc<Mutex<SharedEngine>>, waker: Option<Waker>) {
        if !source::configured() {
            if mode == Mode::Urgent {
                set(shared, waker.as_ref(), EngineStatus::Failed(Fault::NotPublished.into()));
            }
            return;
        }
        let Some(root) = installer::try_engines_root() else {
            if mode == Mode::Urgent {
                set(shared, waker.as_ref(), EngineStatus::Failed(Fault::Dirs(installer::NO_DIRS).into()));
            }
            return;
        };
        let mut ctl = self.lock();
        ctl.hosts.retain(|(host, _)| host.strong_count() > 0);
        if !ctl.hosts.iter().any(|(host, _)| std::ptr::eq(host.as_ptr(), Arc::as_ptr(shared))) {
            ctl.hosts.push((Arc::downgrade(shared), waker.clone()));
        }
        ctl.launched |= mode == Mode::Launch;
        let (next, effect) = step(ctl.run, Event::Start(mode));
        ctl.run = next;
        let mut wake = None;
        if mode == Mode::Urgent {
            if !ctl.watchers.iter().any(|w| Arc::ptr_eq(&w.shared, shared)) {
                ctl.watchers.push(Watcher { shared: shared.clone(), waker: waker.clone() });
            }
            // The download it joins shows at once, where it is.
            if effect == Some(Effect::Nudge) && matches!(ctl.shown, EngineStatus::Downloading { .. } | EngineStatus::Verifying) {
                wake = tell(shared, waker.as_ref(), EngineEvent::InstallStatus(ctl.shown.clone()));
            }
        }
        drop(ctl);
        match effect {
            Some(Effect::Spawn(_)) => {
                let job = Job { ctl: self.clone() };
                if std::thread::Builder::new().name("opencode-browser-install".into()).spawn(move || job.run(&root)).is_err() {
                    self.end(EngineStatus::Failed(Fault::Launch("no thread to download it on".to_owned()).into()));
                }
            }
            Some(Effect::Nudge) => self.nudge.notify_all(),
            None => {}
        }
        if let Some(wake) = wake {
            wake();
        }
    }

    /// The app quits: the job stops at its next step (its partial file stays, and the next run
    /// goes on from it) and its threads get normal priority back. Nothing waits for them.
    pub(crate) fn quit(&self) {
        self.lock().run = State::Idle;
        self.nudge.notify_all();
        background::lift_for_quit();
    }

    /// The network changed: a job that waits for it tries again now.
    pub(crate) fn network_changed(&self) {
        let mut ctl = self.lock();
        if ctl.run == State::Idle {
            return;
        }
        ctl.kicks = ctl.kicks.wrapping_add(1);
        drop(ctl);
        self.nudge.notify_all();
    }

    fn mode(&self) -> Option<Mode> {
        match self.lock().run {
            State::Running(mode) => Some(mode),
            State::Idle => None,
        }
    }

    /// Whether a window's engine starts or runs.
    fn engine_starts(&self) -> bool {
        self.lock().hosts.iter().filter_map(|(host, _)| host.upgrade()).any(|host| {
            let host = host.lock().unwrap_or_else(PoisonError::into_inner);
            host.is_connected() || host.status() == EngineStatus::Connecting
        })
    }

    /// Whether a source that publishes nothing is watched (a launch asked this run).
    fn watches_source(&self) -> bool {
        self.lock().launched
    }

    /// Where the job is now: the pages that wait hear it, and a quiet job keeps it for a page
    /// that joins.
    fn report(&self, next: EngineStatus) {
        let mut ctl = self.lock();
        if ctl.run != State::Running(Mode::Urgent) {
            ctl.shown = next;
            return;
        }
        let wakes: Vec<Waker> = ctl.watchers.iter().filter_map(|w| tell(&w.shared, w.waker.as_ref(), EngineEvent::InstallStatus(next.clone()))).collect();
        ctl.shown = next;
        drop(ctl);
        wake_all(wakes);
    }

    /// Where a quiet job is (paused, waiting for its next try): never shown as it is, since a
    /// page that joins makes the job go on at once.
    fn note(&self, next: EngineStatus) {
        self.lock().shown = next;
    }

    /// Waits `for_`, or until a page waits or the job stops; with `network`, also until the
    /// network changes.
    fn wait(&self, for_: Duration, network: bool) {
        let ctl = self.lock();
        let kicks = ctl.kicks;
        self.nudge
            .wait_timeout_while(ctl, for_, |c| matches!(c.run, State::Running(Mode::Launch | Mode::Update)) && !(network && c.kicks != kicks))
            .ok();
    }

    /// The source lists no engine this build accepts: the pages that wait hear it and stop
    /// waiting, every window's host hears it (the browser hides), and the job stays to read the
    /// source again.
    fn unlisted(&self) {
        let mut ctl = self.lock();
        let failed = EngineStatus::Failed(Fault::NotPublished.into());
        let mut wakes = ctl.release_watchers(&failed);
        wakes.extend(ctl.each_host(|| EngineEvent::ProbeNotPublished));
        ctl.shown = failed;
        ctl.run = step(ctl.run, Event::Unlisted).0;
        drop(ctl);
        wake_all(wakes);
    }

    /// The source lists an engine again.
    fn listed(&self) {
        let wakes = self.lock().each_host(|| EngineEvent::ProbePublished);
        wake_all(wakes);
    }

    /// The job ends with `last` (installed, or why a page's download failed): every page that
    /// waits hears it, one that joined since the job last reported too.
    fn end(&self, last: EngineStatus) {
        let mut ctl = self.lock();
        let mut wakes = ctl.release_watchers(&last);
        if last == EngineStatus::Installed {
            wakes.extend(ctl.each_host(|| EngineEvent::Fetched));
        }
        ctl.shown = last;
        ctl.run = step(ctl.run, Event::Finished).0;
        drop(ctl);
        wake_all(wakes);
    }

    /// A quiet job stops for this run. False when a page joined meanwhile: the job goes on for it.
    fn give_up(&self) -> bool {
        let mut ctl = self.lock();
        if !ctl.watchers.is_empty() {
            return false;
        }
        ctl.run = step(ctl.run, Event::Finished).0;
        true
    }
}

struct Job {
    ctl: Arc<Prefetch>,
}

impl Job {
    fn run(self, root: &Path) {
        let mut attempts = 0;
        // A launch's fetch runs at background priority throughout. An update's index check and
        // pause answer come at normal priority (they're small, and a busy machine would starve
        // them for minutes); its download sets its pace.
        let low = Arc::new(AtomicBool::new(false));
        let mut unlisted = false;
        // What the partial file held at the last failed try, and the silent connections since it
        // last grew: a download that still gets somewhere starts its tries and its backoff anew.
        let mut reached = 0;
        let mut stalls = 0;
        loop {
            let Some(mode) = self.ctl.mode() else { break };
            self.pace_thread(mode, false, &low);
            let installed = installer::installed();
            // A launch that finds its engine asks the source nothing: a newer release of the same
            // revision is the update's to find, once a page runs.
            if mode == Mode::Launch && installed.is_some() {
                rune::marks::mark("browser.engine.probe.installed");
                self.ctl.end(EngineStatus::Installed);
                break;
            }
            if mode != Mode::Urgent
                && let Some(reason) = background::pause_reason(mode == Mode::Update)
            {
                log(&format!("browser engine: background download waits ({})", reason.describe()));
                self.ctl.note(EngineStatus::Paused(reason));
                self.ctl.wait(PAUSED_RECHECK, true);
                continue;
            }
            rune::marks::mark("browser.engine.probe.index");
            let release = match source::latest() {
                // A page waits and the source is out of reach, but an engine is here: use it.
                Err(_) if installed.is_some() && mode != Mode::Update => {
                    self.ctl.end(EngineStatus::Installed);
                    break;
                }
                // Nothing published for this build (a stale copy, or a revision it rejects): the
                // index is read again every hour, so a running app finds an engine published
                // meanwhile.
                Err(Fault::NotPublished) if self.ctl.watches_source() => {
                    rune::marks::mark("browser.engine.probe.not_published");
                    if !std::mem::replace(&mut unlisted, true) {
                        log(&format!("browser engine: {}", Fault::NotPublished));
                    }
                    self.ctl.unlisted();
                    self.ctl.wait(PROBE_RECHECK, false);
                    continue;
                }
                Err(err) => {
                    if self.failed(err, &mut attempts) {
                        break;
                    }
                    continue;
                }
                Ok(release) => release,
            };
            rune::marks::mark("browser.engine.index");
            if std::mem::take(&mut unlisted) {
                self.ctl.listed();
            }
            // Up to date, or another app sharing the engine folder installed it meanwhile.
            if installed.as_ref().is_some_and(|i| i.version.is_none_or(|v| v >= release.version)) {
                if mode == Mode::Update {
                    log(&format!("browser engine: {} is current", release.version));
                }
                self.ctl.end(EngineStatus::Installed);
                break;
            }
            match mode {
                Mode::Update => log(&format!("browser engine: downloading {} for the next start", release.version)),
                Mode::Launch => log(&format!("browser engine: downloading {} in the background", release.version)),
                Mode::Urgent => {}
            }
            let partial = root.join(format!(".{}-{}.part", release.version, &release.sha256[..16]));
            let started = Instant::now();
            let err = match self.install(root, &release, &partial, &low) {
                Ok(()) => {
                    rune::marks::mark("browser.engine.installed");
                    log(&format!("browser engine {} installed in {} µs", release.version, started.elapsed().as_micros()));
                    installer::remove_old(root, release.version);
                    installer::sweep_partials(root);
                    // A page that waits starts the engine itself, now. Else its files load once
                    // here, so the first page doesn't pay their first load.
                    let quiet = self.ctl.mode().filter(|&now| now != Mode::Urgent);
                    self.ctl.end(EngineStatus::Installed);
                    if let Some(quiet) = quiet
                        && let Some(engine) = installer::installed()
                    {
                        self.warm(&engine.bin, quiet);
                    }
                    break;
                }
                Err(Cut::Stopped) => continue,
                // Another app downloads it into the same file: never twice.
                Err(Cut::Failed(Fault::Busy)) if self.follow(&partial, &release) => continue,
                Err(cut) => {
                    let have = std::fs::metadata(&partial).map_or(0, |m| m.len()).min(release.size);
                    if have > std::mem::replace(&mut reached, have) {
                        attempts = 0;
                        stalls = 0;
                    }
                    // A page began to wait during a quiet try that the network then failed: the
                    // page gets its own try, now, from what the partial file holds.
                    let joined = mode != Mode::Urgent && self.ctl.mode() == Some(Mode::Urgent);
                    match cut {
                        Cut::Stalled => {
                            log(&format!("browser engine: the download went silent at {} of {} MB; connecting again", have / 1_000_000, release.size.div_ceil(1_000_000)));
                            stalls += 1;
                            // Once at once; a source that goes silent again without a new byte
                            // is a failed try, with its backoff.
                            if joined || stalls == 1 {
                                continue;
                            }
                            Fault::Interrupted { have, total: release.size }
                        }
                        Cut::Failed(err) if joined && network_fault(&err) => {
                            log(&format!("browser engine: {err}"));
                            continue;
                        }
                        Cut::Failed(err) | Cut::Rejected(err) => err,
                        // Handled above.
                        Cut::Stopped => continue,
                    }
                }
            };
            // Another app finished the same install while this try ran (and took the partial
            // file with it): the engine is there, whatever this try tripped on.
            if installer::installed().is_some_and(|i| i.version.is_some_and(|v| v >= release.version)) {
                self.ctl.end(EngineStatus::Installed);
                break;
            }
            if self.failed(err, &mut attempts) {
                break;
            }
        }
    }

    /// Loads a newly installed engine's program and libraries once, in the background, and lets
    /// it exit (`--print-cef-dir` returns at the top of the engine's `main`: no Chromium, no
    /// profile, no Keychain, no socket). The OS's first load of newly written images (Windows
    /// Defender scans each one; macOS checks the bundle's signature) took the first page after
    /// an install 4.1 s against 0.4 s for every later start; this pays it before the page comes.
    ///
    /// Considerate about it: the process runs at idle priority, only once the keyboard and
    /// mouse have been still for `WARM_QUIET` (it waits up to `WARM_PATIENCE` for that, then
    /// leaves the first load to the first page), not at all when a page starts the engine
    /// meanwhile, and it is ended if it hasn't exited after `WARM_LIMIT`.
    fn warm(&self, bin: &Path, mode: Mode) {
        let asked = Instant::now();
        loop {
            // After a launch's fetch no engine runs: one that starts now loads the files itself.
            if mode == Mode::Launch && self.ctl.engine_starts() {
                return log("browser engine: not warmed (a page starts it)");
            }
            if background::input_idle().is_none_or(|idle| idle >= WARM_QUIET) {
                break;
            }
            if asked.elapsed() >= WARM_PATIENCE {
                return log("browser engine: not warmed (the keyboard and mouse stayed busy)");
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let started = Instant::now();
        let mut command = std::process::Command::new(bin);
        command.arg("--print-cef-dir").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        background::idle_priority(&mut command);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => return log(&format!("browser engine: not warmed ({err})")),
        };
        let ran = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.to_string(),
                Ok(None) if started.elapsed() < WARM_LIMIT => std::thread::sleep(Duration::from_millis(50)),
                Ok(None) => {
                    child.kill().ok();
                    child.wait().ok();
                    break "ended: it didn't exit".to_owned();
                }
                Err(err) => break err.to_string(),
            }
        };
        let peak = background::peak_memory(&child).map_or_else(String::new, |bytes| format!(", peak {} MB", bytes.div_ceil(1_000_000)));
        log(&format!("browser engine: warmed in {} µs ({ran}{peak})", started.elapsed().as_micros()));
    }

    /// A try failed: true when the job ended (a page waits and hears why, or a quiet job has
    /// nothing more to try this run), else it waited out its backoff.
    fn failed(&self, err: Fault, attempts: &mut u32) -> bool {
        log(&format!("browser engine: {err}"));
        *attempts += 1;
        let Some(mode) = self.ctl.mode() else { return true };
        if mode == Mode::Urgent {
            self.ctl.end(EngineStatus::Failed(err.into()));
            return true;
        }
        let Some(backoff) = retry_after(&err, *attempts, mode) else {
            log("browser engine: the background download stops for this run");
            return self.ctl.give_up();
        };
        let network = network_fault(&err);
        self.ctl.note(EngineStatus::Failed(Failure::retrying(err, Instant::now() + backoff)));
        self.ctl.wait(backoff, network);
        false
    }

    /// Another app holds the partial file: a second copy of this app, or another build beside it,
    /// downloads the same archive. This job doesn't download it again. A page that waits shows
    /// that download's progress, read from the file's size, until the other app lets go of the
    /// file: it finished (the next try finds the engine, or the whole archive to check), or it
    /// quit (the next try goes on from what it left).
    ///
    /// False when the file hasn't grown for `FOLLOW_STALLED` and is still held (a holder that
    /// hangs, or a disk whose files can't be locked at all): the try failed as `Fault::Busy`.
    fn follow(&self, partial: &Path, release: &Release) -> bool {
        log("browser engine: another app is downloading it; following that download");
        let mut grew = (Instant::now(), 0);
        loop {
            let Some(mode) = self.ctl.mode() else { return true };
            let have = std::fs::metadata(partial).map_or(0, |m| m.len()).min(release.size);
            self.ctl.report(EngineStatus::Downloading { downloaded_bytes: have, total_bytes: Some(release.size) });
            if unlocked(partial) {
                return true;
            }
            if have != grew.1 {
                grew = (Instant::now(), have);
            } else if grew.0.elapsed() >= FOLLOW_STALLED {
                return false;
            }
            if mode == Mode::Urgent {
                std::thread::sleep(FOLLOW_EVERY);
            } else {
                self.ctl.wait(BUSY_RECHECK, false);
            }
        }
    }

    /// One try: the rest of the archive, unpacked as it arrives into a staging folder, checked,
    /// and moved into place as `<root>\<version>`.
    fn install(&self, root: &Path, release: &Release, partial: &Path, low: &Arc<AtomicBool>) -> Result<(), Cut> {
        std::fs::create_dir_all(root).map_err(|e| Cut::Failed(err_write(&e)))?;
        installer::sweep_unpacking(root);
        let dest = root.join(release.version.to_string());
        // Another app installed it since this job looked.
        if installer::is_complete(&dest) && source::accepts_install(&dest) {
            return Ok(());
        }
        // Before a byte comes: the rest of the archive and what it unpacks to (under three times
        // its size) must fit, and a launch's fetch leaves room beside it.
        let have = std::fs::metadata(partial).map_or(0, |m| m.len()).min(release.size);
        let need = (release.size - have).saturating_add(release.size.saturating_mul(3));
        let headroom = if self.ctl.mode() == Some(Mode::Launch) { LAUNCH_HEADROOM } else { 0 };
        if let Some(free) = background::free_space(root).filter(|&free| free < need.saturating_add(headroom)) {
            return Err(Cut::Failed(Fault::NoSpace { need, free }));
        }
        let staging = root.join(format!(".{}.staging-{}-{}", release.version, std::process::id(), STAGING_SEQ.fetch_add(1, Ordering::Relaxed)));
        std::fs::remove_dir_all(&staging).ok();
        std::fs::create_dir_all(&staging).map_err(|e| Cut::Failed(err_write(&e)))?;
        let result = self.fetch_into(release, partial, &staging, low);
        if matches!(result, Err(Cut::Rejected(_))) {
            // Bytes that don't check out never resume: the next try starts over.
            std::fs::remove_file(partial).ok();
        }
        let done = result.and_then(|()| {
            // A half-installed folder (missing executable/DLL or unaccepted trust marker), or a
            // stray file at `dest`, makes way for this verified install — never following links/junctions.
            if std::fs::symlink_metadata(&dest).is_ok()
                && (!source::accepts_install(&dest) || !installer::is_complete(&dest))
            {
                if installer::is_reparse_or_symlink(&dest) {
                    return Err(Cut::Failed(Fault::Write("refusing to replace a link at the engine install path".into())));
                }
                installer::discard(root, release.version);
            }
            match std::fs::rename(&staging, &dest) {
                Ok(()) => Ok(()),
                // Another app installed the same version meanwhile.
                Err(_) if installer::is_complete(&dest) && source::accepts_install(&dest) => Ok(()),
                Err(e) => Err(Cut::Failed(err_write(&e))),
            }
        });
        if done.is_ok() {
            std::fs::remove_file(partial).ok();
        }
        std::fs::remove_dir_all(&staging).ok();
        done
    }

    fn fetch_into(&self, release: &Release, partial: &Path, staging: &Path, low: &Arc<AtomicBool>) -> Result<(), Cut> {
        let mut progress = |downloaded_bytes: u64, total: u64| {
            self.ctl.report(EngineStatus::Downloading { downloaded_bytes, total_bytes: Some(total) });
        };
        let mut checked = Instant::now();
        let mut pace = || {
            let Some(mode) = self.ctl.mode() else { return Pace::Stop };
            self.pace_thread(mode, true, low);
            if mode == Mode::Urgent {
                return Pace::Full;
            }
            if checked.elapsed() >= PAUSE_CHECK {
                checked = Instant::now();
                if background::pause_reason(mode == Mode::Update).is_some() {
                    return Pace::Stop;
                }
            }
            if mode == Mode::Update { Pace::Limit(BACKGROUND_RATE) } else { Pace::Quiet }
        };
        let verifier = Verifier::new(&release.trust);
        // Each unpacked file: a page that began to wait meanwhile gets a normal-priority thread
        // for the rest.
        let mut unpacked = |file: &Path| {
            self.keep_pace(low);
            verifier.file(file);
        };
        let tar_zst = release.url.split('?').next().is_some_and(|u| u.ends_with(".tar.zst"));
        #[cfg(not(target_os = "macos"))]
        if !tar_zst {
            return Err(Cut::Rejected(Fault::Corrupt));
        }
        let mut body = Body::open(&release.url, partial, release.size, &mut progress, &mut pace, low).map_err(Cut::Failed)?;
        // A first try unpacks as the bytes arrive. A resumed one downloads the rest first and
        // unpacks the whole file once, rather than unpacking the bytes it already has again on
        // every try.
        let stream_unpack = tar_zst && body.resumed_at() == 0;
        if body.resumed_at() > 0 {
            log(&format!("browser engine: the download goes on from {} of {} MB", body.resumed_at() / 1_000_000, release.size.div_ceil(1_000_000)));
        }
        let streamed = if stream_unpack {
            unpack::tar_zst(&mut body, staging, &mut unpacked).map_err(unpack::UnpackError::cut)
        } else {
            std::io::copy(&mut body, &mut std::io::sink()).map(|_| ()).map_err(|_| Cut::Failed(Fault::Interrupted { have: 0, total: release.size }))
        };
        if let Err(cut) = streamed {
            return Err(body.cut().cloned().unwrap_or(cut));
        }
        let sha256 = body.finish()?;

        rune::marks::mark("browser.engine.downloaded");
        self.ctl.report(EngineStatus::Verifying);
        if sha256 != release.sha256 {
            return Err(Cut::Rejected(Fault::Corrupt));
        }
        self.keep_pace(low);
        if tar_zst && !stream_unpack {
            let mut archive = std::io::BufReader::new(std::fs::File::open(partial).map_err(|e| Cut::Failed(err_write(&e)))?);
            unpack::tar_zst(&mut archive, staging, &mut unpacked).map_err(unpack::UnpackError::cut)?;
        }
        #[cfg(target_os = "macos")]
        if !tar_zst {
            unzip(partial, staging)?;
        }
        self.keep_pace(low);
        verifier.finish(&staging.join(ENGINE_BUNDLE_NAME)).map_err(Cut::Rejected)?;
        rune::marks::mark("browser.engine.verified");
        if !installer::bin_in(staging).is_file() {
            return Err(Cut::Rejected(Fault::Corrupt));
        }
        #[cfg(windows)]
        {
            if !installer::bin_in(staging).with_file_name("opencode-browser-engine.dll").is_file() {
                return Err(Cut::Rejected(Fault::Corrupt));
            }
            unpack::grant_sandbox_rx(&staging.join(ENGINE_BUNDLE_NAME))
                .map_err(|e| Cut::Failed(Fault::Write(format!("sandbox permissions: {e}"))))?;
        }
        // Which rule accepted it: a later build starts it only if it accepts that rule too.
        std::fs::write(staging.join(installer::TRUST_MARKER), source::marker(&release.trust, &release.sha256)).map_err(|e| Cut::Failed(err_write(&e)))?;
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            let bin = installer::bin_in(staging);
            for program in [bin.with_file_name("opencode-browser-engine-helper"), bin] {
                let executable = program.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o100 != 0);
                if !executable {
                    return Err(Cut::Rejected(Fault::Corrupt));
                }
            }
        }
        Ok(())
    }

    /// The thread's priority for `mode`: normal when a page waits, background for a launch's
    /// fetch throughout, and for an update while it downloads and unpacks (`working`).
    /// The thread that reads the network follows `low` (`download::Watched`).
    fn pace_thread(&self, mode: Mode, working: bool, low: &AtomicBool) {
        let want = match mode {
            Mode::Urgent => false,
            Mode::Launch => true,
            Mode::Update => working,
        };
        if low.swap(want, Ordering::Relaxed) != want {
            background::set_background_priority(want);
        }
    }

    /// `pace_thread` for the job's mode now, while it downloads, unpacks or checks.
    fn keep_pace(&self, low: &AtomicBool) {
        if let Some(mode) = self.ctl.mode() {
            self.pace_thread(mode, true, low);
        }
    }
}

/// How long a quiet job waits before its next try after `fault`; `None` when it stops for this
/// run (a page that opens tries again itself, and shows why if it fails too).
fn retry_after(fault: &Fault, attempts: u32, mode: Mode) -> Option<Duration> {
    let backoff = Duration::from_secs(15 << attempts.min(6)).min(Duration::from_secs(15 * 60));
    let more = attempts < MAX_ATTEMPTS;
    match fault {
        // Out of reach, or cut short: another try costs nothing (a cut download resumes), so a
        // launch's fetch waits for the network as long as it takes. A development build's
        // loopback stand-in that isn't running is left alone after a few tries.
        Fault::Unreachable { .. } | Fault::Interrupted { .. } => (more || (mode == Mode::Launch && !source::local())).then_some(backoff),
        Fault::Server { .. } | Fault::Blocked { .. } | Fault::Unexpected { .. } => more.then_some(backoff),
        // Bytes that didn't check out are gone: one more download, then it's a page's to try.
        Fault::Corrupt | Fault::Untrusted(_) => (attempts < 2).then_some(backoff),
        // No room, no memory, a folder that can't be written, nothing published: another try
        // changes nothing.
        _ => None,
    }
}

/// Whether `fault` is the network's, so a change of network is worth a try at once.
fn network_fault(fault: &Fault) -> bool {
    matches!(fault, Fault::Unreachable { .. } | Fault::Interrupted { .. } | Fault::Server { .. } | Fault::Blocked { .. } | Fault::Unexpected { .. })
}

/// Whether no app holds the partial file's lock (or the file is gone).
fn unlocked(partial: &Path) -> bool {
    match std::fs::OpenOptions::new().read(true).write(true).open(partial) {
        Ok(file) => file.try_lock().is_ok(),
        Err(err) => err.kind() == std::io::ErrorKind::NotFound,
    }
}

/// macOS's bundle zip, with its links and attributes (`ditto`), into `staging`.
#[cfg(target_os = "macos")]
fn unzip(archive: &Path, staging: &Path) -> Result<(), Cut> {
    let ok = std::process::Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(archive).arg(staging).status().is_ok_and(|s| s.success());
    if ok { Ok(()) } else { Err(Cut::Rejected(Fault::Corrupt)) }
}

/// Steps `shared` with `event`; its window's waker when the step asks for a wake.
fn tell(shared: &Mutex<SharedEngine>, waker: Option<&Waker>, event: EngineEvent) -> Option<Waker> {
    matches!(shared.lock().unwrap_or_else(PoisonError::into_inner).step(event), EngineEffect::Wake).then(|| waker.cloned()).flatten()
}

fn wake_all(wakes: Vec<Waker>) {
    for wake in wakes {
        wake();
    }
}

fn set(shared: &Mutex<SharedEngine>, waker: Option<&Waker>, next: EngineStatus) {
    if let Some(wake) = tell(shared, waker, EngineEvent::InstallStatus(next)) {
        wake();
    }
}

fn log(line: &str) {
    eprintln!("{line}");
}

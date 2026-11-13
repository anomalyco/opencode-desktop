//! The one engine download job: started by a page that waits for the engine, or once a run after
//! the first page opens to fetch a newer release, and never twice. Nothing starts it before a
//! page opens. An update runs at low priority, throttled, waits while the OS reports a metered
//! connection or battery power, and retries with backoff; when a page waits (`hurry`) the
//! download goes on at full speed and normal priority. It reads the source's index, then streams the archive into place: the bytes
//! land in a partial file that a later try resumes, unpack as they arrive, and the folder moves
//! into place only once the archive's hash and the trust rule pass.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use rune::time::Instant;

use rune::Waker;

use crate::background;
use crate::download::{Body, Cut, Pace};
use crate::installer::{self, ENGINE_BUNDLE_NAME, ERR_INSTALL, ERR_VERIFY, EngineStatus, Failure, err_write};
use crate::source::{self, Release};
use crate::trust::Verifier;
use crate::unpack;

/// The background download's bandwidth: 2 MB/s, about a minute for the whole engine.
const BACKGROUND_RATE: u64 = 2_000_000;
/// How often a running background download asks the OS whether to pause.
const PAUSE_CHECK: Duration = Duration::from_secs(5);
/// How often a paused download asks again.
const PAUSED_RECHECK: Duration = Duration::from_secs(60);
/// An update's tries before it waits for the next run.
const MAX_ATTEMPTS: u32 = 6;
/// The background progress wakes the UI at most this often (the pane, when open, every 256 KB).
const QUIET_WAKE: Duration = Duration::from_secs(2);

static STAGING_SEQ: AtomicU64 = AtomicU64::new(1);

/// What a job is for, which sets its pace and what it reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// A page waits for the engine: full speed, normal priority, no pausing, one try.
    Urgent,
    /// An engine runs; a newer release installs beside it for its next start, at background
    /// pace, without touching the engine's status.
    Update,
}

/// The job's state (one per `BrowserHost`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Running(Mode),
}

#[derive(Clone, Copy, Debug)]
enum Event {
    Start(Mode),
    Hurry,
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
        (State::Running(_), Event::Start(Mode::Urgent) | Event::Hurry) => (State::Running(Mode::Urgent), Some(Effect::Nudge)),
        (_, Event::Finished) => (State::Idle, None),
        (other, _) => (other, None),
    }
}

pub(crate) struct Prefetch {
    state: Mutex<State>,
    /// Wakes the job's waits early (it hurries, or it's cancelled).
    nudge: Condvar,
}

impl Default for Prefetch {
    fn default() -> Self {
        Self { state: Mutex::new(State::Idle), nudge: Condvar::new() }
    }
}

impl Prefetch {
    /// Starts a job for `mode`, or makes a running one hurry when a page waits. Without a source,
    /// a page waiting hears the browser isn't available, and nothing else starts.
    pub(crate) fn start(self: &Arc<Self>, mode: Mode, status: Arc<Mutex<EngineStatus>>, waker: Option<Waker>) {
        if !source::configured() {
            if mode == Mode::Urgent {
                set(&status, waker.as_ref(), EngineStatus::Failed(installer::ERR_NO_SOURCE.into()));
            }
            return;
        }
        let Some(root) = installer::try_engines_root() else {
            if mode == Mode::Urgent {
                set(&status, waker.as_ref(), EngineStatus::Failed(installer::ERR_DIRS.into()));
            }
            return;
        };
        let effect = self.event(Event::Start(mode));
        if effect == Some(Effect::Spawn(mode)) {
            let job = Job { ctl: self.clone(), status, waker };
            if std::thread::Builder::new().name("opencode-browser-install".into()).spawn(move || job.run(&root)).is_err() {
                self.event(Event::Finished);
            }
        }
    }

    /// Makes a running job go on at full speed: someone waits for it now.
    pub(crate) fn hurry(&self) {
        self.event(Event::Hurry);
    }


    fn event(&self, event: Event) -> Option<Effect> {
        let mut state = self.state.lock().unwrap();
        let (next, effect) = step(*state, event);
        *state = next;
        if effect == Some(Effect::Nudge) {
            self.nudge.notify_all();
        }
        effect
    }

    fn state(&self) -> State {
        *self.state.lock().unwrap()
    }

    /// Waits `for_`, or until the job hurries or stops.
    fn wait(&self, for_: Duration) {
        let state = self.state.lock().unwrap();
        let _ = self.nudge.wait_timeout_while(state, for_, |s| matches!(s, State::Running(Mode::Update)));
    }
}

struct Job {
    ctl: Arc<Prefetch>,
    status: Arc<Mutex<EngineStatus>>,
    waker: Option<Waker>,
}

impl Job {
    fn run(self, root: &Path) {
        let mut attempts = 0;
        // The index check and the OS's pause answer come at normal priority (they're small, and a
        // busy machine would starve them for minutes); the download's first read sets its pace.
        let mut low = false;
        loop {
            let mode = match self.ctl.state() {
                State::Running(mode) => mode,
                State::Idle => {
                    self.report(EngineStatus::NotInstalled);
                    break;
                }
            };
            if mode != Mode::Urgent
                && let Some(reason) = background::pause_reason()
            {
                log(&format!("browser engine: background download waits ({})", reason.describe()));
                self.report(EngineStatus::Paused(reason));
                self.ctl.wait(PAUSED_RECHECK);
                continue;
            }
            let installed = installer::installed();
            let release = match source::latest() {
                // A page waits and the source is out of reach, but an engine is here: use it.
                Err(_) if installed.is_some() && mode != Mode::Update => {
                    self.report(EngineStatus::Installed);
                    break;
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
            // Up to date, or another app sharing the engine folder installed it meanwhile.
            if installed.as_ref().is_some_and(|i| i.version.is_none_or(|v| v >= release.version)) {
                if mode == Mode::Update {
                    log(&format!("browser engine: {} is current", release.version));
                }
                self.report(EngineStatus::Installed);
                break;
            }
            if mode == Mode::Update {
                log(&format!("browser engine: downloading {} for the next start", release.version));
            }
            let started = Instant::now();
            let err = match self.install(root, &release, &mut low) {
                Ok(()) => {
                    rune::marks::mark("browser.engine.installed");
                    log(&format!("browser engine {} installed in {} µs", release.version, started.elapsed().as_micros()));
                    installer::remove_old(root, release.version);
                    self.report(EngineStatus::Installed);
                    break;
                }
                Err(Cut::Stopped) => continue,
                Err(Cut::Failed(err) | Cut::Rejected(err)) => err,
            };
            if self.failed(err, &mut attempts) {
                break;
            }
        }
        self.ctl.event(Event::Finished);
        self.wake();
    }

    /// A try failed: true when the job gives up (a page waits, or it has tried enough), else it
    /// waits out its backoff.
    fn failed(&self, err: String, attempts: &mut u32) -> bool {
        log(&format!("browser engine: {err}"));
        *attempts += 1;
        if self.urgent() || *attempts >= MAX_ATTEMPTS {
            self.report(EngineStatus::Failed(err.into()));
            return true;
        }
        let backoff = Duration::from_secs(15 << (*attempts).min(6)).min(Duration::from_secs(15 * 60));
        self.report(EngineStatus::Failed(Failure::retrying(err, Instant::now() + backoff)));
        self.ctl.wait(backoff);
        false
    }

    /// One try: the rest of the archive, unpacked as it arrives into a staging folder, checked,
    /// and moved into place as `<root>\<version>`.
    fn install(&self, root: &Path, release: &Release, low: &mut bool) -> Result<(), Cut> {
        std::fs::create_dir_all(root).map_err(|e| Cut::Failed(err_write(&e)))?;
        installer::sweep_unpacking(root);
        let partial = root.join(format!(".{}-{}.part", release.version, &release.sha256[..16]));
        // Before a byte comes: the rest of the archive and what it unpacks to (under three times
        // its size) must fit.
        let have = std::fs::metadata(&partial).map_or(0, |m| m.len()).min(release.size);
        let need = (release.size - have).saturating_add(release.size.saturating_mul(3));
        if let Some(free) = background::free_space(root).filter(|&free| free < need) {
            return Err(Cut::Failed(installer::err_disk_space(need, free)));
        }
        let staging = root.join(format!(".{}.staging-{}-{}", release.version, std::process::id(), STAGING_SEQ.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).map_err(|e| Cut::Failed(err_write(&e)))?;
        let result = self.fetch_into(release, &partial, &staging, low);
        if matches!(result, Err(Cut::Rejected(_))) {
            // Bytes that don't check out never resume: the next try starts over.
            let _ = std::fs::remove_file(&partial);
        }
        let done = result.and_then(|()| {
            let dest = root.join(release.version.to_string());
            match std::fs::rename(&staging, &dest) {
                Ok(()) => Ok(()),
                // Another app installed the same version meanwhile.
                Err(_) if installer::bin_in(&dest).is_file() => Ok(()),
                Err(e) => Err(Cut::Failed(err_write(&e))),
            }
        });
        if done.is_ok() {
            let _ = std::fs::remove_file(&partial);
        }
        let _ = std::fs::remove_dir_all(&staging);
        done
    }

    fn fetch_into(&self, release: &Release, partial: &Path, staging: &Path, low: &mut bool) -> Result<(), Cut> {
        let update = self.ctl.state() == State::Running(Mode::Update);
        let mut last_wake = Instant::now();
        let mut progress = |downloaded_bytes: u64, total: u64| {
            if update {
                return;
            }
            *self.status.lock().unwrap() = EngineStatus::Downloading { downloaded_bytes, total_bytes: Some(total) };
            if self.urgent() || last_wake.elapsed() >= QUIET_WAKE || downloaded_bytes == total {
                last_wake = Instant::now();
                self.wake();
            }
        };
        let mut checked = Instant::now();
        let mut pace = || {
            let urgent = match self.ctl.state() {
                State::Idle => return Pace::Stop,
                State::Running(mode) => mode == Mode::Urgent,
            };
            if *low == urgent {
                *low = !urgent;
                background::set_background_priority(*low);
            }
            if urgent {
                return Pace::Full;
            }
            if checked.elapsed() >= PAUSE_CHECK {
                checked = Instant::now();
                if background::pause_reason().is_some() {
                    return Pace::Stop;
                }
            }
            Pace::Limit(BACKGROUND_RATE)
        };
        let verifier = Verifier::new(&release.trust);
        let tar_zst = release.url.split('?').next().is_some_and(|u| u.ends_with(".tar.zst"));
        #[cfg(not(target_os = "macos"))]
        if !tar_zst {
            return Err(Cut::Rejected(ERR_VERIFY.into()));
        }
        let mut body = Body::open(&release.url, partial, release.size, &mut progress, &mut pace).map_err(Cut::Failed)?;
        // A first try unpacks as the bytes arrive. A resumed one downloads the rest first and
        // unpacks the whole file once, rather than unpacking the bytes it already has again on
        // every try.
        let stream_unpack = tar_zst && body.resumed_at() == 0;
        let streamed = if stream_unpack {
            unpack::tar_zst(&mut body, staging, &mut |file| verifier.file(file)).map_err(unpack::UnpackError::cut)
        } else {
            std::io::copy(&mut body, &mut std::io::sink()).map(|_| ()).map_err(|_| Cut::Failed(installer::ERR_DOWNLOAD.into()))
        };
        if let Err(cut) = streamed {
            return Err(body.cut().cloned().unwrap_or(cut));
        }
        let sha256 = body.finish()?;

        rune::marks::mark("browser.engine.downloaded");
        if !update {
            self.report(EngineStatus::Verifying);
        }
        if sha256 != release.sha256 {
            return Err(Cut::Rejected(ERR_VERIFY.into()));
        }
        if tar_zst && !stream_unpack {
            let mut archive = std::io::BufReader::new(std::fs::File::open(partial).map_err(|e| Cut::Failed(err_write(&e)))?);
            unpack::tar_zst(&mut archive, staging, &mut |file| verifier.file(file)).map_err(unpack::UnpackError::cut)?;
        }
        #[cfg(target_os = "macos")]
        if !tar_zst {
            unzip(partial, staging)?;
        }
        verifier.finish(&staging.join(ENGINE_BUNDLE_NAME)).map_err(Cut::Rejected)?;
        rune::marks::mark("browser.engine.verified");
        if !installer::bin_in(staging).is_file() {
            return Err(Cut::Failed(ERR_INSTALL.into()));
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            let bin = installer::bin_in(staging);
            for program in [bin.with_file_name("opencode-browser-engine-helper"), bin] {
                let executable = program.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o100 != 0);
                if !executable {
                    return Err(Cut::Rejected(ERR_VERIFY.into()));
                }
            }
        }
        Ok(())
    }

    fn urgent(&self) -> bool {
        self.ctl.state() == State::Running(Mode::Urgent)
    }

    /// Sets the engine's status, unless this job is an update (an engine runs).
    fn report(&self, next: EngineStatus) {
        if self.ctl.state() != State::Running(Mode::Update) {
            set(&self.status, self.waker.as_ref(), next);
        }
    }

    fn wake(&self) {
        if let Some(w) = &self.waker {
            w();
        }
    }
}

/// macOS's bundle zip, with its links and attributes (`ditto`), into `staging`.
#[cfg(target_os = "macos")]
fn unzip(archive: &Path, staging: &Path) -> Result<(), Cut> {
    let ok = std::process::Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(archive).arg(staging).status().is_ok_and(|s| s.success());
    if ok { Ok(()) } else { Err(Cut::Failed(ERR_INSTALL.into())) }
}

fn set(status: &Mutex<EngineStatus>, waker: Option<&Waker>, next: EngineStatus) {
    *status.lock().unwrap() = next;
    if let Some(w) = waker {
        w();
    }
}

fn log(line: &str) {
    eprintln!("{line}");
}

//! The one engine download job: started quietly by the idle prefetch, or by the pane, and never
//! twice. In the background it runs at low priority, throttled, waits while the OS reports a
//! metered connection or battery power, and retries with backoff; when the pane asks (`hurry`)
//! the same download goes on at full speed and normal priority. The bytes land in a partial file
//! that later tries resume, are checked against the pinned SHA-256, and install atomically.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use rune::time::Instant;

use rune::Waker;

use crate::background;
use crate::download::{self, Fetched, Pace};
use crate::github_download::GithubAuth;
use crate::installer::{self, ERR_VERIFY, EngineStatus, Failure};

/// The background download's bandwidth: 2 MB/s, about a minute for the whole engine.
const BACKGROUND_RATE: u64 = 2_000_000;
/// How often a running background download asks the OS whether to pause.
const PAUSE_CHECK: Duration = Duration::from_secs(5);
/// How often a paused download asks again.
const PAUSED_RECHECK: Duration = Duration::from_secs(60);
/// Background tries before it waits for the pane (or the next launch).
const MAX_ATTEMPTS: u32 = 6;
/// The background progress wakes the UI at most this often (the pane, when open, every 256 KB).
const QUIET_WAKE: Duration = Duration::from_secs(2);

/// Whether a download job runs, and what it should do next. One per `BrowserHost`.
#[derive(Default)]
pub(crate) struct Prefetch {
    flags: Mutex<Flags>,
    /// Wakes the job's waits early (`start` while it runs, `cancel_background`).
    nudge: Condvar,
}

#[derive(Default)]
struct Flags {
    running: bool,
    /// Someone is waiting for the browser: full speed, normal priority, no pausing, one try.
    urgent: bool,
    cancelled: bool,
}

impl Prefetch {
    /// Starts the download job, or, when one runs, makes it hurry (`urgent`). Without a download
    /// source a background start does nothing; an urgent one says the browser isn't available.
    pub(crate) fn start(self: &Arc<Self>, urgent: bool, status: Arc<Mutex<EngineStatus>>, waker: Option<Waker>) {
        let source = installer::resolve_download_source();
        let mut flags = self.flags.lock().unwrap();
        if urgent {
            flags.urgent = true;
            flags.cancelled = false;
        }
        if flags.running {
            self.nudge.notify_all();
            return;
        }
        let Some((url, sha256)) = source else {
            flags.urgent = false;
            drop(flags);
            if urgent {
                set(&status, waker.as_ref(), EngineStatus::Failed(installer::ERR_NO_SOURCE.into()));
            }
            return;
        };
        flags.running = true;
        let job = Job { ctl: self.clone(), url, sha256, dest_root: installer::engine_cache_root(), status, waker };
        if std::thread::Builder::new().name("opencode-browser-install".into()).spawn(move || job.run()).is_err() {
            *flags = Flags::default();
        }
    }

    /// Makes a running job go on at full speed: someone waits for it now.
    pub(crate) fn hurry(&self) {
        let mut flags = self.flags.lock().unwrap();
        if flags.running {
            flags.urgent = true;
            flags.cancelled = false;
            self.nudge.notify_all();
        }
    }

    /// Stops a background download, keeping its bytes for later. A download the pane asked for
    /// goes on.
    pub(crate) fn cancel_background(&self) {
        let mut flags = self.flags.lock().unwrap();
        if flags.running && !flags.urgent {
            flags.cancelled = true;
            self.nudge.notify_all();
        }
    }

    fn urgent(&self) -> bool {
        self.flags.lock().unwrap().urgent
    }

    fn cancelled(&self) -> bool {
        let flags = self.flags.lock().unwrap();
        flags.cancelled && !flags.urgent
    }

    /// Waits `for_`, or until the job should hurry or stop.
    fn wait(&self, for_: Duration) {
        let flags = self.flags.lock().unwrap();
        let _ = self.nudge.wait_timeout_while(flags, for_, |f| !f.urgent && !f.cancelled);
    }

    fn finish(&self) {
        *self.flags.lock().unwrap() = Flags::default();
    }
}

struct Job {
    ctl: Arc<Prefetch>,
    url: String,
    sha256: Option<String>,
    dest_root: PathBuf,
    status: Arc<Mutex<EngineStatus>>,
    waker: Option<Waker>,
}

impl Job {
    fn run(self) {
        let partial = installer::partial_path(&self.dest_root, &self.url, self.sha256.as_deref());
        installer::sweep_stale_staging(&self.dest_root);
        let github = GithubAuth::user();
        let mut attempts = 0;
        let mut low = !self.ctl.urgent();
        background::set_background_priority(low);
        loop {
            if self.ctl.cancelled() {
                self.set(EngineStatus::NotInstalled);
                break;
            }
            if !self.ctl.urgent()
                && let Some(reason) = background::pause_reason()
            {
                self.set(EngineStatus::Paused(reason));
                self.ctl.wait(PAUSED_RECHECK);
                continue;
            }
            // Another app sharing the engine folder may have installed it meanwhile.
            if installer::resolve_installed_executable().is_some() {
                self.set(EngineStatus::Installed);
                break;
            }
            let have = std::fs::metadata(&partial).map_or(0, |m| m.len());
            self.set(EngineStatus::Downloading { downloaded_bytes: have, total_bytes: None });
            // A download that ended before it installed (the app quit while unpacking) is whole.
            let whole = self
                .sha256
                .as_deref()
                .filter(|_| have > 0)
                .and_then(|want| download::sha256_file(&partial).ok().filter(|got| got.eq_ignore_ascii_case(want)));
            let fetched = match whole {
                Some(sha256) => Ok(Fetched::Done { sha256 }),
                None => self.fetch(&partial, &github, &mut low),
            };
            let err = match fetched {
                Ok(Fetched::Stopped) => continue,
                Ok(Fetched::Done { sha256 }) => {
                    self.set(EngineStatus::Verifying);
                    match installer::install_archive(&self.url, &partial, self.sha256.as_deref(), &sha256, &self.dest_root) {
                        Ok(_) => {
                            let _ = std::fs::remove_file(&partial);
                            self.set(EngineStatus::Installed);
                            break;
                        }
                        Err(err) => {
                            // Bad bytes never resume: the next try starts over.
                            if err == ERR_VERIFY {
                                let _ = std::fs::remove_file(&partial);
                            }
                            err
                        }
                    }
                }
                Err(err) => err,
            };
            attempts += 1;
            if self.ctl.urgent() || attempts >= MAX_ATTEMPTS {
                self.set(EngineStatus::Failed(err.into()));
                break;
            }
            let backoff = Duration::from_secs(15 << attempts.min(6)).min(Duration::from_secs(15 * 60));
            self.set(EngineStatus::Failed(Failure::retrying(err, Instant::now() + backoff)));
            self.ctl.wait(backoff);
        }
        self.ctl.finish();
        self.wake();
    }

    /// One try: the rest of the archive into `partial`, paced by the job's mode, which can
    /// change mid-download (the pane opened, the OS asked to pause, the setting went off).
    fn fetch(&self, partial: &Path, github: &GithubAuth, low: &mut bool) -> Result<Fetched, String> {
        let mut last_wake = Instant::now();
        let progress = |downloaded_bytes, total_bytes| {
            *self.status.lock().unwrap() = EngineStatus::Downloading { downloaded_bytes, total_bytes };
            if self.ctl.urgent() || last_wake.elapsed() >= QUIET_WAKE || total_bytes == Some(downloaded_bytes) {
                last_wake = Instant::now();
                self.wake();
            }
        };
        let mut checked = Instant::now();
        let mut pace = || {
            if self.ctl.cancelled() {
                return Pace::Stop;
            }
            let urgent = self.ctl.urgent();
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
        download::fetch(&self.url, partial, github, progress, &mut pace)
    }

    fn set(&self, next: EngineStatus) {
        set(&self.status, self.waker.as_ref(), next);
    }

    fn wake(&self) {
        if let Some(w) = &self.waker {
            w();
        }
    }
}

fn set(status: &Mutex<EngineStatus>, waker: Option<&Waker>, next: EngineStatus) {
    *status.lock().unwrap() = next;
    if let Some(w) = waker {
        w();
    }
}

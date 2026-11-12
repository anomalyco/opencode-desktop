//! Finding, downloading, starting and connecting to the engine over its Unix socket. The UI
//! thread only hands the work to a thread: the socket, the install's folder and the spawn are
//! touched there.

#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicU64, Ordering};

use crate::client::BrowserHost;
use crate::installer::{self, EngineStatus};
#[cfg(target_os = "macos")]
use crate::mach_pipe::MachReceiver;
use crate::prefetch::Mode;
use crate::profile;
use crate::transport::{Link, UnixStream, spawn_and_attach, wire_stream};

#[cfg(target_os = "macos")]
static NEXT_RECEIVER_TAG: AtomicU64 = AtomicU64::new(1);

impl BrowserHost {
    /// Nothing is looked for until a page needs the engine (`EngineStatus::Unchecked`).
    pub(crate) fn initial_status() -> EngineStatus {
        EngineStatus::Unchecked
    }

    /// The status for a view that shows it without a page (Settings): an unchecked engine is
    /// looked for on a thread of its own, and the view hears the answer through the waker.
    pub fn status_checked(&self) -> EngineStatus {
        let mut status = self.status.lock().unwrap();
        if *status != EngineStatus::Unchecked {
            return status.clone();
        }
        *status = EngineStatus::Checking;
        let (shared, waker) = (self.status.clone(), self.waker.clone());
        let spawned = std::thread::Builder::new().name("opencode-browser-check".into()).spawn(move || {
            let found = if installer::installed().is_some() { EngineStatus::Installed } else { EngineStatus::NotInstalled };
            let mut status = shared.lock().unwrap();
            if *status == EngineStatus::Checking {
                *status = found;
            }
            drop(status);
            if let Some(w) = waker {
                w();
            }
        });
        if spawned.is_err() {
            *status = EngineStatus::Unchecked;
        }
        status.clone()
    }

    /// Starts (or retries) the engine: connects to this app's running one, starts the installed
    /// one, or downloads it first (a download already running goes on at full speed instead).
    /// Queued tabs open once it's ready (`poll`).
    pub fn start_install(&mut self) {
        if self.is_connected() || self.status() == EngineStatus::Connecting {
            return;
        }
        self.requeue_tabs();
        self.connect();
    }

    /// Downloads the engine in the background when it isn't installed, without starting it, so
    /// the first page opens without a wait: at low priority, throttled, pausing on a metered
    /// connection or battery power, retrying with backoff. Nothing shows unless the pane opens,
    /// which then shows this download's progress. Does nothing without a download source.
    pub fn prefetch(&mut self) {
        // The job looks for an installed engine first, on its own thread.
        if !matches!(self.status(), EngineStatus::Unchecked | EngineStatus::Checking | EngineStatus::NotInstalled) {
            return;
        }
        self.prefetch.start(Mode::Background, self.status.clone(), self.waker.clone());
    }

    /// Stops the background download (its bytes stay for later); one the pane asked for goes on.
    pub fn stop_prefetch(&mut self) {
        self.prefetch.cancel_background();
    }

    /// An engine is up: a newer release the source offers downloads beside it, quietly, for its
    /// next start.
    pub(crate) fn check_for_update(&self) {
        self.prefetch.start(Mode::Update, self.status.clone(), self.waker.clone());
    }

    fn ensure_mach_receiver(&mut self) -> Option<String> {
        #[cfg(target_os = "macos")]
        {
            if self.mach.is_none() {
                let tag = NEXT_RECEIVER_TAG.fetch_add(1, Ordering::Relaxed);
                self.mach = MachReceiver::start(self.surfaces.clone(), tag).ok();
            }
            self.mach.as_ref().map(|m| m.service_name.clone())
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    pub(crate) fn ensure_connected(&mut self) {
        if self.is_connected() {
            return;
        }
        match self.status() {
            EngineStatus::Connecting | EngineStatus::Failed(_) => {}
            // A tab waits for the download running in the background: let it finish, at full speed.
            EngineStatus::Downloading { .. } | EngineStatus::Verifying | EngineStatus::Paused(_) => self.prefetch.hurry(),
            _ => self.connect(),
        }
    }

    /// Hands the connect to a thread: this app's running engine (a UI hot reload), else the
    /// installed one, started, else a download that a page waits for.
    fn connect(&mut self) {
        *self.status.lock().unwrap() = EngineStatus::Connecting;
        let mach_service = self.ensure_mach_receiver();
        let sock = self.socket_override.clone().unwrap_or_else(installer::socket_path);
        let profile = profile::for_spawn(self.profile_dir.as_deref());
        let link = Link {
            status: self.status.clone(),
            outbox: self.outbox.clone(),
            inbox_tx: self.inbox_tx.clone(),
            surfaces: self.surfaces.clone(),
            waker: self.waker.clone(),
        };
        let prefetch = self.prefetch.clone();
        let spawned = std::thread::Builder::new().name("opencode-browser-connect".into()).spawn(move || {
            if let Ok(stream) = UnixStream::connect(&sock) {
                return wire_stream(stream, mach_service, &link);
            }
            let Some(engine) = installer::installed() else {
                *link.status.lock().unwrap() = EngineStatus::NotInstalled;
                return prefetch.start(Mode::Urgent, link.status.clone(), link.waker.clone());
            };
            rune::marks::mark("browser.engine.spawn");
            spawn_and_attach(&engine.bin, &sock, &profile, mach_service, &link);
            // Earlier versions this one replaces, now nothing starts them.
            if let Some(version) = engine.version {
                installer::remove_old(&installer::engines_root(), version);
            }
        });
        if spawned.is_err() {
            *self.status.lock().unwrap() = EngineStatus::Failed(installer::ERR_STOPPED.into());
        }
    }
}

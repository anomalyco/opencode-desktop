//! Finding, downloading, starting and connecting to the engine over its Unix socket.

use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicU64, Ordering};

use crate::client::BrowserHost;
use crate::installer::{self, EngineStatus};
#[cfg(target_os = "macos")]
use crate::mach_pipe::MachReceiver;
use crate::profile;
use crate::transport::{UnixStream, spawn_and_attach_bg, wire_stream};

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
            let found = if installer::resolve_installed_executable().is_some() { EngineStatus::Installed } else { EngineStatus::NotInstalled };
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

    /// Starts (or retries) downloading and installing the browser engine off the main thread; a
    /// download already running (the background prefetch) goes on at full speed instead. Queued
    /// tabs open once it lands (`poll`).
    pub fn start_install(&mut self) {
        if self.is_connected() || self.status() == EngineStatus::Connecting {
            return;
        }
        let mach_service = self.ensure_mach_receiver();
        let sock = self
            .socket_override
            .clone()
            .unwrap_or_else(installer::socket_path);
        if let Ok(stream) = UnixStream::connect(&sock) {
            self.attach_stream(stream, mach_service);
            return;
        }
        if let Some(bin) = installer::resolve_installed_executable() {
            self.spawn_and_connect(bin, sock, mach_service);
            return;
        }
        if matches!(self.status(), EngineStatus::Unchecked | EngineStatus::Checking) {
            *self.status.lock().unwrap() = EngineStatus::NotInstalled;
        }
        self.prefetch.start(true, self.status.clone(), self.waker.clone());
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
        self.prefetch.start(false, self.status.clone(), self.waker.clone());
    }

    /// Stops the background download (its bytes stay for later); one the pane asked for goes on.
    pub fn stop_prefetch(&mut self) {
        self.prefetch.cancel_background();
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
        let cur_status = self.status();
        if cur_status == EngineStatus::Connecting {
            return;
        }
        // A tab waits for the download running in the background: let it finish, at full speed.
        if matches!(cur_status, EngineStatus::Downloading { .. } | EngineStatus::Verifying | EngineStatus::Paused(_)) {
            self.prefetch.hurry();
            return;
        }

        let sock = self
            .socket_override
            .clone()
            .unwrap_or_else(installer::socket_path);

        // 1. Reconnect to an already-running sidecar first (survives UI hot-reloads).
        if let Ok(stream) = UnixStream::connect(&sock) {
            let mach_service = self.ensure_mach_receiver();
            self.attach_stream(stream, mach_service);
            return;
        }

        // 2. Spawn installed sidecar binary if present on disk.
        if let Some(bin) = installer::resolve_installed_executable() {
            let mach_service = self.ensure_mach_receiver();
            self.spawn_and_connect(bin, sock, mach_service);
            return;
        }

        // 3. Otherwise download it, then start it; a failed download waits for Retry.
        if !matches!(cur_status, EngineStatus::Failed(_)) {
            self.start_install();
        }
    }

    fn spawn_and_connect(&mut self, bin: PathBuf, sock: PathBuf, mach_service: Option<String>) {
        *self.status.lock().unwrap() = EngineStatus::Connecting;
        let status = self.status.clone();
        let outbox = self.outbox.clone();
        let inbox_tx = self.inbox_tx.clone();
        let surfaces = self.surfaces.clone();
        let waker = self.waker.clone();
        let profile = profile::for_spawn(self.profile_dir.as_deref());
        std::thread::Builder::new()
            .name("opencode-browser-connect".into())
            .spawn(move || {
                spawn_and_attach_bg(bin, sock, profile, mach_service, status, outbox, inbox_tx, surfaces, waker);
            })
            .ok();
    }

    fn attach_stream(&mut self, stream: UnixStream, mach_service: Option<String>) {
        wire_stream(
            stream,
            mach_service,
            &self.status,
            &self.outbox,
            self.inbox_tx.clone(),
            self.surfaces.clone(),
            self.waker.clone(),
        );
    }
}

//! Finding, downloading, starting and connecting to the engine over its Unix socket.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicU64, Ordering};

use crate::client::BrowserHost;
use crate::installer::{self, EngineStatus};
#[cfg(target_os = "macos")]
use crate::mach_pipe::MachReceiver;
use crate::profile;
use crate::transport::{spawn_and_attach_bg, wire_stream};

#[cfg(target_os = "macos")]
static NEXT_RECEIVER_TAG: AtomicU64 = AtomicU64::new(1);

impl BrowserHost {
    pub(crate) fn initial_status() -> EngineStatus {
        if installer::resolve_installed_executable().is_some() {
            EngineStatus::Ready
        } else {
            EngineStatus::NotInstalled
        }
    }

    /// Starts (or retries) downloading and installing the browser engine off the main thread,
    /// then spawns it and opens any queued tabs as soon as it is ready.
    pub fn start_install(&mut self) {
        if self.is_connected() {
            return;
        }
        if matches!(
            self.status(),
            EngineStatus::Downloading { .. } | EngineStatus::Connecting
        ) {
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
        let status = self.status.clone();
        let waker = self.waker.clone();
        let outbox = self.outbox.clone();
        let inbox_tx = self.inbox_tx.clone();
        let profile = profile::for_spawn(self.profile_dir.as_deref());
        installer::start_download(
            status.clone(),
            waker.clone(),
            Box::new(move |bin| {
                spawn_and_attach_bg(bin, sock, profile, mach_service, status, outbox, inbox_tx, waker);
            }),
        );
    }

    /// Downloads the engine in the background when it isn't installed, without starting it, so
    /// the first page opens without a wait. A tab opened meanwhile starts once the download lands.
    pub fn prefetch(&mut self) {
        if self.status() != EngineStatus::NotInstalled {
            return;
        }
        let status = self.status.clone();
        let waker = self.waker.clone();
        installer::start_download(
            status.clone(),
            waker.clone(),
            Box::new(move |_bin| {
                *status.lock().unwrap() = EngineStatus::Ready;
                if let Some(w) = waker {
                    w();
                }
            }),
        );
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
        if matches!(cur_status, EngineStatus::Downloading { .. } | EngineStatus::Connecting) {
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
        let waker = self.waker.clone();
        let profile = profile::for_spawn(self.profile_dir.as_deref());
        std::thread::Builder::new()
            .name("opencode-browser-connect".into())
            .spawn(move || {
                spawn_and_attach_bg(bin, sock, profile, mach_service, status, outbox, inbox_tx, waker);
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
            self.waker.clone(),
        );
    }
}

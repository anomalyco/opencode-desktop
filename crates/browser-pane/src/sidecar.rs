//! Finding, downloading, starting and connecting to the engine over its Unix socket. The UI
//! thread only hands the work to a thread: the socket, the install's folder and the spawn are
//! touched there.

#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicU64, Ordering};

use crate::client::BrowserHost;
use crate::installer::{self, EngineStatus, Fault};
#[cfg(target_os = "macos")]
use crate::mach_pipe::MachReceiver;
use crate::prefetch::Mode;
use crate::profile;
use crate::transport::{Link, UnixStream, spawn_and_attach, wire_stream};

#[cfg(target_os = "macos")]
static NEXT_RECEIVER_TAG: AtomicU64 = AtomicU64::new(1);

impl BrowserHost {
    /// Nothing is looked for until a page needs the engine (`EngineStatus::Unchecked`), unless
    /// this build has no engine source, trust rule or binary override at all (`Fault::NotPublished`).
    pub(crate) fn initial_status() -> EngineStatus {
        if Self::engine_configured() {
            EngineStatus::Unchecked
        } else {
            EngineStatus::Failed(Fault::NotPublished.into())
        }
    }

    /// Starts (or retries) the engine: connects to this app's running one, starts the installed
    /// one, or downloads it first (a download already running goes on at full speed instead).
    /// Queued tabs open once it's ready (`poll`).
    pub fn start_install(&mut self) {
        // One that stopped answering is ended: its pages open again in a new one.
        if self.status().is_hung() {
            *self.outbox.lock().unwrap() = None;
            crate::transport::end_engine(&self.engine);
            if let Ok(sock) = self.engine_socket() {
                let _ = std::fs::remove_file(sock);
            }
        }
        if self.is_connected() || self.status() == EngineStatus::Connecting {
            return;
        }
        self.requeue_tabs();
        self.connect(Start::ForPage);
    }

    /// A page is likely within `hold` (the pointer on a launcher for one, an agent's turn): the
    /// installed engine's process starts now, so the page doesn't wait for it, and it stops again
    /// once `hold` passes with no tab opened. Nothing starts while a tab is open, an engine runs
    /// or starts, or none is installed, and nothing downloads. One sleeping thread per call
    /// keeps the hold; the next call's hold replaces it.
    pub fn prepare(&mut self, hold: std::time::Duration) {
        use std::sync::atomic::Ordering;
        if !self.tabs.is_empty() || !matches!(self.status(), EngineStatus::Unchecked | EngineStatus::Installed | EngineStatus::Ready | EngineStatus::Connecting) {
            return;
        }
        rune::marks::mark("browser.engine.prepare");
        let call = self.prepared.fetch_add(1, Ordering::Relaxed) + 1;
        if !self.is_connected() && self.status() != EngineStatus::Connecting {
            self.connect(Start::Only);
        }
        if self.keep_engine {
            return;
        }
        let (opened, prepared, outbox, conn_gen, status) = (
            self.opened.clone(),
            self.prepared.clone(),
            self.outbox.clone(),
            self.conn_gen.clone(),
            self.status.clone(),
        );
        let sock = self.engine_socket().ok();
        let tabs = opened.load(Ordering::Relaxed);
        let _ = std::thread::Builder::new().name("opencode-browser-hold".into()).spawn(move || {
            std::thread::sleep(hold);
            if prepared.load(Ordering::Relaxed) == call && opened.load(Ordering::Relaxed) == tabs {
                let mut outbox = outbox.lock().unwrap();
                if let Some(tx) = outbox.take() {
                    conn_gen.fetch_add(1, Ordering::Relaxed);
                    if let Some(sock) = sock {
                        let _ = std::fs::remove_file(sock);
                    }
                    rune::marks::mark("browser.engine.unprepared");
                    let _ = tx.send(crate::protocol::HostMsg::Shutdown);
                    let mut st = status.lock().unwrap();
                    if *st == EngineStatus::Ready || *st == EngineStatus::Connecting {
                        *st = EngineStatus::Installed;
                    }
                }
            }
        });
    }

    /// The first page of the run is open on a running engine: a newer release the source offers
    /// downloads beside it, quietly, for its next start. Once a run; before that page, the
    /// installer and the updater do nothing at all (no request, timer or thread).
    pub(crate) fn check_for_update_once(&mut self) {
        if self.update_checked || self.tabs.is_empty() || self.status() != EngineStatus::Ready {
            return;
        }
        self.update_checked = true;
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
            _ => self.connect(Start::ForPage),
        }
    }

    pub(crate) fn engine_socket(&self) -> Result<std::path::PathBuf, &'static str> {
        self.socket_override.as_deref().map_or_else(installer::try_socket_path, installer::custom_socket)
    }

    /// Hands the connect to a thread: this app's running engine (a UI hot reload), else the
    /// installed one, started, else a download that a page waits for.
    fn connect(&mut self, start: Start) {
        *self.status.lock().unwrap() = EngineStatus::Connecting;
        let mach_service = self.ensure_mach_receiver();
        let sock = match self.engine_socket() {
            Ok(path) => path,
            Err(err) => {
                *self.status.lock().unwrap() = EngineStatus::Failed(Fault::Dirs(err).into());
                return;
            }
        };
        let generation = self.conn_gen.fetch_add(1, std::sync::atomic::Ordering::Relaxed).wrapping_add(1);
        let profile = profile::for_spawn(self.profile_dir.as_deref());
        let link = Link {
            generation,
            conn_gen: self.conn_gen.clone(),
            status: self.status.clone(),
            outbox: self.outbox.clone(),
            inbox_tx: self.inbox_tx.clone(),
            surfaces: self.surfaces.clone(),
            waker: self.waker.clone(),
            engine: self.engine.clone(),
            sock: sock.clone(),
            opens: self.pending_open_msgs(),
            early: self.early.clone(),
        };
        let prefetch = self.prefetch.clone();
        let spawned = std::thread::Builder::new().name("opencode-browser-connect".into()).spawn(move || {
            if let Ok(stream) = UnixStream::connect(&sock) {
                return wire_stream(stream, mach_service, &link);
            }
            let Some(profile) = profile else {
                return link.set(EngineStatus::Failed(Fault::Dirs(installer::NO_DIRS).into()));
            };
            let Some(engine) = installer::installed() else {
                if start == Start::Only {
                    return link.set(EngineStatus::NotInstalled);
                }
                return prefetch.start(Mode::Urgent, link.status.clone(), link.waker.clone());
            };
            rune::marks::mark("browser.engine.spawn");
            spawn_and_attach(&engine, &sock, &profile, mach_service, &link);
            // Earlier versions this one replaces, now nothing starts them (for a page only: an
            // engine started ahead of one leaves the folder alone).
            if start == Start::ForPage
                && let Some(version) = engine.version
                && let Some(root) = installer::try_engines_root()
            {
                installer::remove_old(&root, version);
            }
        });
        if spawned.is_err() {
            *self.status.lock().unwrap() = EngineStatus::Failed(Fault::Launch("no thread to start it on".to_owned()).into());
        }
    }
}

/// Why the engine starts: a page waits for it (downloading it first if need be), or one is
/// likely soon (`prepare`: the installed engine or nothing).
#[derive(Clone, Copy, PartialEq)]
enum Start {
    ForPage,
    Only,
}

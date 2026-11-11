//! Keeps OpenCode (Rune) current from Anomaly's release feed.
//!
//! The update engine is a host service ([`service::UpdaterService`]: a state machine, download
//! progress, and check and install actions) over a platform updater:
//!
//! - **macOS** (`mac.rs`): team builds (`anomaly.opencode-rune`) download and verify a signed
//!   bundle (code signature, signing team, Gatekeeper) into a private folder under
//!   `~/Library/Caches/anomaly.opencode-rune/Updates`, and swap it in atomically on Restart to
//!   update. Local dev bundles and bare `cargo` binaries never update.
//! - **Windows** (`windows.rs`): a per-user install verifies the download (size, SHA-256,
//!   Authenticode), stages it beside the install folder, and swaps the folders once the app exits
//!   (`apply.rs`, the app's own executable as the helper), on Restart to update or on quit.
//!
//! Both check ten seconds after launch and every ten minutes. The Updater plugin
//! (`plugins/updater`) draws it, as Desktop's updater plugin does: Settings > Updates, the update
//! pill, the toasts and What's new. It reaches the service through the `Updater` Luau service,
//! whose state, checks and installs this module answers ([`state_data`], [`check_async`],
//! [`install_async`]).

#[cfg(windows)]
pub mod apply;
#[cfg(windows)]
mod authenticode;
mod engine;
mod feed;
#[cfg(target_os = "macos")]
pub mod mac;
mod machine;
mod platform;
pub mod protocol;
pub mod service;
#[cfg(windows)]
mod windows;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rune::Tx;
use rune_luau::{Completer, Data};

use self::machine::View;
use self::service::{Answer, Snapshot, Ticket, UpdaterService};
use crate::App;

#[derive(Default)]
pub struct Updater {
    pub version: String,
    pub channel: String,
    /// The update service of an installed build, which every window shares; none in a build
    /// without an updater.
    service: Option<Rc<RefCell<UpdaterService>>>,
    /// The service's revision this window last showed.
    seen: u64,
}

/// A check or install a plugin waits on, answered by the service's ticket.
enum Waiting {
    Check(Ticket, Completer),
    Install(Ticket, Completer),
}

thread_local! {
    /// The one update service of the app (its windows share the app's thread), started by the
    /// first window, and how many windows use it.
    static SHARED: RefCell<Option<Rc<RefCell<UpdaterService>>>> = const { RefCell::new(None) };
    static WINDOWS: Cell<usize> = const { Cell::new(0) };
    /// Plugins' checks and installs still running.
    static WAITING: RefCell<Vec<Waiting>> = const { RefCell::new(Vec::new()) };
}

impl Updater {
    /// The update service, while one runs: the interface a UI drives (state, progress, check,
    /// install).
    pub fn service(&self) -> Option<Rc<RefCell<UpdaterService>>> {
        self.service.clone()
    }

    /// The service's revision: grows whenever what a UI draws of it changes.
    pub fn revision(&self) -> u64 {
        self.service.as_ref().map_or(0, |s| s.borrow().revision())
    }

    /// This window closes. When it's the last, the app quits, and a ready update installs on the
    /// way out (Windows).
    pub fn closing(&mut self) {
        let Some(service) = self.service.take() else { return };
        let left = WINDOWS.get().saturating_sub(1);
        WINDOWS.set(left);
        if left == 0 {
            service.borrow_mut().quitting();
            SHARED.with(|s| s.borrow_mut().take());
        }
    }
}

pub fn build_channel() -> &'static str {
    match option_env!("OPENCODE_RUNE_BUILD_CHANNEL") {
        Some(ch) if !ch.is_empty() => ch,
        _ => "local",
    }
}

/// This build's version: `OPENCODE_RUNE_VERSION` when the build set it (a release build sets the
/// crate's version to it too), else the crate's.
pub fn build_version() -> &'static str {
    match option_env!("OPENCODE_RUNE_VERSION") {
        Some(v) if !v.is_empty() => v,
        _ => env!("CARGO_PKG_VERSION"),
    }
}

/// The commit this build was made from (`build.rs`), when it knows it.
pub fn build_commit() -> Option<&'static str> {
    option_env!("OPENCODE_RUNE_COMMIT").filter(|c| !c.is_empty())
}

/// The changes a local or beta build carries (`build.rs`, Desktop's `OPENCODE_BETA_NOTES`): JSON
/// `{ sha, commits, notes }`. Other builds carry none.
pub fn beta_notes() -> Option<&'static str> {
    Some(include_str!(concat!(env!("OUT_DIR"), "/beta_notes.json"))).filter(|n| !n.is_empty())
}

/// Starts the update service, or joins this window to the one the first window started. Called
/// after the first frame; the service's worker finds out whether this build updates
/// (`platform::make`) off the UI thread, so this only creates channels and starts the thread.
pub fn start_if_packaged(s: &mut App) {
    if crate::headless() || s.updater.service.is_some() {
        return;
    }
    let shared = SHARED.with(|shared| shared.borrow().clone());
    let service = match shared {
        Some(service) => {
            if let Some(waker) = s.waker.clone() {
                service.borrow().add_waker(waker);
            }
            service
        }
        None => {
            let service = Rc::new(RefCell::new(UpdaterService::start(Box::new(make_platform), build_version(), feed::feed_channel(build_channel()), s.waker.clone())));
            SHARED.with(|shared| *shared.borrow_mut() = Some(service.clone()));
            service
        }
    };
    WINDOWS.set(WINDOWS.get() + 1);
    {
        let service = service.borrow();
        s.updater.version = service.snapshot().version.clone();
        s.updater.channel = service.snapshot().channel.clone();
        s.updater.seen = service.revision();
    }
    s.updater.service = Some(service);
}

/// This build's platform updater and its records folder, on the updater's worker thread.
fn make_platform() -> Option<(std::sync::Arc<dyn platform::Platform>, std::path::PathBuf)> {
    let (exe, updates) = (std::env::current_exe().ok()?, crate::dirs::updates()?);
    platform::make(&exe, build_version(), build_channel(), updates.clone()).map(|p| (p, updates))
}

/// Feeds the service's results to this window on the app's thread, and answers the plugins'
/// checks and installs that settled; zero allocations when idle. An install that handed off
/// quits the app, which saves every window first (`App::on_quit`), so the update opens each of
/// them again.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let Some(service) = &s.updater.service else { return false };
    let service = service.clone();
    let (quit, relaunch, revision) = {
        let mut service = service.borrow_mut();
        service.poll();
        if service.revision() == s.updater.seen && !service.quit_pending() {
            return false;
        }
        // The worker names the running version once it's up (the bundle's, on macOS).
        s.updater.version.clone_from(&service.snapshot().version);
        (service.take_quit(), service.relaunches(), service.revision())
    };
    s.updater.seen = revision;
    answer_waiting(&service);
    match (quit, relaunch) {
        (true, true) => tx.relaunch(),
        (true, false) => tx.quit(),
        (false, _) => {}
    }
    tx.instant();
    true
}

fn answer_waiting(service: &Rc<RefCell<UpdaterService>>) {
    let mut service = service.borrow_mut();
    WAITING.with_borrow_mut(|waiting| {
        let mut i = 0;
        while i < waiting.len() {
            let ticket = match &waiting[i] {
                Waiting::Check(t, _) | Waiting::Install(t, _) => *t,
            };
            let Some(answer) = service.take_answer(ticket) else {
                i += 1;
                continue;
            };
            match (waiting.remove(i), answer) {
                (Waiting::Check(_, done), Answer::Checked(view)) => done.complete(view_data(&view, service.snapshot())),
                (Waiting::Install(_, done), Answer::Installed(result)) => done.complete(result.err().map_or(Data::Nil, Data::String)),
                (Waiting::Check(_, done) | Waiting::Install(_, done), _) => done.complete(Data::Nil),
            }
        }
    });
}

const NO_UPDATER: &str = "No updater runs in this build";

/// Where the updater stands, as the `UpdaterState` record; nil when no updater runs, or while
/// the service is still finding out.
pub fn state_data(s: &App) -> Data {
    let Some(service) = &s.updater.service else { return Data::Nil };
    let service = service.borrow();
    match service.snapshot().state {
        View::Starting | View::Disabled => Data::Nil,
        ref view => view_data(view, service.snapshot()),
    }
}

fn view_data(view: &View, snapshot: &Snapshot) -> Data {
    let (phase, version, error) = match view {
        View::Starting | View::Disabled => ("error", None, Some(NO_UPDATER)),
        View::Idle => ("idle", None, None),
        View::Checking => ("checking", None, None),
        View::Downloading(v) => ("downloading", Some(v), None),
        View::UpToDate => ("up-to-date", None, None),
        View::Ready(v) => ("ready", Some(v), None),
        View::Installing(v) => ("installing", Some(v), None),
        View::Error(e) => ("error", None, Some(e.as_str())),
    };
    let progress = snapshot.progress.filter(|p| p.total > 0).map(|p| p.downloaded as f64 / p.total as f64);
    crate::plugin_host::services::map([
        ("phase", Data::String(phase.into())),
        ("version", version.map_or(Data::Nil, |v| Data::String(v.clone()))),
        ("progress", progress.map_or(Data::Nil, Data::Number)),
        ("error", error.map_or(Data::Nil, |e| Data::String(e.to_owned()))),
    ])
}

/// `Updater:CheckAsync()`: checks now and answers with what the check found.
pub fn check_async(s: &App, done: Completer) {
    let Some(service) = &s.updater.service else {
        done.complete(crate::plugin_host::services::map([("phase", Data::String("error".into())), ("error", Data::String(NO_UPDATER.into()))]));
        return;
    };
    let ticket = service.borrow_mut().check();
    WAITING.with_borrow_mut(|w| w.push(Waiting::Check(ticket, done)));
    answer_waiting(service);
}

/// `Updater:InstallAsync()`: restarts into the ready version; answers nil once it handed off.
pub fn install_async(s: &App, done: Completer) {
    let Some(service) = s.updater.service.as_ref().filter(|s| s.borrow().snapshot().state != View::Disabled) else {
        done.complete(Data::String(NO_UPDATER.into()));
        return;
    };
    let ticket = service.borrow_mut().install();
    WAITING.with_borrow_mut(|w| w.push(Waiting::Install(ticket, done)));
    answer_waiting(service);
}


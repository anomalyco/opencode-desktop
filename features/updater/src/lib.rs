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
//! - **Linux** (`linux.rs`): checks only. The AppImage, `.deb` and `.rpm` don't replace
//!   themselves, so a newer release is offered as a download (`download-required`, Desktop's
//!   external mode) for the package this build was installed from.
//!
//! All check ten seconds after launch and every ten minutes. The Updater plugin
//! (`plugins/updater`) draws it, as Desktop's updater plugin does: Settings > Updates, the toasts
//! (an update these checks find toasts once per version, in place of Desktop's update pill) and
//! What's new. It reaches the service through the `Updater` Luau service, whose state, checks and
//! installs this crate answers ([`state_data`], [`check_async`], [`install_async`]).

#[cfg(target_arch = "wasm32")]
extern crate ureq_web as ureq;

#[cfg(windows)]
pub mod apply;
pub mod engine;
mod feed;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
pub mod mac;
pub mod machine;
pub mod platform;
pub mod protocol;
pub mod service;
#[cfg(windows)]
mod windows;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::OnceLock;

use rune::Tx;
use rune_luau::{Completer, Data};

use self::machine::View;
use self::service::{Answer, Snapshot, Ticket, UpdaterService};

/// Build identity supplied by the host binary so binary provenance stays in the executable crate.
#[derive(Clone, Copy, Debug)]
pub struct BuildInfo {
    pub channel: &'static str,
    pub version: &'static str,
    pub commit: Option<&'static str>,
    pub product_name: &'static str,
}

/// Process-wide configuration registered once at startup by the host app.
#[derive(Clone, Copy)]
pub struct UpdaterConfig {
    pub build: BuildInfo,
    pub updates_dir: fn() -> Option<PathBuf>,
    /// Formats a raw OS/helper error string for user-facing failure toasts.
    pub plain_error: fn(&str) -> String,
}

/// Host window hooks the updater drives on the UI thread.
pub trait UpdaterHost: opencode_kernel::AppEnv + opencode_kernel::AppFeedback {
    fn updater(&self) -> &Updater;
    fn updater_mut(&mut self) -> &mut Updater;
}

static CONFIG: OnceLock<UpdaterConfig> = OnceLock::new();

const DEFAULT_BUILD: BuildInfo = BuildInfo {
    channel: "local",
    version: "0.0.0",
    commit: None,
    product_name: "OpenCode (Rune)",
};

/// Registers the host build identity, updates directory and error formatter.
/// Call once during startup before starting the updater service.
pub fn configure(config: UpdaterConfig) {
    CONFIG.get_or_init(|| config);
}

pub fn build_info() -> &'static BuildInfo {
    CONFIG.get().map_or(&DEFAULT_BUILD, |c| &c.build)
}

pub fn build_channel() -> &'static str {
    build_info().channel
}

pub fn build_version() -> &'static str {
    build_info().version
}

pub fn build_commit() -> Option<&'static str> {
    build_info().commit
}

/// Strips a trailing ` (os error N)` suffix from a Rust `io::Error` display string.
pub fn default_plain_error(e: &str) -> String {
    let e = e.trim();
    let e = match e.rfind(" (os error ") {
        Some(i) if e.ends_with(')') => &e[..i],
        _ => e,
    };
    e.to_owned()
}

pub(crate) fn updates_dir() -> Option<PathBuf> {
    (CONFIG.get()?.updates_dir)()
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn plain_error(e: &str) -> String {
    match CONFIG.get() {
        Some(cfg) => (cfg.plain_error)(e),
        None => default_plain_error(e),
    }
}

/// A map of `Data` fields, omitting `Data::Nil` entries.
fn map(fields: impl IntoIterator<Item = (&'static str, Data)>) -> Data {
    Data::Map(fields.into_iter().filter(|(_, v)| *v != Data::Nil).map(|(k, v)| (k.to_owned(), v)).collect())
}

/// Per-window updater lifecycle: idle before startup or when another copy holds the profile,
/// waiting for storage to decide profile ownership, or attached to the shared service.
#[derive(Default)]
enum Lifecycle {
    #[default]
    Idle,
    /// Its start waits for storage to say whether this copy holds the profile.
    WaitingProfile,
    /// Attached to the shared updater service; `seen` is the revision this window last showed.
    Running {
        service: Rc<RefCell<UpdaterService>>,
        seen: u64,
    },
}

enum LifecycleEvent {
    StartRequested { headless: bool, holds_profile: Option<bool> },
    ProfileDecided { headless: bool, holds_profile: bool },
    Attached { service: Rc<RefCell<UpdaterService>>, seen: u64 },
    RevisionSeen(u64),
    Closing,
}

enum LifecycleEffect {
    None,
    Attach,
    Detach(Rc<RefCell<UpdaterService>>),
}

fn step(state: &mut Lifecycle, event: LifecycleEvent) -> LifecycleEffect {
    match (std::mem::replace(state, Lifecycle::Idle), event) {
        (
            Lifecycle::Idle | Lifecycle::WaitingProfile,
            LifecycleEvent::StartRequested { headless: false, holds_profile: Some(true) },
        ) => LifecycleEffect::Attach,
        (
            Lifecycle::Idle | Lifecycle::WaitingProfile,
            LifecycleEvent::StartRequested { headless: false, holds_profile: None },
        ) => {
            *state = Lifecycle::WaitingProfile;
            LifecycleEffect::None
        }
        (Lifecycle::WaitingProfile, LifecycleEvent::StartRequested { holds_profile: Some(false), .. }) => {
            LifecycleEffect::None
        }
        (Lifecycle::WaitingProfile, LifecycleEvent::ProfileDecided { headless: false, holds_profile: true }) => {
            LifecycleEffect::Attach
        }
        (Lifecycle::WaitingProfile, LifecycleEvent::ProfileDecided { .. }) => LifecycleEffect::None,
        (Lifecycle::Idle, LifecycleEvent::Attached { service, seen }) => {
            *state = Lifecycle::Running { service, seen };
            LifecycleEffect::None
        }
        (Lifecycle::Running { service, .. }, LifecycleEvent::RevisionSeen(seen)) => {
            *state = Lifecycle::Running { service, seen };
            LifecycleEffect::None
        }
        (Lifecycle::Running { service, .. }, LifecycleEvent::Closing) => LifecycleEffect::Detach(service),
        (Lifecycle::WaitingProfile, LifecycleEvent::Closing) => LifecycleEffect::None,
        (prev, _) => {
            *state = prev;
            LifecycleEffect::None
        }
    }
}

#[derive(Default)]
pub struct Updater {
    pub version: String,
    pub channel: String,
    state: Lifecycle,
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
    fn service_ref(&self) -> Option<&Rc<RefCell<UpdaterService>>> {
        match &self.state {
            Lifecycle::Running { service, .. } => Some(service),
            Lifecycle::Idle | Lifecycle::WaitingProfile => None,
        }
    }

    /// The update service, while one runs: the interface a UI drives (state, progress, check,
    /// install).
    pub fn service(&self) -> Option<Rc<RefCell<UpdaterService>>> {
        self.service_ref().cloned()
    }

    /// The service's revision: grows whenever what a UI draws of it changes.
    pub fn revision(&self) -> u64 {
        self.service_ref().map_or(0, |s| s.borrow().revision())
    }

    /// This window closes. When it's the last, the app quits, and a ready update installs on the
    /// way out (Windows).
    pub fn closing(&mut self) {
        let LifecycleEffect::Detach(service) = step(&mut self.state, LifecycleEvent::Closing) else { return };
        let left = WINDOWS.get().saturating_sub(1);
        WINDOWS.set(left);
        if left == 0 {
            service.borrow_mut().quitting();
            SHARED.with(|s| s.borrow_mut().take());
        }
    }
}

/// The install-on-quit hand-off, started by the last window's quit (`UpdaterService::quitting`).
static QUIT_INSTALL: std::sync::Mutex<Option<std::thread::JoinHandle<()>>> = std::sync::Mutex::new(None);

fn install_on_quit(platform: std::sync::Arc<dyn platform::Platform>) {
    let handle = std::thread::Builder::new().name("opencode-update-handoff".into()).spawn(move || platform.install_on_quit());
    if let Ok(handle) = handle {
        *QUIT_INSTALL.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
    }
}

/// Whether this quit installs an update (its helper replaces the app once this process exits).
pub fn installs_on_quit() -> bool {
    QUIT_INSTALL.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// Waits for an install-on-quit hand-off to finish starting its helper, which must exist before
/// this process exits (it waits for the exit, then swaps the folders). Call it last in `main`.
pub fn finish_quit() {
    let handle = QUIT_INSTALL.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(handle) = handle {
        handle.join().ok();
    }
}

/// A start that waited for the storage's decision (`start_if_packaged`).
pub fn poll_owed(s: &mut dyn UpdaterHost) {
    if !matches!(s.updater().state, Lifecycle::WaitingProfile) {
        return;
    }
    let Some(holds_profile) = opencode_storage::holds_profile() else { return };
    if matches!(
        step(&mut s.updater_mut().state, LifecycleEvent::ProfileDecided { headless: rune::is_headless(), holds_profile }),
        LifecycleEffect::Attach
    ) {
        attach_service(s);
    }
}

/// Starts the update service, or joins this window to the one the first window started. Called
/// after the first frame; the service's worker finds out whether this build updates
/// (`platform::make`) off the UI thread, so this only creates channels and starts the thread.
pub fn start_if_packaged(s: &mut dyn UpdaterHost) {
    let headless = rune::is_headless();
    if headless || matches!(s.updater().state, Lifecycle::Running { .. }) {
        return;
    }
    // Only the copy that holds the profile checks, downloads and stages: a second copy of the app
    // (macOS: `open -n`, a shell) would stage into the same folders. Undecided yet (a relaunch
    // waiting for its old copy): once it is (`poll_owed`).
    let holds_profile = opencode_storage::holds_profile();
    if holds_profile == Some(false) && matches!(s.updater().state, Lifecycle::Idle) {
        return;
    }
    if holds_profile.is_none() && matches!(s.updater().state, Lifecycle::WaitingProfile) {
        return;
    }
    if matches!(
        step(&mut s.updater_mut().state, LifecycleEvent::StartRequested { headless, holds_profile }),
        LifecycleEffect::Attach
    ) {
        attach_service(s);
    }
}

fn attach_service(s: &mut dyn UpdaterHost) {
    let waker = s.waker();
    let shared = SHARED.with(|shared| shared.borrow().clone());
    let service = match shared {
        Some(service) => {
            if let Some(waker) = waker {
                service.borrow().add_waker(waker);
            }
            service
        }
        None => {
            let service = Rc::new(RefCell::new(UpdaterService::start(Box::new(make_platform), build_version(), feed::feed_channel(build_channel()), waker)));
            SHARED.with(|shared| *shared.borrow_mut() = Some(service.clone()));
            service
        }
    };
    WINDOWS.set(WINDOWS.get() + 1);
    let (version, channel, seen) = {
        let service = service.borrow();
        (service.snapshot().version.clone(), service.snapshot().channel.clone(), service.revision())
    };
    let updater = s.updater_mut();
    updater.version = version;
    updater.channel = channel;
    step(&mut updater.state, LifecycleEvent::Attached { service, seen });
}

/// This build's platform updater, on the updater's worker thread.
fn make_platform() -> Option<std::sync::Arc<dyn platform::Platform>> {
    let (exe, updates) = (std::env::current_exe().ok()?, updates_dir()?);
    platform::make(&exe, build_version(), build_channel(), updates)
}

/// Feeds the service's results to this window on the app's thread, and answers the plugins'
/// checks and installs that settled; zero allocations when idle. An install that handed off
/// quits the app, which saves every window first (`App::on_quit`), so the update opens each of
/// them again.
pub fn poll(s: &mut dyn UpdaterHost, tx: &mut Tx) -> bool {
    let Lifecycle::Running { service, seen } = &s.updater().state else { return false };
    let (service, seen) = (service.clone(), *seen);
    let (quit, relaunch, revision, version) = {
        let mut service = service.borrow_mut();
        service.poll();
        if service.revision() == seen && !service.quit_pending() {
            return false;
        }
        // The worker names the running version once it's up (the bundle's, on macOS).
        (service.take_quit(), service.relaunches(), service.revision(), service.snapshot().version.clone())
    };
    {
        let updater = s.updater_mut();
        updater.version = version;
        step(&mut updater.state, LifecycleEvent::RevisionSeen(revision));
    }
    answer_waiting(&service);
    if let Some(err) = service.borrow_mut().take_apply_error() {
        s.toast_fail(opencode_i18n::t!("common.requestFailed").into(), err);
    }
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
pub fn state_data(s: &dyn UpdaterHost) -> Data {
    let Some(service) = s.updater().service_ref() else { return Data::Nil };
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
        View::Installed(v) => ("installed", Some(v), None),
        View::Ready(v) => ("ready", Some(v), None),
        View::DownloadRequired(v) => ("download-required", Some(v), None),
        View::Installing(v) => ("installing", Some(v), None),
        View::Error(e) => ("error", None, Some(e.as_str())),
    };
    let progress = snapshot.progress.filter(|p| p.total > 0).map(|p| p.downloaded as f64 / p.total as f64);
    map([
        ("phase", Data::String(phase.into())),
        ("version", version.map_or(Data::Nil, |v| Data::String(v.clone()))),
        ("progress", progress.map_or(Data::Nil, Data::Number)),
        ("error", error.map_or(Data::Nil, |e| Data::String(e.to_owned()))),
    ])
}

/// `Updater:CheckAsync()`: checks now and answers with what the check found.
pub fn check_async(s: &dyn UpdaterHost, done: Completer) {
    let Some(service) = s.updater().service_ref() else {
        done.complete(map([("phase", Data::String("error".into())), ("error", Data::String(NO_UPDATER.into()))]));
        return;
    };
    let ticket = service.borrow_mut().check();
    WAITING.with_borrow_mut(|w| w.push(Waiting::Check(ticket, done)));
    answer_waiting(service);
}

/// `Updater:InstallAsync()`: restarts into the ready version, or opens a required download;
/// answers nil once it handed off.
pub fn install_async(s: &dyn UpdaterHost, done: Completer) {
    let Some(service) = s.updater().service_ref().filter(|s| s.borrow().snapshot().state != View::Disabled) else {
        done.complete(Data::String(NO_UPDATER.into()));
        return;
    };
    let ticket = service.borrow_mut().install();
    WAITING.with_borrow_mut(|w| w.push(Waiting::Install(ticket, done)));
    answer_waiting(service);
}

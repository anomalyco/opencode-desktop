//! Launch order. A window's first frame draws while the window is created, so a windowed live
//! run reads only local files before it (saved tabs, zoom, route and drafts). `on_ready` then
//! starts the first server reads; when they land, the event stream, the open session's page and
//! the prefetch start (`connect`). Headless runs and hot builds have no `on_ready`: they read the
//! server before the first frame, as a snapshot needs, and connect from `setup`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use opencode_kit::client::{Request, Server};
use opencode_kit::model::Transcript;
use rune::Tx;
use serde_json::Value;

use crate::reconnect::ServerId;
use crate::{App, Route, Source, arg, headless, session_pages};
use rune::machine::Effects;

static DEFERRED: AtomicBool = AtomicBool::new(false);

/// Windows: a release build is a windowed program (see `build.rs`), which Windows starts without
/// a console. Started from a terminal, it writes to that terminal's console (`--version`, logs);
/// output redirected to a file or a pipe already has somewhere to go.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
pub fn attach_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE};
    // SAFETY: reads this process's standard error handle, and attaches to its parent's console
    // when it has none; both fail harmlessly.
    unsafe {
        if GetStdHandle(STD_ERROR_HANDLE).is_null() {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn attach_console() {}

/// `opencode-desktop --version`: the version and the release channel it was built for.
pub fn print_version_if_asked() {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("opencode-rune {} ({})", crate::updater::build_version(), crate::updater::build_channel());
        std::process::exit(0);
    }
}

/// Window title (`"OpenCode (Rune)"` on `main`, `"OpenCode (Rune) · <branch>"` on PR builds).
pub fn window_title() -> &'static str {
    option_env!("OPENCODE_RUNE_WINDOW_TITLE").unwrap_or("OpenCode (Rune)")
}

/// The build's identity: its name, its app id (the Windows AppUserModelID, the uninstall
/// entry's key and the single-instance pipe's name) and its own URL scheme, from the same
/// environment the release workflows give every artifact of a build (the setup too).
pub fn product() -> win_install::Product {
    win_install::Product::from_build(
        option_env!("OPENCODE_RUNE_WINDOW_TITLE"),
        option_env!("OPENCODE_RUNE_BUNDLE_ID"),
        option_env!("OPENCODE_RUNE_BUILD_CHANNEL"),
        option_env!("OPENCODE_RUNE_URL_SCHEME"),
        crate::updater::build_version(),
    )
}

/// What `rune::single_instance` keeps one process of: the build's app id, this executable and
/// its data folder, as Desktop's single-instance lock is its `userData` folder's. An installed
/// app has one of each; development builds in other checkouts, and runs with their own
/// `RUNE_DIRS_ROOT`, each run as their own.
pub fn instance_id() -> String {
    use sha2::{Digest, Sha256};
    let exe = std::env::current_exe().unwrap_or_default();
    let data = crate::dirs::data().unwrap_or_default();
    let digest = Sha256::digest(format!("{}\n{}", exe.display(), data.display()).to_lowercase().as_bytes());
    let tag: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{tag}", product().id())
}

/// The app icon full bleed, as Windows draws an app's icon (no macOS grid or shadow), for the
/// notification center's icon file.
#[cfg(target_os = "windows")]
pub const WINDOWS_ICON: &[u8] = include_bytes!("../assets/icon/art-512.png");

/// Windows: the windows' title bar, taskbar and Alt+Tab icon, from the exe's own icon resource
/// (`build.rs`), so nothing is decoded or resized at launch.
#[cfg(target_os = "windows")]
pub fn apply_app_icon() {
    rune::set_app_icon_resource(1);
}

/// Sets the Dock icon from the enclosing `.app`'s `Contents/Resources/icon.icns` (preserving PR
/// icon badges) when bundled, or the embedded default icon during `cargo run`.
#[cfg(not(target_os = "windows"))]
pub fn apply_app_icon() {
    let bundled = std::env::current_exe()
        .ok()
        .and_then(|exe| std::fs::read(exe.parent()?.parent()?.join("Resources/icon.icns")).ok());
    match bundled {
        Some(bytes) => {
            let _ = rune::set_app_icon(bytes.as_slice());
        }
        None => {
            let _ = rune::set_app_icon(include_bytes!("../assets/icon.icns").as_slice());
        }
    }
}

/// Initial server target: `--server` if passed, otherwise `:4096` (plus `service.json` discovery
/// immediately when `on_ready` won't run, such as `--live --snap` or `hot` builds).
pub fn initial_server() -> Server {
    let explicit = crate::arg("--server");
    let server = Server::new(&explicit.clone().unwrap_or_else(opencode_kit::client::default_server)).with_media_refs();
    if explicit.is_none() && !deferred() {
        opencode_kit::local_service::discover_local_server(&server)
    } else {
        server
    }
}

/// `main`, for runs that call `on_ready`: the server reads wait for it.
#[cfg_attr(feature = "hot", allow(dead_code))]
pub fn defer() {
    DEFERRED.store(true, Ordering::Relaxed);
}

/// Whether `App::load` leaves the server reads to `on_ready`.
pub fn deferred() -> bool {
    DEFERRED.load(Ordering::Relaxed)
}

/// The first reads: projects, sessions, models and agents, providers, and what's running.
pub struct First {
    projects: Value,
    sessions: Option<Value>,
    models: Option<(Value, Value)>,
    providers: Option<Value>,
    active: Option<Value>,
    /// The config's documents, for the model it names (`configured.ts`).
    config: Option<Value>,
}

impl First {
    pub fn fetch(server: &Server) -> Result<First, String> {
        let get = |path: &'static str| {
            let start = std::time::Instant::now();
            let r = server.get(path);
            crate::telemetry::launch_read(path, start);
            r
        };
        let projects = get("/api/project")?;
        // Once the project probe succeeds these reads are independent. A slow catalog
        // mustn't add its whole round trip to each successive startup read.
        Ok(std::thread::scope(|scope| {
            let [sessions, models, agents, providers, active, config] = ["/api/session?limit=120", "/api/model", "/api/agent", "/api/provider", "/api/session/active", "/api/config"]
                .map(|path| scope.spawn(move || get(path).ok())).map(|task| task.join().unwrap());
            First { projects, sessions, models: models.map(|m| (m,agents.unwrap_or_default())), providers, active, config }
        }))
    }
}

/// What `on_ready`'s background startup worker brings back: Desktop Beta's tabs (when no
/// `tabs.json` exists yet) and the first server reads, or `first: None` when the server wasn't
/// reachable and the window should fall back to the fixture.
pub struct Ready {
    pub server: Option<Server>,
    pub beta_tabs: Option<Vec<crate::tabs::Tab>>,
    pub first: Option<Result<First, String>>,
}

/// A server that didn't answer, trying it again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Again {
    pub retries: u32,
    /// The app tried on its own (its timer, the network coming back): the card's button doesn't
    /// say Checking, and another miss changes nothing on screen.
    pub quiet: bool,
}

/// One server's launch: from the first reads to connected, or no answer and tries again. As
/// OpenCode Desktop's server health settles `checking → settled` per server
/// (`packages/app/src/runtime/server/health-machine.ts`), and its startup waits for the server
/// before it mounts (`packages/desktop/src/renderer/startup/machine.ts`: `booting → hydrating`).
#[derive(Debug, Default)]
pub enum Phase {
    /// A windowed run waits for `on_ready` before its first reads. `beta_tabs`: no tabs were
    /// saved, so Desktop Beta's `drafts.sqlite` tabs are read too; without them a first launch
    /// has none and shows Home, as Desktop's does.
    Waiting { beta_tabs: bool },
    /// The probe and first reads are on their way; `again` while the server didn't answer before.
    Probing { rx: Receiver<Ready>, again: Option<Again> },
    /// No server answered: the next try is at `retry_at` (`App::clock`).
    Offline { retries: u32, retry_at: f64 },
    /// The first reads landed (a headless run reads them before its first frame).
    #[default]
    Connected,
}

pub enum LaunchEvent {
    /// `on_ready`, or `setup` for a run that read the server already.
    Start,
    /// Retry: the card's button (`quiet` false), or the app on its own.
    Retry { quiet: bool },
    /// The network came back: start the backoff over.
    NetworkUp,
    /// The window polls at `now`.
    Tick { now: f64 },
    /// The probe answered: the server answered (`Some(true)`), didn't (`Some(false)`), or the
    /// window falls back to the fixture (`None`).
    Landed { reached: Option<bool>, now: f64 },
}

#[derive(Debug, PartialEq)]
pub enum LaunchEffect {
    /// Start the probe and first reads, keeping on for `grace`; the caller enters `Probing`.
    Probe { beta_tabs: bool, again: Option<Again>, grace: Duration },
    /// The first reads are in: connect the stream and the open session.
    Connect,
    /// No server answered, first said: the card and the session's unavailable panel show.
    Unreachable,
    /// The fixture stands in for a server that isn't there.
    Fixture,
    WakeAt(f64),
    /// Something on screen changed.
    Repaint,
}

impl Phase {
    pub fn step(&mut self, event: LaunchEvent, grace: Duration) -> Effects<LaunchEffect> {
        use LaunchEffect::*;
        match (std::mem::take(self), event) {
            (Phase::Waiting { beta_tabs }, LaunchEvent::Start) => Effects::one(Probe { beta_tabs, again: None, grace }),
            (Phase::Connected, LaunchEvent::Start) => Effects::one(Connect),
            (Phase::Offline { retries, .. }, LaunchEvent::Retry { quiet }) => {
                Effects::two(Repaint, Probe { beta_tabs: false, again: Some(Again { retries, quiet }), grace: if quiet { Duration::ZERO } else { grace } })
            }
            (Phase::Offline { .. }, LaunchEvent::NetworkUp) => Effects::one(Probe { beta_tabs: false, again: Some(Again { retries: 0, quiet: true }), grace: Duration::ZERO }),
            (Phase::Offline { retries, retry_at }, LaunchEvent::Tick { now }) if now + 1e-3 >= retry_at => {
                Effects::one(Probe { beta_tabs: false, again: Some(Again { retries, quiet: true }), grace: Duration::ZERO })
            }
            (Phase::Offline { retries, retry_at }, LaunchEvent::Tick { .. }) => {
                *self = Phase::Offline { retries, retry_at };
                Effects::none()
            }
            (Phase::Probing { again, .. }, LaunchEvent::Landed { reached: Some(true), .. }) => {
                *self = Phase::Connected;
                let _ = again;
                Effects::two(Repaint, Connect)
            }
            (Phase::Probing { .. }, LaunchEvent::Landed { reached: None, .. }) => {
                *self = Phase::Connected;
                Effects::two(Repaint, Fixture)
            }
            (Phase::Probing { again, .. }, LaunchEvent::Landed { reached: Some(false), now }) => {
                let retries = again.map_or(0, |a| a.retries);
                let retry_at = now + retry_delay(retries);
                *self = Phase::Offline { retries: retries.saturating_add(1), retry_at };
                // A try the app made on its own that misses again changes nothing on screen.
                if again.is_some_and(|a| a.quiet) { return Effects::one(WakeAt(retry_at)); }
                let mut effects = Effects::two(Repaint, Unreachable);
                effects.push(WakeAt(retry_at));
                effects
            }
            (phase, _) => {
                *self = phase;
                Effects::none()
            }
        }
    }

    /// No server answered: Home shows the card, a session the unavailable panel.
    pub fn offline(&self) -> bool { matches!(self, Phase::Offline { .. } | Phase::Probing { again: Some(_), .. }) }
    /// The card's button says Checking: a probe the person asked for is on its way.
    pub fn checking(&self) -> bool { matches!(self, Phase::Probing { again: None | Some(Again { quiet: false, .. }), .. }) }
    /// The first reads haven't landed.
    pub fn waiting(&self) -> bool { matches!(self, Phase::Waiting { .. } | Phase::Probing { again: None, .. }) }

}

#[derive(Default)]
pub enum PluginSetup {
    #[default]
    Immediate,
    AfterFrame(rune::Waker),
}

#[derive(Default)]
pub struct Launch {
    pub plugins: PluginSetup,
    network_change: Option<std::sync::Arc<network::Change>>,
    /// A quiet poll for network deliveries; unlike an image completion, the inbox
    /// often contains nothing that changes this window.
    pub inbox_wake: Option<rune::Waker>,
    /// Each server's launch, many at once: every server the window has a half of (`servers::runtime`).
    phases: std::collections::HashMap<ServerId, Phase>,
    /// Fall back to the built-in fixture when the background probe finds no server.
    pub fallback_fixture: bool,
    /// Restored session id whose cached messages painted on frame 0 and settle on first live refresh.
    pub cold_transcript: Option<String>,
    /// How long a launch keeps probing the local default address before Home shows the setup
    /// card ([`GRACE`] unless a test sets it): a server that is just starting comes up in this time.
    pub grace: Option<Duration>,
    /// The address the setup card names, in its story.
    pub story_addr: Option<String>,
}

impl Launch {
    pub fn phase(&self, server: &str) -> Option<&Phase> { self.phases.get(server) }
    pub fn set_phase(&mut self, server: &str, phase: Phase) { self.phases.insert(server.to_owned(), phase); }
    pub fn forget(&mut self, server: &str) { self.phases.remove(server); }
}

/// A story's window whose server didn't answer; `checking` while a Retry the person pressed is
/// on its way (its answer never comes).
pub(crate) fn story_offline(s: &mut App, checking: bool) {
    let Some(server) = window_server(s) else { return };
    let phase = if checking {
        let (answer, rx) = std::sync::mpsc::channel();
        // The story's probe stays on its way: its sender lives as long as the app.
        std::mem::forget(answer);
        Phase::Probing { rx, again: Some(Again { retries: 0, quiet: false }) }
    } else {
        Phase::Offline { retries: 0, retry_at: f64::INFINITY }
    };
    s.launch.set_phase(&server, phase);
}

/// How long a launch keeps trying a server it hasn't reached before Home shows the setup card:
/// a server that is just starting comes up in this time. Only the local default address waits:
/// another (a remote server, a tunnel) that refuses is unreachable at once, as Desktop's health
/// check settles on its first answer.
pub const GRACE: Duration = Duration::from_secs(5);
/// How often the setup card tries again on its own.
fn retry_delay(attempt: u32) -> f64 { (0.25 * 2_f64.powi(attempt.min(4) as i32)).min(3.0) }
mod network;
#[cfg(target_os = "windows")]
pub mod io_trace;
pub mod trace;
pub(crate) fn network_available(s:&mut App,tx:&mut Tx) {
    if !s.live() { return; }
    Server::wake_reconnects();
    step(s, LaunchEvent::NetworkUp, tx);
    s.pump_outbox();
}

/// The server `server` stands for (`service.json` when `discover`), probed until it answers or
/// `grace` runs out; true when it answered.
pub(crate) fn probe(server: &Server, discover: bool, grace: Duration) -> (Server, bool) {
    let start = Instant::now();
    let mut pause = Duration::from_millis(250);
    loop {
        let actual = if discover { opencode_kit::local_service::discover_local_server(server) } else { server.clone() };
        if actual.reachable() {
            return (actual, true);
        }
        let left = grace.saturating_sub(start.elapsed());
        if left.is_zero() {
            return (actual, false);
        }
        std::thread::sleep(pause.min(left));
        pause = (pause * 2).min(Duration::from_secs(1));
    }
}

/// `setup`: connects now unless `on_ready` will start the reads (or a synchronous live probe found no server).
pub fn setup(s: &mut App) {
    if !deferred() {
        s.checkout.boot = crate::servers::runtime::Boot::Ready;
    }
    if matches!(window_phase(s), None | Some(Phase::Connected)) {
        start(s, &mut Tx::default());
    }
}

/// The server the window works against now (`servers::runtime`'s checkout), keyed as
/// `reconnect::Links` keys it.
pub(crate) fn window_server(s: &App) -> Option<ServerId> {
    Some(crate::servers::runtime::active(s).as_str().to_owned())
}

fn window_phase(s: &App) -> Option<&Phase> { s.launch.phase(crate::servers::runtime::active(s).as_str()) }

/// The window's server didn't answer: Home shows the card, a session the unavailable panel.
pub fn offline(s: &App) -> bool { window_phase(s).is_some_and(Phase::offline) }
/// The window's first reads haven't landed.
pub fn waiting(s: &App) -> bool { window_phase(s).is_some_and(Phase::waiting) }
/// The card's Retry says Checking.
pub fn checking(s: &App) -> bool { window_phase(s).is_some_and(Phase::checking) }

/// Coalesce a burst into one quiet poll. An ordinary window wake requests a drawable even
/// when the batch only updated a background session; a timer lets Rune skip that present.
pub fn inbox_waker(handle: rune::AppHandle<App>) -> rune::Waker {
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
    let pending = Arc::new(AtomicBool::new(false));
    Arc::new(move || {
        if pending.swap(true, Ordering::AcqRel) { return; }
        let pending = pending.clone();
        handle.update_if(move |_, tx| {
            pending.store(false, Ordering::Release);
            tx.wake_after(Duration::ZERO);
            false
        });
    })
}

/// `on_ready`, after the first frame: starts the probe, Beta tab read and first server reads once.
#[cfg_attr(feature = "hot", allow(dead_code))]
pub fn ready(s: &mut App, tx: &mut Tx) -> bool {
    if s.live() { s.launch.network_change = s.waker.clone().and_then(network::subscribe); }
    // The menu bar was built before the saved shortcuts loaded.
    crate::menubar::refresh();
    // Which Open in apps are installed: the first frame showed the remembered choice.
    s.open_in.detect();
    crate::updater::start_if_packaged(s);
    // Which install this is, for the toasts' clicks; it reads the registry, so after the first frame and off the UI thread.
    crate::notifications::settle_identity();
    // The window's other servers start from its next poll (`servers::runtime::poll`).
    s.checkout.boot = crate::servers::runtime::Boot::Ready;
    start(s, tx);
    // Start the probe first so its I/O overlaps plugin setup. Redraw the newly
    // registered panels and commands without waiting for a server response.
    let deferred = if let PluginSetup::AfterFrame(wake) = std::mem::take(&mut s.launch.plugins) {
        crate::plugin_host::setup(s, wake);
        tx.instant();
        true
    } else {
        false
    };
    // Plugins from folders (`--plugin-dir`, overrides, installed) are read now, on a worker.
    crate::plugin_host::first_frame(s);
    deferred
}

/// Re-probes the local OpenCode server (including any fresh `service.json` written since launch).
pub fn retry(s: &mut App, tx: &mut Tx) {
    step(s, LaunchEvent::Retry { quiet: false }, tx);
}

pub(crate) fn start(s: &mut App, tx: &mut Tx) {
    let Some(server) = window_server(s) else { return connect(s, tx) };
    // A run that read the server before its first frame has connected.
    if s.launch.phase(&server).is_none() {
        s.launch.set_phase(&server, Phase::Connected);
    }
    step(s, LaunchEvent::Start, tx);
}

/// Steps the window's server's launch and carries out what it asks for.
fn step(s: &mut App, event: LaunchEvent, tx: &mut Tx) -> bool {
    let Some(server) = window_server(s) else { return false };
    // Only the local default address may be a server still starting (see `GRACE`).
    let grace = if server == opencode_kit::client::DEFAULT_SERVER || s.launch.grace.is_some() { s.launch.grace.unwrap_or(GRACE) } else { Duration::ZERO };
    let Some(mut phase) = s.launch.phases.remove(&server) else { return false };
    let effects = phase.step(event, grace);
    s.launch.set_phase(&server, phase);
    let mut changed = false;
    for effect in effects {
        match effect {
            LaunchEffect::Probe { beta_tabs, again, grace } => {
                let rx = spawn_probe(s, beta_tabs, grace);
                s.launch.set_phase(&server, Phase::Probing { rx, again });
            }
            LaunchEffect::Connect => connect(s, tx),
            LaunchEffect::Unreachable | LaunchEffect::Fixture => {}
            LaunchEffect::WakeAt(at) => tx.wake_after(Duration::from_secs_f64((at - s.clock).max(0.0))),
            LaunchEffect::Repaint => {
                tx.instant();
                changed = true;
            }
        }
    }
    changed
}

/// The probe, Desktop Beta's tabs and the first reads, on a thread; the window wakes for them.
fn spawn_probe(s: &App, beta_tabs: bool, grace: Duration) -> Receiver<Ready> {
    let (out, rx) = std::sync::mpsc::channel();
    let Source::Live { server, .. } = &s.source else { return rx };
    let (server, wake) = (server.clone(), s.launch.inbox_wake.clone().or_else(|| s.waker.clone()));
    // Only the app's own server may stand down for the fixture, or move to the port
    // `service.json` names.
    let own = crate::servers::runtime::active(s).is_sidecar();
    let fallback = s.launch.fallback_fixture && own;
    let discover_local = own && crate::arg("--server").is_none();
    std::thread::Builder::new()
        .name("opencode-first".into())
        .spawn(move || {
            let (actual, reached) = probe(&server, discover_local, grace);
            let ready = if !reached {
                if fallback {
                    Ready { server: None, beta_tabs: None, first: None }
                } else {
                    let msg = format!("No OpenCode server responded at {}", actual.base);
                    Ready { server: Some(actual), beta_tabs: None, first: Some(Err(msg)) }
                }
            } else {
                let beta_tabs = beta_tabs.then(crate::tabs::from_beta).flatten();
                let first = First::fetch(&actual);
                Ready { server: Some(actual), beta_tabs, first: Some(first) }
            };
            let _ = out.send(ready);
            if let Some(w) = wake {
                w();
            }
        })
        .expect("spawn first reads");
    rx
}

/// `poll`: the first reads land at once (nothing animates in on the first load), then connect.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    if s.launch.network_change.as_ref().is_some_and(|n| n.take()) { network_available(s,tx); }
    let now = s.clock;
    if s.live() && crate::attended() {
        step(s, LaunchEvent::Tick { now }, tx);
    }
    // Every poll comes here: the window's key is borrowed, not copied.
    let Some(Phase::Probing { rx, .. }) = window_phase(s) else { return false };
    let ready = match rx.try_recv() {
        Ok(r) => r,
        Err(TryRecvError::Empty) => return false,
        Err(TryRecvError::Disconnected) => Ready {
            server: None,
            beta_tabs: None,
            first: Some(Err("The first server reads stopped".into())),
        },
    };
    let reached = ready.first.as_ref().map(Result::is_ok);
    rune::marks::mark("server.answered");
    tx.instant();
    let Some(result) = ready.first else {
        step(s, LaunchEvent::Landed { reached, now }, tx);
        s.launch.cold_transcript = None;
        crate::windows::peer::leave_server(s);
        s.transcripts.clear();
        s.tabs.clear();
        s.route = Route::Home;
        s.load_fixture(crate::FIXTURE);
        return true;
    };
    if let (Some(actual), Source::Live { server, .. }) = (ready.server, &mut s.source) {
        *server = actual.clone();
        if crate::servers::runtime::active(s).is_sidecar() {
            crate::servers::runtime::moved(s, &actual);
        }
    }
    if let Some(tabs) = ready.beta_tabs {
        let drafts: Vec<_> = s.tabs.drain(..).filter(crate::tabs::Tab::is_draft).collect();
        s.tabs = tabs;
        s.tabs.extend(drafts);
    }
    match result {
        Ok(first) => {
            s.apply_first(first);
            rune::marks::mark("server.first_reads");
            step(s, LaunchEvent::Landed { reached, now }, tx);
            s.show_outbox();
            s.pump_outbox();
            true
        }
        // The tabs and the page stay, as Desktop never drops a tab while its server is away
        // (`packages/app/src/shell/tabs/tabs.tsx`): Home shows the card, a session whose server
        // hasn't answered the unavailable panel (`view::session_state`).
        Err(e) => {
            let shown = step(s, LaunchEvent::Landed { reached, now }, tx);
            if shown { s.status = e; }
            shown
        }
    }
}
impl App {
    /// Home shows server `key` (a server picked in Settings' switcher, a plugin's `Servers:Connect`):
    /// every server stays connected, and the window's tabs stay, as Desktop never switches a
    /// window to a server (`notes/multi-server.md` §5.1).
    pub(crate) fn focus_server(&mut self, key: &opencode_kit::servers::ServerKey, tx: &mut Tx) {
        if !self.live() {
            return;
        }
        tx.instant();
        crate::projects::home::focus(self, key);
        self.opening = None;
        self.route = Route::Home;
        crate::servers::runtime::switch(self, key);
    }

    /// Attachments and markdown images read through the server, so a window that opens with
    /// a transcript (a torn-out tab's) shows its images before it connects.
    pub(crate) fn set_media_fetch(&mut self) {
        let Source::Live { server, .. } = &self.source else { return };
        let media = server.clone();
        self.attach.set_fetch(std::sync::Arc::new(move |hash: &str| media.media(hash)));
        // Markdown images read through the server as Desktop's `readLocalImage` does; web
        // images and link favicons come from the web, as the browser fetches them for Desktop.
        let files = server.clone();
        self.attach.markdown.set_fetch(
            std::sync::Arc::new(move |directory: &str, path: &str| {
                let encoded = crate::review::data::encode_fs_path(path);
                files.get_bytes(&format!("/api/fs/read/{encoded}?location%5Bdirectory%5D={}", crate::switcher::encode(directory)))
            }),
            std::sync::Arc::new(crate::web_fetch::get),
        );
    }

    pub(crate) fn apply_first(&mut self, f: First) {
        // A window that started in its opener's copy (a torn-out tab's) reads only what it
        // lacks: the copy is live, and newer than these reads.
        let joined = crate::windows::peer::joined(self);
        if !joined || self.projects.is_empty() {
            self.projects = opencode_kit::model::parse_projects(&f.projects);
        }
        if let Some(sessions) = &f.sessions {
            // Sessions past the page (another window's tab, a subagent's parent) stay known,
            // and a session read since keeps what it was marked read up to.
            let mut list = opencode_kit::model::parse_sessions(sessions);
            for n in &mut list {
                if let Some(old) = self.sessions.iter().find(|o| o.id == n.id) {
                    n.viewed = n.viewed.max(old.viewed);
                }
            }
            let rest: Vec<_> = self.sessions.drain(..).filter(|o| !list.iter().any(|n| n.id == o.id)).collect();
            list.extend(rest);
            self.sessions = list;
            self.sidebar_inbox.heard_sessions();
        }
        if let Some(p) = &f.providers {
            self.pickers.load_providers(p.get("data").unwrap_or(p));
        }
        if let Some(c) = &f.config {
            self.pickers.load_config(c);
        }
        if let Some((m, agents)) = &f.models {
            self.pickers.load_live(m.get("data").unwrap_or(m), agents.get("data").unwrap_or(agents));
        }
        if let Some(a) = f.active.as_ref().filter(|_| !joined) {
            let map = a.get("data").unwrap_or(a);
            self.running = map.as_object().into_iter().flatten().map(|(k, _)| k.clone()).filter(|id| !self.stops.pending(id)).collect();
        }
        crate::windows::peer::touch(self);
        // A fresh profile has used no projects yet: Home lists none, as Desktop's
        // `usedProjectIDs` starts empty.
        let saved_projects = match &self.source {
            Source::Live { server, .. } => Some(crate::tabs::load_projects(&server.base).unwrap_or_default()),
            Source::Fixture => None,
        };
        self.init_used_projects(saved_projects);
        if let Route::Session { id, server } = &self.route
            && server == crate::servers::runtime::active(self)
            && !self.tabs.iter().any(|t| t.id == *id)
            && !self.sessions.iter().any(|s| s.id == *id)
        {
            self.launch.cold_transcript = None;
            self.route = Route::Home;
        }
        self.sort_projects();
    }
}

/// With the data in: the event stream, info for tabs past the listed page, the open session,
/// `--session`, and the newest pages.
fn connect(s: &mut App, tx: &mut Tx) {
    // Running sessions (subagents) whose family isn't loaded yet.
    let mut missing: Vec<String> = s.running.iter().filter_map(|id| s.missing_ancestor(id)).collect();
    missing.sort();
    missing.dedup();
    s.set_media_fetch();
    if let (Source::Live { server, requests, writes, .. }, Some(waker)) = (&mut s.source, s.waker.clone()) {
        s.pages.set(session_pages::Pages::live(server.clone(), waker.clone()));
        let inbox_wake = s.launch.inbox_wake.clone().unwrap_or(waker);
        let (r, w, inbox) = crate::windows::connect(server, inbox_wake.clone());
        *writes = Some(w);
        // Tabs for sessions past the listed page: their info, so marks and avatars resolve.
        for t in s.tabs.iter().filter(|t| !t.is_draft() && !s.sessions.iter().any(|x| x.id == t.id)) {
            let _ = r.send(Request::Info(t.id.clone()));
        }
        for id in missing {
            let _ = r.send(Request::Info(id));
        }
        *requests = Some(r);
        s.inbox = Some(inbox);
        crate::windows::peer::join(s, Some(inbox_wake.clone()));
        // A window that started in its opener's copy was woken plainly until now.
        crate::windows::peer::set_waker(s, inbox_wake);
        s.list_unlisted_worktrees();
        // The session open at last quit (see `restore`) subscribes like a click on its tab.
        if let Route::Session { id, server } = s.route.clone()
            && server == *crate::servers::runtime::active(s)
        {
            crate::telemetry::open_restored(&id, s.transcripts.get(&id).map_or(0, |t| t.messages.len()));
            s.open(&id, tx);
        }
    }
    // `--session <id or title words>` opens a session of the app's own server at start.
    if let Some(raw) = arg("--session").filter(|_| crate::servers::runtime::active(s).is_sidecar()) {
        let q = raw.to_lowercase();
        let found = s.sessions.iter().find(|x| x.id == raw).or_else(|| s.sessions.iter().find(|x| x.title.to_lowercase().contains(&q))).map(|x| x.id.clone());
        if let Some(id) = found {
            s.open(&id, tx);
        }
    }
    // Small newest pages, on the speculative lane after foreground startup work is queued.
    let mut recent: Vec<_> = s.sessions.iter().filter(|s| s.parent.is_none()).collect();
    recent.sort_by_key(|s| std::cmp::Reverse(s.updated));
    for session in recent.into_iter().take(session_pages::WARM) {
        s.prefetch_session(&session.id);
    }
    // Deterministic headless snapshots have no wall-clock event loop to await the worker.
    if headless()
        && let (Route::Session { id, server: on }, Source::Live { server, .. }) = (&s.route, &s.source)
        && on == crate::servers::runtime::active(s)
        && !s.transcripts.contains_key(id)
        && let Ok(v) = server.read(&format!("/api/session/{id}/message?limit={}&order=desc", session_pages::LATEST))
    {
        s.transcripts.insert(id.clone(), Transcript { session: id.clone(), messages: opencode_kit::model::parse_messages(&v) });
    }
    // And what waits in its inbox: steers, and shells and subagents that came back.
    if headless()
        && let (Route::Session { id, server: on }, Source::Live { server, .. }) = (&s.route, &s.source)
        && on == crate::servers::runtime::active(s)
        && let Ok(v) = server.read(&format!("/api/session/{id}/inbox"))
    {
        let id = id.clone();
        let items = v["data"].as_array().or_else(|| v.as_array()).cloned().unwrap_or_default();
        s.apply_inbox("session.inbox.listed", &serde_json::json!({ "sessionID": id, "items": items }), &id);
        if let Some(dir) = s.session(&id).map(|i| i.directory.clone()) {
            crate::subagents::shells::list(s, &dir);
        }
    }
}


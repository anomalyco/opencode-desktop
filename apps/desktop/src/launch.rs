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

use crate::{App, Route, Source, arg, headless, session_pages};

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

/// The app icon full bleed, as Windows draws an app's icon (no macOS grid or shadow).
#[cfg(target_os = "windows")]
pub const WINDOWS_ICON: &[u8] = include_bytes!("../assets/icon/art-512.png");

/// Windows: the windows' title bar, taskbar and Alt+Tab icon.
#[cfg(target_os = "windows")]
pub fn apply_app_icon() {
    let _ = rune::set_app_icon(WINDOWS_ICON);
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

#[cfg(test)]
impl First {
    pub fn from_parts(projects: Value, sessions: Option<Value>) -> First {
        First { projects, sessions, models: None, providers: None, active: None, config: None }
    }
}

#[cfg(test)]
impl Launch {
    pub fn set_first(&mut self, rx: Receiver<Ready>) {
        self.first = Some(rx);
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

#[derive(Default)]
pub struct Launch {
    network_change: Option<std::sync::Arc<network::Change>>,
    retries: u32,
    started: bool,
    /// A quiet poll for network deliveries; unlike an image completion, the inbox
    /// often contains nothing that changes this window.
    pub inbox_wake: Option<rune::Waker>,
    /// The `on_ready` probe, Beta tab read and first server reads, on their way.
    first: Option<Receiver<Ready>>,
    /// The first reads wait for `on_ready` (a windowed live run).
    pub waiting: bool,
    /// Fall back to the built-in fixture when the background probe finds no server.
    pub fallback_fixture: bool,
    /// True while a live launch found no local OpenCode server (Home shows the setup card).
    pub offline: bool,
    /// No tabs were saved: read Desktop Beta's `drafts.sqlite` tabs in `on_ready`. Without them a
    /// first launch has none, and shows Home, as Desktop's does.
    pub beta_tabs: bool,
    /// Restored session id whose cached messages painted on frame 0 and settle on first live refresh.
    pub cold_transcript: Option<String>,
    /// Tabs put aside while the setup card shows, back when the server answers.
    held_tabs: Vec<crate::tabs::Tab>,
    /// A probe the app started on its own while offline: the card's button doesn't say Checking.
    pub quiet: bool,
    /// When the setup card next probes on its own (`App::clock`).
    next_retry: f64,
    /// How long a launch keeps probing before the setup card ([`GRACE`] unless a test sets it).
    pub grace: Option<Duration>,
    /// The address the setup card names, in its story.
    pub story_addr: Option<String>,
    /// The event stream after connecting (`reconnect`).
    pub link: crate::reconnect::Link,
}

/// How long a launch keeps trying a server it hasn't reached before Home shows the setup card:
/// a tunnel or a server that is just starting comes up in this time.
pub const GRACE: Duration = Duration::from_secs(5);
/// How often the setup card tries again on its own.
fn retry_delay(attempt: u32) -> f64 { (0.25 * 2_f64.powi(attempt.min(4) as i32)).min(3.0) }

mod network;
pub(crate) fn network_available(s:&mut App,tx:&mut Tx) {
    if !s.live() { return; }
    Server::wake_reconnects();
    if s.launch.offline && s.launch.first.is_none() { s.launch.retries=0; s.launch.quiet=true; again(s,tx); }
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
    if !s.launch.waiting && !s.launch.offline {
        start(s, &mut Tx::default());
    }
}

/// Coalesce a burst into one quiet poll. An ordinary window wake requests a drawable even
/// when the batch only updated a background session; a timer lets Rune skip that present.
#[cfg(not(feature = "hot"))]
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
pub fn ready(s: &mut App, tx: &mut Tx) {
    if s.live() { s.launch.network_change = s.waker.clone().and_then(network::subscribe); }
    // The menu bar was built before the saved shortcuts loaded.
    crate::menubar::refresh();
    crate::updater::start_if_packaged(s);
    start(s, tx);
}

/// Re-probes the local OpenCode server (including any fresh `service.json` written since launch).
pub fn retry(s: &mut App, tx: &mut Tx) {
    s.launch.quiet = false;
    if s.launch.first.is_none() {
        again(s, tx);
    }
}

fn again(s: &mut App, tx: &mut Tx) {
    s.launch.started = false;
    s.launch.waiting = true;
    tx.instant();
    start(s, tx);
}

pub(crate) fn start(s: &mut App, tx: &mut Tx) {
    if std::mem::replace(&mut s.launch.started, true) {
        return;
    }
    let (Source::Live { server, .. }, true) = (&s.source, s.launch.waiting) else { return connect(s, tx) };
    let (out, rx) = std::sync::mpsc::channel();
    let (server, wake) = (server.clone(), s.waker.clone());
    let (fallback, beta_tabs) = (s.launch.fallback_fixture, std::mem::take(&mut s.launch.beta_tabs));
    let discover_local = s.server_key.is_none() && crate::arg("--server").is_none();
    let grace = if s.launch.quiet { Duration::ZERO } else { s.launch.grace.unwrap_or(GRACE) };
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
    s.launch.first = Some(rx);
}

/// `poll`: the first reads land at once (nothing animates in on the first load), then connect.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    if s.launch.network_change.as_ref().is_some_and(|n| n.take()) { network_available(s,tx); }
    if s.launch.offline && s.launch.first.is_none() && s.live() && !crate::headless() {
        if s.clock + 1e-3 < s.launch.next_retry {
            return false;
        }
        s.launch.quiet = true;
        again(s, tx);
    }
    let Some(rx) = &s.launch.first else { return false };
    let ready = match rx.try_recv() {
        Ok(r) => r,
        Err(TryRecvError::Empty) => return false,
        Err(TryRecvError::Disconnected) => Ready {
            server: None,
            beta_tabs: None,
            first: Some(Err("The first server reads stopped".into())),
        },
    };
    s.launch.first = None;
    s.launch.waiting = false;
    let quiet = std::mem::take(&mut s.launch.quiet);
    tx.instant();
    let Some(result) = ready.first else {
        s.launch.cold_transcript = None;
        crate::windows::shared::leave(s);
        s.transcripts.clear();
        s.tabs.clear();
        s.route = Route::Home;
        s.load_fixture(crate::FIXTURE);
        return true;
    };
    if let (Some(actual), Source::Live { server, .. }) = (ready.server, &mut s.source) {
        *server = actual;
    }
    if let Some(tabs) = ready.beta_tabs {
        let drafts: Vec<_> = s.tabs.drain(..).filter(crate::tabs::Tab::is_draft).collect();
        s.tabs = tabs;
        s.tabs.extend(drafts);
    }
    match result {
        Ok(first) => {
            s.launch.offline = false;
            s.launch.retries = 0;
            if s.tabs.is_empty() && !s.launch.held_tabs.is_empty() {
                s.tabs = std::mem::take(&mut s.launch.held_tabs);
            }
            s.apply_first(first);
            connect(s, tx);
            s.show_outbox();
            s.pump_outbox();
        }
        Err(e) => {
            let delay = retry_delay(s.launch.retries);
            s.launch.retries = s.launch.retries.saturating_add(1);
            s.launch.next_retry = s.clock + delay;
            tx.wake_after(Duration::from_secs_f64(delay));
            if quiet {
                return false;
            }
            s.launch.offline = true;
            if matches!(s.route, Route::Session(_)) && !s.transcripts.is_empty() {
                s.status = e;
                return true;
            }
            s.launch.cold_transcript = None;
            // The other windows keep what they show while this one waits for the server.
            crate::windows::shared::leave(s);
            s.transcripts.clear();
            if s.launch.held_tabs.is_empty() {
                s.launch.held_tabs = std::mem::take(&mut s.tabs);
            }
            s.tabs.clear();
            s.route = Route::Home;
            s.status = e;
        }
    }
    true
}

impl App {
    /// Connects the window to `server` in place of the one it has: what it held of the old one
    /// goes, its own saved tabs come, and Home shows its projects. `key` is a
    /// contributed server's (`<plugin>:<id>`), `None` for the app's own. The app holds one server
    /// at a time; Desktop keeps them all connected at once.
    pub(crate) fn switch_server(&mut self, server: Server, key: Option<String>, tx: &mut Tx) {
        if !self.live() {
            return;
        }
        if let Source::Live { server: old, .. } = &self.source {
            if self.persists() {
                crate::tabs::save(&self.window, &old.base, &self.tabs);
            }
            if self.server_key.is_none() {
                self.own_server = Some(old.clone());
            }
        }
        tx.instant();
        self.close_settings();
        self.opening = None;
        self.route = Route::Home;
        self.tabs = crate::tabs::load(&self.window, &server.base).unwrap_or_default();
        // Its windows keep the old server's data; this one starts empty for the new server.
        crate::windows::shared::leave(self);
        self.sessions.clear();
        self.transcripts.clear();
        self.projects.clear();
        self.running.clear();
        self.requests.clear();
        self.inbox = None;
        self.server_key = key;
        self.plugins.forget_reads();
        if let Source::Live { server: s, requests, writes, .. } = &mut self.source {
            *s = server;
            *requests = None;
            *writes = None;
        }
        self.launch.cold_transcript = None;
        self.launch.held_tabs.clear();
        self.open_outbox();
        self.launch.offline = false;
        // Desktop Beta's saved tabs are the app's own server's.
        self.launch.beta_tabs = false;
        self.launch.started = false;
        self.launch.waiting = true;
        start(self, tx);
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
        *self.projects = opencode_kit::model::parse_projects(&f.projects);
        if let Some(sessions) = &f.sessions {
            // Sessions past the page (another window's tab, a subagent's parent) stay known.
            let mut list = opencode_kit::model::parse_sessions(sessions);
            let rest: Vec<_> = self.sessions.drain(..).filter(|o| !list.iter().any(|n| n.id == o.id)).collect();
            list.extend(rest);
            *self.sessions = list;
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
        if let Some(a) = &f.active {
            let map = a.get("data").unwrap_or(a);
            *self.running = map.as_object().into_iter().flatten().map(|(k, _)| k.clone()).collect();
        }
        // A fresh profile has used no projects yet: Home lists none, as Desktop's
        // `usedProjectIDs` starts empty.
        let saved_projects = match &self.source {
            Source::Live { server, .. } => Some(crate::tabs::load_projects(&server.base).unwrap_or_default()),
            Source::Fixture => None,
        };
        self.init_used_projects(saved_projects);
        if let Route::Session(id) = &self.route
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
        let (r, w, inbox) = crate::windows::connect(server, inbox_wake);
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
        crate::windows::shared::join(s);
        s.list_unlisted_worktrees();
        // The session open at last quit (see `restore`) subscribes like a click on its tab.
        if let Route::Session(id) = s.route.clone() {
            crate::telemetry::open_restored(&id, s.transcripts.get(&id).map_or(0, |t| t.messages.len()));
            s.open(&id, tx);
        }
    }
    // `--session <id or title words>` opens a session at start.
    if let Some(raw) = arg("--session") {
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
        && let (Route::Session(id), Source::Live { server, .. }) = (&s.route, &s.source)
        && !s.transcripts.contains_key(id)
        && let Ok(v) = server.read(&format!("/api/session/{id}/message?limit={}&order=desc", session_pages::LATEST))
    {
        s.transcripts.insert(id.clone(), Transcript { session: id.clone(), messages: opencode_kit::model::parse_messages(&v) });
    }
    // And what waits in its inbox: steers, and shells and subagents that came back.
    if headless()
        && let (Route::Session(id), Source::Live { server, .. }) = (&s.route, &s.source)
        && let Ok(v) = server.read(&format!("/api/session/{id}/inbox"))
    {
        let id = id.clone();
        let items = v["data"].as_array().or_else(|| v.as_array()).cloned().unwrap_or_default();
        s.apply_inbox("session.inbox.listed", &serde_json::json!({ "sessionID": id, "items": items }), &id);
    }
}

#[cfg(test)]
#[path = "launch_tests.rs"]
mod tests;

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

/// `opencode-desktop --version`: the version and the release channel it was built for.
pub fn print_version_if_asked() {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("opencode-rune {} ({})", env!("CARGO_PKG_VERSION"), crate::updater::build_channel());
        std::process::exit(0);
    }
}

/// Window title (`"OpenCode (Rune)"` on `main`, `"OpenCode (Rune) · <branch>"` on PR builds).
pub fn window_title() -> &'static str {
    option_env!("OPENCODE_RUNE_WINDOW_TITLE").unwrap_or("OpenCode (Rune)")
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
        let sessions = get("/api/session?limit=120").ok();
        let models = get("/api/model").ok().map(|m| (m, get("/api/agent").unwrap_or_default()));
        // The model list names a provider by its id; this names it as a person reads it.
        let providers = get("/api/provider").ok();
        let active = get("/api/session/active").ok();
        Ok(First { projects, sessions, models, providers, active })
    }
}

#[cfg(test)]
impl First {
    pub fn from_parts(projects: Value, sessions: Option<Value>) -> First {
        First { projects, sessions, models: None, providers: None, active: None }
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
    started: bool,
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
const RETRY_EVERY: f64 = 3.0;

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

/// `on_ready`, after the first frame: starts the probe, Beta tab read and first server reads once.
#[cfg_attr(feature = "hot", allow(dead_code))]
pub fn ready(s: &mut App, tx: &mut Tx) {
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
    if let Source::Live { server, .. } = &mut s.source {
        let addr = crate::arg("--server").unwrap_or_else(opencode_kit::client::default_server);
        *server = Server::new(&addr).with_media_refs();
    }
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
    let discover_local = crate::arg("--server").is_none();
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
            if s.tabs.is_empty() && !s.launch.held_tabs.is_empty() {
                s.tabs = std::mem::take(&mut s.launch.held_tabs);
            }
            s.apply_first(first);
            connect(s, tx);
        }
        Err(e) => {
            s.launch.next_retry = s.clock + RETRY_EVERY;
            tx.wake_after(Duration::from_secs_f64(RETRY_EVERY));
            if quiet {
                return false;
            }
            s.launch.offline = true;
            s.launch.cold_transcript = None;
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
    pub(crate) fn apply_first(&mut self, f: First) {
        self.projects = opencode_kit::model::parse_projects(&f.projects);
        if let Some(sessions) = &f.sessions {
            self.sessions = opencode_kit::model::parse_sessions(sessions);
            self.sidebar_inbox.heard_sessions();
        }
        if let Some(p) = &f.providers {
            self.pickers.load_providers(p.get("data").unwrap_or(p));
        }
        if let Some((m, agents)) = &f.models {
            self.pickers.load_live(m.get("data").unwrap_or(m), agents.get("data").unwrap_or(agents));
        }
        if let Some(a) = &f.active {
            let map = a.get("data").unwrap_or(a);
            self.running = map.as_object().into_iter().flatten().map(|(k, _)| k.clone()).collect();
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
    if let (Source::Live { server, requests, writes, .. }, Some(waker)) = (&mut s.source, s.waker.clone()) {
        s.pages.set(session_pages::Pages::live(server.clone(), waker.clone()));
        let media = server.clone();
        s.attach.set_fetch(std::sync::Arc::new(move |hash: &str| media.media(hash)));
        // Markdown images read through the server as Desktop's `readLocalImage` does; web
        // images and link favicons come from the web, as the browser fetches them for Desktop.
        let files = server.clone();
        s.attach.markdown.set_fetch(
            std::sync::Arc::new(move |directory: &str, path: &str| {
                let encoded = crate::review::data::encode_fs_path(path);
                files.get_bytes(&format!("/api/fs/read/{encoded}?location%5Bdirectory%5D={}", crate::switcher::encode(directory)))
            }),
            std::sync::Arc::new(crate::web_fetch::get),
        );
        let (r, w, inbox) = opencode_kit::client::connect_with_idle(server.clone(), waker, crate::reconnect::STREAM_IDLE);
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
}

#[cfg(test)]
#[path = "launch_tests.rs"]
mod tests;

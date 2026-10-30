//! Launch order. A window's first frame draws while the window is created, so a windowed live
//! run reads only local files before it (saved tabs, zoom, route and drafts). `on_ready` then
//! starts the first server reads; when they land, the event stream, the open session's page and
//! the prefetch start (`connect`). Headless runs and hot builds have no `on_ready`: they read the
//! server before the first frame, as a snapshot needs, and connect from `setup`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use opencode_kit::client::{Request, Server};
use opencode_kit::model::Transcript;
use rune::Tx;
use serde_json::Value;

use crate::{App, Route, Source, arg, headless, session_pages};

static DEFERRED: AtomicBool = AtomicBool::new(false);

/// `main`, for runs that call `on_ready`: the server reads wait for it.
#[cfg_attr(feature = "hot", allow(dead_code))]
pub fn defer() {
    DEFERRED.store(true, Ordering::Relaxed);
}

/// Whether `App::load` leaves the server reads to `on_ready`.
pub fn deferred() -> bool {
    DEFERRED.load(Ordering::Relaxed)
}

/// The first reads: projects, sessions, models and agents, and what's running.
pub struct First {
    projects: Value,
    sessions: Option<Value>,
    models: Option<(Value, Value)>,
    active: Option<Value>,
}

impl First {
    pub fn fetch(server: &Server) -> Result<First, String> {
        let projects = server.get("/api/project")?;
        let sessions = server.get("/api/session?limit=120").ok();
        let models = server.get("/api/model").ok().map(|m| (m, server.get("/api/agent").unwrap_or_default()));
        let active = server.get("/api/session/active").ok();
        Ok(First { projects, sessions, models, active })
    }
}

/// What `on_ready`'s background startup worker brings back: Desktop Beta's tabs (when no
/// `tabs.json` exists yet) and the first server reads, or `first: None` when the server wasn't
/// reachable and the window should fall back to the fixture.
pub struct Ready {
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
    /// Read Desktop Beta's `drafts.sqlite` tabs in `on_ready` before falling back to recent sessions.
    pub beta_tabs: bool,
    /// No tabs were saved: the first reads' recent sessions become them.
    pub recent_tabs: bool,
    /// Restored session id whose cached messages painted on frame 0 and settle on first live refresh.
    pub cold_transcript: Option<String>,
}

/// `setup`: connects now unless `on_ready` will start the reads.
pub fn setup(s: &mut App) {
    if !s.launch.waiting {
        start(s, &mut Tx::default());
    }
}

/// `on_ready`, after the first frame: starts the probe, Beta tab read and first server reads once.
#[cfg_attr(feature = "hot", allow(dead_code))]
pub fn ready(s: &mut App, tx: &mut Tx) {
    start(s, tx);
}

fn start(s: &mut App, tx: &mut Tx) {
    if std::mem::replace(&mut s.launch.started, true) {
        return;
    }
    let (Source::Live { server, .. }, true) = (&s.source, s.launch.waiting) else { return connect(s, tx) };
    let (out, rx) = std::sync::mpsc::channel();
    let (server, wake) = (server.clone(), s.waker.clone());
    let (fallback, beta_tabs) = (s.launch.fallback_fixture, std::mem::take(&mut s.launch.beta_tabs));
    std::thread::Builder::new()
        .name("opencode-first".into())
        .spawn(move || {
            let ready = if fallback && !server.reachable() {
                Ready { beta_tabs: None, first: None }
            } else {
                let beta_tabs = beta_tabs.then(crate::tabs::from_beta).flatten();
                Ready { beta_tabs, first: Some(First::fetch(&server)) }
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
    let Some(rx) = &s.launch.first else { return false };
    let ready = match rx.try_recv() {
        Ok(r) => r,
        Err(TryRecvError::Empty) => return false,
        Err(TryRecvError::Disconnected) => Ready { beta_tabs: None, first: Some(Err("The first server reads stopped".into())) },
    };
    s.launch.first = None;
    s.launch.waiting = false;
    tx.instant();
    let Some(result) = ready.first else {
        s.launch.cold_transcript = None;
        s.transcripts.clear();
        s.tabs.clear();
        s.route = Route::Home;
        s.load_fixture(crate::FIXTURE);
        return true;
    };
    if let Some(tabs) = ready.beta_tabs {
        s.tabs = tabs;
        s.launch.recent_tabs = false;
    }
    match result {
        Ok(first) => {
            s.apply_first(first);
            connect(s, tx);
        }
        Err(e) => s.status = e,
    }
    true
}

impl App {
    pub(crate) fn apply_first(&mut self, f: First) {
        self.projects = opencode_kit::model::parse_projects(&f.projects);
        if let Some(sessions) = &f.sessions {
            self.sessions = opencode_kit::model::parse_sessions(sessions);
        }
        if let Some((m, agents)) = &f.models {
            self.pickers.load_live(m.get("data").unwrap_or(m), agents.get("data").unwrap_or(agents));
        }
        if let Some(a) = &f.active {
            let map = a.get("data").unwrap_or(a);
            self.running = map.as_object().into_iter().flatten().map(|(k, _)| k.clone()).collect();
        }
        if std::mem::take(&mut self.launch.recent_tabs) {
            self.tabs = self.recent_tabs();
        }
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
        let (r, w, inbox) = opencode_kit::client::connect(server.clone(), waker);
        *writes = Some(w);
        // Tabs for sessions past the listed page: their info, so marks and avatars resolve.
        for t in s.tabs.iter().filter(|t| !s.sessions.iter().any(|x| x.id == t.id)) {
            let _ = r.send(Request::Info(t.id.clone()));
        }
        for id in missing {
            let _ = r.send(Request::Info(id));
        }
        *requests = Some(r);
        s.inbox = Some(inbox);
        // The session open at last quit (see `restore`) subscribes like a click on its tab.
        if let Route::Session(id) = s.route.clone() {
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

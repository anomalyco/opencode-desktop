//! System notifications, as OpenCode Desktop posts them (`shell/notifications/notification.tsx` and
//! the desktop platform's `notify`): when an agent finishes (off by default), when a permission is
//! asked (on) and when a run fails (off), for a root session with an open tab, and only while the
//! window isn't focused. A click brings the app forward and opens that session. At most 40 stay
//! in Notification Center; posting another takes the oldest out.
//!
//! Only a live window with a person at it posts (`App::persists`), through `rune_mac::notify`.
//! Tests put a fake `Sink` in.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use rune::Tx;
use serde_json::{Value, json};

use crate::App;

/// Desktop's `MAX_ACTIVE_NOTIFICATIONS`.
pub const CAP: usize = 40;
/// Permission requests already posted, so a repeated `permission.asked` doesn't post twice.
const ASKED: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The agent finished (`session.execution.succeeded`).
    Agent,
    /// A permission is asked (`permission.asked`).
    Permission,
    /// A run failed (`session.execution.failed`).
    Error,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Agent => "agent",
            Kind::Permission => "permissions",
            Kind::Error => "errors",
        }
    }

    /// Desktop's titles (`notification.session.responseReady.title` and friends).
    pub fn title(self) -> &'static str {
        match self {
            Kind::Agent => "Response ready",
            Kind::Permission => "Permission required",
            Kind::Error => "Session error",
        }
    }
}

/// Which kinds notify: Settings › Notifications, saved as Desktop's `notifications` object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kinds {
    pub agent: bool,
    pub permissions: bool,
    pub errors: bool,
}

impl Default for Kinds {
    fn default() -> Self {
        Kinds { agent: false, permissions: true, errors: false }
    }
}

impl Kinds {
    pub fn get(&self, kind: Kind) -> bool {
        match kind {
            Kind::Agent => self.agent,
            Kind::Permission => self.permissions,
            Kind::Error => self.errors,
        }
    }

    pub fn set(&mut self, kind: Kind, on: bool) {
        match kind {
            Kind::Agent => self.agent = on,
            Kind::Permission => self.permissions = on,
            Kind::Error => self.errors = on,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({ "agent": self.agent, "permissions": self.permissions, "errors": self.errors })
    }

    pub fn from_json(v: &Value) -> Kinds {
        let d = Kinds::default();
        Kinds {
            agent: v["agent"].as_bool().unwrap_or(d.agent),
            permissions: v["permissions"].as_bool().unwrap_or(d.permissions),
            errors: v["errors"].as_bool().unwrap_or(d.errors),
        }
    }
}

/// One notification to post: what it says and the session a click opens.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub id: String,
    pub kind: Kind,
    pub body: String,
    pub session: String,
}

/// Where notifications go: the system, or a test's list.
pub trait Sink {
    /// Whether the window is focused right now; nothing posts while it is.
    fn focused(&self) -> bool;
    fn post(&mut self, note: &Note, server: &str);
    fn remove(&mut self, id: &str);
}

#[derive(Default)]
pub struct Notifier {
    sink: Option<Box<dyn Sink>>,
    /// Whether this run looked for the system's notifications yet.
    tried: bool,
    /// Ids posted this run, oldest first, at most `CAP`.
    shown: VecDeque<String>,
    asked: VecDeque<String>,
    posted: u64,
    /// Sessions whose notification was clicked, from the click's thread.
    clicks: Arc<Mutex<Vec<(String, String)>>>,
}

impl Notifier {
    #[cfg(test)]
    pub fn with_sink(sink: impl Sink + 'static) -> Notifier {
        Notifier { sink: Some(Box::new(sink)), ..Default::default() }
    }

    /// A click on a notification for `session` on `server`, from any thread.
    fn click(clicks: &Mutex<Vec<(String, String)>>, session: String, server: String) {
        clicks.lock().unwrap_or_else(|e| e.into_inner()).push((session, server));
    }

    fn post(&mut self, kind: Kind, session: &str, body: String, server: &str) {
        let Some(sink) = self.sink.as_mut() else { return };
        if sink.focused() {
            return;
        }
        if self.shown.len() >= CAP
            && let Some(oldest) = self.shown.pop_front()
        {
            sink.remove(&oldest);
        }
        self.posted += 1;
        let note = Note { id: format!("opencode-rune-{}-{}", std::process::id(), self.posted), kind, body, session: session.to_owned() };
        sink.post(&note, server);
        self.shown.push_back(note.id);
    }
}

impl App {
    fn server_base(&self) -> &str {
        match &self.source {
            crate::Source::Live { server, .. } => &server.base,
            crate::Source::Fixture => "",
        }
    }

    /// A session with a tab open, as Desktop asks of every notification: its title, or None.
    fn tab_title(&self, id: &str) -> Option<String> {
        let tab = self.tabs.iter().find(|t| t.id == id)?;
        let title = self.session(id).map_or(tab.title.as_str(), |s| s.title.as_str());
        Some(title.to_owned())
    }

    /// Whether a direct child of `id` is still running: its report wakes `id` again, so it isn't done.
    fn running_child(&self, id: &str) -> bool {
        self.running.iter().any(|c| c != id && self.session(c).and_then(|s| s.parent.as_deref()) == Some(id))
    }
}

/// What `ty` posts, if anything: the kind, the session a click opens and the body.
fn decide(app: &App, ty: &str, data: &Value, sid: &str) -> Option<(Kind, String, String)> {
    let is_root = |id: &str| app.session(id).is_none_or(|s| s.parent.is_none());
    match ty {
        "session.execution.succeeded" if app.settings.notify.agent => {
            if app.running_child(sid) || !is_root(sid) {
                return None;
            }
            let title = app.tab_title(sid)?;
            Some((Kind::Agent, sid.to_owned(), if title.is_empty() { sid.to_owned() } else { title }))
        }
        "session.execution.failed" if app.settings.notify.errors => {
            if !is_root(sid) {
                return None;
            }
            let title = app.tab_title(sid)?;
            Some((Kind::Error, sid.to_owned(), if title.is_empty() { "An error occurred".to_owned() } else { title }))
        }
        // A subagent's request waits on its root's page, so it notifies for the root.
        "permission.asked" if app.settings.notify.permissions => {
            let root = app.root_of(data["sessionID"].as_str().unwrap_or(sid)).to_owned();
            let title = app.tab_title(&root)?;
            Some((Kind::Permission, root.clone(), if title.is_empty() { root } else { title }))
        }
        _ => None,
    }
}

/// Posts the notification `ty` calls for, if any (from `App::apply_event`).
pub fn event(app: &mut App, ty: &str, data: &Value, sid: &str) {
    if app.notify.sink.is_none() {
        return;
    }
    if ty == "permission.asked" {
        let id = data["id"].as_str().unwrap_or("").to_owned();
        if id.is_empty() || app.notify.asked.contains(&id) {
            return;
        }
        if app.notify.asked.len() >= ASKED {
            app.notify.asked.pop_front();
        }
        app.notify.asked.push_back(id);
    }
    let Some((kind, session, body)) = decide(app, ty, data, sid) else { return };
    let server = app.server_base().to_owned();
    app.notify.post(kind, &session, body, &server);
}

/// Opens clicked notifications' sessions, and connects the system the first time a live window
/// polls (from `poll`).
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    if !app.notify.tried && app.persists() {
        app.notify.tried = true;
        install(app);
    }
    let clicks = std::mem::take(&mut *app.notify.clicks.lock().unwrap_or_else(|e| e.into_inner()));
    let server = app.server_base().to_owned();
    let mut opened = false;
    for (session, from) in clicks {
        // A notification left from a run against another server.
        if session.is_empty() || (!from.is_empty() && from != server) {
            continue;
        }
        if app.live() {
            rune_mac::notify::bring_to_front();
        }
        app.close_settings();
        app.open(&session, tx);
        opened = true;
    }
    opened
}

fn install(app: &mut App) {
    if !rune_mac::notify::available() {
        return;
    }
    let clicks = app.notify.clicks.clone();
    let waker = app.waker.clone();
    rune_mac::notify::on_click(move |c| {
        let (session, server) = (c.get("session").unwrap_or("").to_owned(), c.get("server").unwrap_or("").to_owned());
        Notifier::click(&clicks, session, server);
        if let Some(w) = &waker {
            w();
        }
    });
    app.notify.sink = Some(Box::new(System));
}

struct System;

impl Sink for System {
    fn focused(&self) -> bool {
        rune_mac::notify::app_focused()
    }

    fn post(&mut self, note: &Note, server: &str) {
        let n = rune_mac::notify::Notification::new(&note.id, note.kind.title())
            .body(&note.body)
            .info("session", &note.session)
            .info("server", server);
        rune_mac::notify::post(&n);
    }

    fn remove(&mut self, id: &str) {
        rune_mac::notify::remove(&[id]);
    }
}

#[cfg(test)]
#[path = "notifications_tests.rs"]
mod tests;

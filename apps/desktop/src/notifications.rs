//! System notifications, as OpenCode Desktop posts them (`shell/notifications/notification.tsx` and
//! the desktop platform's `notify`): when an agent finishes (off by default), when a permission is
//! asked (on), when the agent asks a question or an MCP server asks for input (on) and when a run
//! fails (off), for a session with an open tab, and only while the window isn't focused. A click
//! brings the app forward and opens that session. At most 40 stay in the system's list; posting
//! another takes the oldest out.
//!
//! Only a live window with a person at it posts (`App::persists`), through `rune_notify`:
//! Notification Center on macOS, toasts on Windows (`identify` gives the app its name and icon
//! there). Tests put a fake `Sink` in.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use rune::Tx;
use serde_json::{Value, json};

use crate::App;

/// Desktop's `MAX_ACTIVE_NOTIFICATIONS`.
pub const CAP: usize = 40;
/// Permission requests already posted, so a repeated `permission.asked` doesn't post twice.
const ASKED: usize = 500;

/// What a notification is about, as Settings › Notifications groups them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The agent finished (`session.execution.succeeded`).
    Agent,
    /// A permission is asked (`permission.asked`, or a web search provider's form).
    Permission,
    /// The agent asks a question, or an MCP server asks for input (`form.created`).
    Question,
    /// A run failed (`session.execution.failed`).
    Error,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Agent => "agent",
            Kind::Permission => "permissions",
            Kind::Question => "questions",
            Kind::Error => "errors",
        }
    }

    /// Desktop's titles (`notification.session.responseReady.title` and friends).
    pub fn title(self) -> &'static str {
        match self {
            Kind::Agent => "Response ready",
            Kind::Permission => "Permission required",
            Kind::Question => "Question requires an answer",
            Kind::Error => "Session error",
        }
    }
}

/// Which kinds notify: Settings › Notifications, saved as Desktop's `notifications` object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kinds {
    pub agent: bool,
    pub permissions: bool,
    pub questions: bool,
    pub errors: bool,
}

impl Default for Kinds {
    fn default() -> Self {
        Kinds { agent: false, permissions: true, questions: true, errors: false }
    }
}

impl Kinds {
    pub fn get(&self, kind: Kind) -> bool {
        match kind {
            Kind::Agent => self.agent,
            Kind::Permission => self.permissions,
            Kind::Question => self.questions,
            Kind::Error => self.errors,
        }
    }

    pub fn set(&mut self, kind: Kind, on: bool) {
        match kind {
            Kind::Agent => self.agent = on,
            Kind::Permission => self.permissions = on,
            Kind::Question => self.questions = on,
            Kind::Error => self.errors = on,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({ "agent": self.agent, "permissions": self.permissions, "questions": self.questions, "errors": self.errors })
    }

    pub fn from_json(v: &Value) -> Kinds {
        let d = Kinds::default();
        Kinds {
            agent: v["agent"].as_bool().unwrap_or(d.agent),
            permissions: v["permissions"].as_bool().unwrap_or(d.permissions),
            questions: v["questions"].as_bool().unwrap_or(d.questions),
            errors: v["errors"].as_bool().unwrap_or(d.errors),
        }
    }
}

/// One notification to post: what it says and the session a click opens.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub id: String,
    pub kind: Kind,
    /// Desktop's title for what asked: usually `kind`'s, "Input requested" for an MCP server's form.
    pub title: &'static str,
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

    fn post(&mut self, kind: Kind, title: &'static str, session: &str, body: String, server: &str) {
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
        let note = Note { id: format!("opencode-rune-{}-{}", std::process::id(), self.posted), kind, title, body, session: session.to_owned() };
        sink.post(&note, server);
        self.shown.push_back(note.id);
    }
}

impl App {
    pub(crate) fn server_base(&self) -> &str {
        match &self.source {
            crate::Source::Live { server, .. } => &server.base,
            crate::Source::Fixture => "",
        }
    }

    pub(crate) fn live_status(&self) -> String {
        format!("Live · {}", self.server_base().trim_start_matches("http://"))
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

/// What `ty` posts, if anything: the kind, its title, the session a click opens and the body.
fn decide(app: &App, ty: &str, data: &Value, sid: &str) -> Option<(Kind, &'static str, String, String)> {
    let is_root = |id: &str| app.session(id).is_none_or(|s| s.parent.is_none());
    match ty {
        "session.execution.succeeded" if app.settings.notify.agent => {
            if app.running_child(sid) || !is_root(sid) {
                return None;
            }
            let title = app.tab_title(sid)?;
            Some((Kind::Agent, Kind::Agent.title(), sid.to_owned(), if title.is_empty() { sid.to_owned() } else { title }))
        }
        "session.execution.failed" if app.settings.notify.errors => {
            if !is_root(sid) {
                return None;
            }
            let title = app.tab_title(sid)?;
            Some((Kind::Error, Kind::Error.title(), sid.to_owned(), if title.is_empty() { "An error occurred".to_owned() } else { title }))
        }
        "permission.asked" | "form.created" => {
            let (kind, title, asker) = request(ty, data, sid)?;
            if !app.settings.notify.get(kind) || (ty == "permission.asked" && app.auto_accepts(&asker)) {
                return None;
            }
            // The asking session's own tab, else its root's: a subagent's request waits there.
            let target = if app.tab_title(&asker).is_some() { asker.clone() } else { app.root_of(&asker).to_owned() };
            app.tab_title(&target)?;
            let body = app.session(&asker).map(|s| s.title.clone()).filter(|t| !t.is_empty()).unwrap_or(asker);
            Some((kind, title, target, body))
        }
        _ => None,
    }
}

/// A request's kind, title and asking session (Desktop's `handleRequest`): a permission, or a
/// form that is a question, an MCP server's elicitation or a web search provider's permission.
fn request(ty: &str, data: &Value, sid: &str) -> Option<(Kind, &'static str, String)> {
    let form = if data["form"].is_object() { &data["form"] } else { data };
    let session = form["sessionID"].as_str().or(data["sessionID"].as_str()).unwrap_or(sid).to_owned();
    if ty == "permission.asked" {
        return Some((Kind::Permission, Kind::Permission.title(), session));
    }
    match form["metadata"]["kind"].as_str()? {
        "question" => Some((Kind::Question, Kind::Question.title(), session)),
        "mcp-elicitation" => Some((Kind::Question, "Input requested", session)),
        "websearch.provider" => Some((Kind::Permission, Kind::Permission.title(), session)),
        _ => None,
    }
}

/// Posts the notification `ty` calls for, if any (from `App::apply_event`).
pub fn event(app: &mut App, ty: &str, data: &Value, sid: &str) {
    if app.notify.sink.is_none() {
        return;
    }
    if matches!(ty, "permission.asked" | "form.created") {
        let id = if data["form"].is_object() { &data["form"]["id"] } else { &data["id"] };
        let id = id.as_str().unwrap_or("").to_owned();
        if id.is_empty() || app.notify.asked.contains(&id) {
            return;
        }
        if app.notify.asked.len() >= ASKED {
            app.notify.asked.pop_front();
        }
        app.notify.asked.push_back(id);
    }
    let Some((kind, title, session, body)) = decide(app, ty, data, sid) else { return };
    let server = app.server_base().to_owned();
    app.notify.post(kind, title, &session, body, &server);
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
            rune_notify::bring_to_front();
        }
        app.close_settings();
        app.open(&session, tx);
        opened = true;
    }
    opened
}

fn install(app: &mut App) {
    if !rune_notify::available() {
        return;
    }
    let clicks = app.notify.clicks.clone();
    let waker = app.waker.clone();
    rune_notify::on_click(move |c| {
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
        rune_notify::app_focused()
    }

    fn post(&mut self, note: &Note, server: &str) {
        let n = rune_notify::Notification::new(&note.id, note.title)
            .body(&note.body)
            .info("session", &note.session)
            .info("server", server);
        rune_notify::post(&n);
    }

    fn remove(&mut self, id: &str) {
        rune_notify::remove(&[id]);
    }
}

/// Gives a windowed run the app's identity before its window opens. On Windows that is the
/// `AppUserModelID` its toasts carry and its taskbar button groups under, named as the window
/// is, with the app icon (kept in the data folder for the notification center to read). macOS
/// takes it from the bundle.
#[cfg(target_os = "windows")]
pub fn identify() {
    if crate::headless() {
        return;
    }
    let id = option_env!("OPENCODE_RUNE_BUNDLE_ID")
        .filter(|id| !id.is_empty())
        .unwrap_or(if crate::updater::build_channel() == "local" { "anomaly.opencode-rune.dev" } else { "anomaly.opencode-rune" });
    let mut identity = rune_notify::Identity::new(id, crate::launch::window_title());
    if let Some(icon) = crate::dirs::data().map(|d| d.join("notification-icon.png")) {
        let png = crate::launch::WINDOWS_ICON;
        if std::fs::metadata(&icon).map(|m| m.len()).ok() != Some(png.len() as u64) {
            let _ = std::fs::create_dir_all(icon.parent().unwrap_or(&icon));
            let _ = std::fs::write(&icon, png);
        }
        identity = identity.icon(icon);
    }
    if let Err(e) = rune_notify::register(&identity) {
        eprintln!("notifications: {e}");
    }
}

#[cfg(not(target_os = "windows"))]
pub fn identify() {}

#[cfg(test)]
#[path = "notifications_tests.rs"]
mod tests;

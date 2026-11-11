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
            Kind::Agent => opencode_i18n::t!("notification.session.responseReady.title"),
            Kind::Permission => opencode_i18n::t!("notification.permission.title"),
            Kind::Question => opencode_i18n::t!("notification.question.title"),
            Kind::Error => opencode_i18n::t!("notification.session.error.title"),
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
    /// Sessions whose notification was clicked, from the click's thread, with their server's key.
    clicks: Arc<Mutex<Vec<(String, String)>>>,
}

/// What every window and every server's events share, as Desktop's one `NotificationCoordinator`
/// for all its servers (`shell/notifications/coordinator.ts`, `runtime/server/runtime.tsx:55`):
/// the requests already posted, claimed as `<server>\0<request id>`, and the notifications
/// posted this run, oldest first, at most [`CAP`] in the system's list.
#[derive(Default)]
pub struct Coordinator {
    shown: VecDeque<String>,
    asked: VecDeque<String>,
    posted: u64,
}

thread_local! {
    static COORDINATOR: std::cell::RefCell<Coordinator> = std::cell::RefCell::default();
}

impl Coordinator {
    /// Claims request `id` of `server` for one notification; false when some window already did.
    fn claim(&mut self, server: &str, id: &str) -> bool {
        let key = format!("{server}\0{id}");
        if self.asked.contains(&key) {
            return false;
        }
        if self.asked.len() >= ASKED {
            self.asked.pop_front();
        }
        self.asked.push_back(key);
        true
    }
}

impl Notifier {

    /// A click on a notification for `session` on `server`, from any thread.
    fn click(clicks: &Mutex<Vec<(String, String)>>, session: String, server: String) {
        clicks.lock().unwrap_or_else(|e| e.into_inner()).push((session, server));
    }

    fn post(&mut self, kind: Kind, title: &'static str, session: &str, body: String, server: &str) {
        let Some(sink) = self.sink.as_mut() else { return };
        if sink.focused() {
            return;
        }
        COORDINATOR.with_borrow_mut(|c| {
            if c.shown.len() >= CAP
                && let Some(oldest) = c.shown.pop_front()
            {
                sink.remove(&oldest);
            }
            c.posted += 1;
            let note = Note { id: format!("opencode-rune-{}-{}", std::process::id(), c.posted), kind, title, body, session: session.to_owned() };
            sink.post(&note, server);
            c.shown.push_back(note.id);
        });
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

    /// A session with a tab open on the server the window is checked out on (the event's), as
    /// Desktop asks of every notification (`sessionIDHasOpenTab`): its title, or None.
    fn tab_title(&self, id: &str) -> Option<String> {
        let server = crate::servers::runtime::active(self);
        let tab = self.tabs.iter().find(|t| t.id == id && t.server == *server)?;
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
            Some((Kind::Error, Kind::Error.title(), sid.to_owned(), if title.is_empty() { opencode_i18n::t!("notification.session.error.fallbackDescription").to_owned() } else { title }))
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
        "mcp-elicitation" => Some((Kind::Question, opencode_i18n::t!("notification.elicitation.title"), session)),
        "websearch.provider" => Some((Kind::Permission, Kind::Permission.title(), session)),
        _ => None,
    }
}

/// Posts the notification event `e` calls for, if any (from `App::react`): each window with
/// the session's tab decides, and the first to decide posts it, once for the event.
pub fn event(app: &mut App, e: &crate::events::StreamEvent, sid: &str) {
    let (ty, data) = (e.ty.as_str(), &e.data);
    if app.notify.sink.is_none() {
        return;
    }
    if matches!(ty, "permission.asked" | "form.created") {
        let id = if data["form"].is_object() { &data["form"]["id"] } else { &data["id"] };
        let id = id.as_str().unwrap_or("");
        let server = crate::servers::runtime::active(app).as_str().to_owned();
        if id.is_empty() || !COORDINATOR.with_borrow_mut(|c| c.claim(&server, id)) {
            return;
        }
    }
    let decided = decide(app, ty, data, sid);
    if !crate::events::StreamEvent::take(&e.notice, decided.is_some()) {
        return;
    }
    let Some((kind, title, session, body)) = decided else { return };
    // The click opens the session on its own server (`openNotificationSession`).
    let server = crate::servers::runtime::active(app).as_str().to_owned();
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
    let mut opened = false;
    for (session, from) in clicks {
        if session.is_empty() {
            continue;
        }
        if app.live() {
            rune_notify::bring_to_front();
        }
        app.close_settings();
        crate::servers::runtime::switch(app, &click_server(&from));
        app.open(&session, tx);
        opened = true;
    }
    opened
}

/// The server a click's notification came from: its key, or, from a build that named servers by
/// address, the app's own server's address.
fn click_server(from: &str) -> opencode_kit::servers::ServerKey {
    let own = crate::servers::runtime::own_server().map(|s| s.base);
    if from.is_empty() || own.as_deref() == Some(from) {
        return opencode_kit::servers::ServerKey::sidecar();
    }
    opencode_kit::servers::ServerKey::new(from)
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

/// A notification's link, opened with the app (`deep_links`): its session opens as a click's
/// does. On Windows every click comes this way, also the one that starts the app after it quit.
pub fn clicked(app: &mut App, click: &rune_notify::Click) {
    let (session, server) = (click.get("session").unwrap_or("").to_owned(), click.get("server").unwrap_or("").to_owned());
    Notifier::click(&app.notify.clicks, session, server);
}

/// Gives a windowed run the app's identity before its window opens. On Windows that is the
/// `AppUserModelID` its toasts carry and its taskbar button groups under (the Start menu
/// shortcut carries it too), named as the window is, with the app icon (kept in the data folder
/// for the notification center to read), and the app's URL scheme, which a click on a toast
/// opens, so a toast left in the notification center starts the app again. macOS takes it from
/// the bundle.
///
/// Only the in-process part runs here (`rune_notify::adopt`: the `AppUserModelID`, which the
/// taskbar button needs before the window shows); the registry key and the icon file the
/// notification center reads are written after the first frame, off the UI thread (`register`).
#[cfg(target_os = "windows")]
pub fn identify() {
    if crate::headless() {
        return;
    }
    if let Err(e) = rune_notify::adopt(&identity(None)) {
        eprintln!("notifications: {e}");
    }
}

#[cfg(target_os = "windows")]
fn identity(icon: Option<std::path::PathBuf>) -> rune_notify::Identity {
    let product = crate::launch::product();
    let identity = rune_notify::Identity::new(product.id(), product.name()).activation(product.scheme());
    match icon {
        Some(icon) => identity.icon(icon),
        None => identity,
    }
}

/// After the first frame, once per process, on a thread of its own: the toasts' registration
/// (`AppUserModelId` key, activation scheme) and the icon file it names, written only when missing
/// or changed. A toast posted before it lands still carries the process's identity.
#[cfg(target_os = "windows")]
pub fn register() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if crate::headless() {
        return;
    }
    ONCE.call_once(|| {
        let _ = std::thread::Builder::new().name("opencode-identity".into()).spawn(|| {
            let icon = crate::dirs::data().map(|d| d.join("notification-icon.png"));
            if let Some(icon) = &icon {
                let png = crate::launch::WINDOWS_ICON;
                if std::fs::metadata(icon).map(|m| m.len()).ok() != Some(png.len() as u64) {
                    let _ = std::fs::create_dir_all(icon.parent().unwrap_or(icon));
                    let _ = std::fs::write(icon, png);
                }
            }
            if let Err(e) = rune_notify::register(&identity(icon)) {
                eprintln!("notifications: {e}");
            }
        });
    });
}

#[cfg(not(target_os = "windows"))]
pub fn identify() {}

#[cfg(not(target_os = "windows"))]
pub fn register() {}


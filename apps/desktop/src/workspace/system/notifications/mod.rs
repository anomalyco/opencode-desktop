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

pub mod actions;

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
    /// The same for the same request (`req-<id>`) or the same session's run (`run-<id>`), so a
    /// repeat replaces it and an answer anywhere takes it out (`answered`).
    pub id: String,
    pub kind: Kind,
    /// Desktop's title for what asked: usually `kind`'s, "Input requested" for an MCP server's form.
    pub title: &'static str,
    pub body: String,
    pub session: String,
    /// What its buttons answer (`actions`): a permission's decisions or a question's answers.
    pub ask: Option<actions::Ask>,
    /// The window that posted it, which its buttons' answers go to.
    pub window: String,
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
}

/// Clicks from any thread, for whichever window they're for (`actions::take`).
static CLICKS: Mutex<Vec<rune_notify::Click>> = Mutex::new(Vec::new());
/// Every window's waker, so a click wakes the one it's for.
static WAKERS: Mutex<Vec<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(Vec::new());

/// What every window and every server's events share, as Desktop's one `NotificationCoordinator`
/// for all its servers (`shell/notifications/coordinator.ts`, `runtime/server/runtime.tsx:55`):
/// the requests already posted, claimed as `<server>\0<request id>`, and the notifications
/// posted this run, oldest first, at most [`CAP`] in the system's list.
#[derive(Default)]
pub struct Coordinator {
    /// Posted ids, oldest first, with the server they're from.
    shown: VecDeque<(String, String)>,
    asked: VecDeque<String>,
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
    /// Whether this window posts to the system's notifications.
    pub fn shows(&self) -> bool {
        self.sink.is_some()
    }

    /// A click on a notification, from any thread: it waits for its window's poll.
    fn click(click: rune_notify::Click) {
        CLICKS.lock().unwrap_or_else(|e| e.into_inner()).push(click);
        for wake in WAKERS.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            wake();
        }
    }

    fn post(&mut self, note: Note, server: &str) {
        let Some(sink) = self.sink.as_mut() else { return };
        if sink.focused() {
            return;
        }
        COORDINATOR.with_borrow_mut(|c| {
            // The same id replaces its toast in place: it isn't counted twice.
            c.shown.retain(|(id, _)| *id != note.id);
            if c.shown.len() >= CAP
                && let Some((oldest, _)) = c.shown.pop_front()
            {
                sink.remove(&oldest);
            }
            sink.post(&note, server);
            c.shown.push_back((note.id, server.to_owned()));
        });
    }

    /// Takes notification `id` out of the system's list, if this run posted it.
    fn take_out(&mut self, id: &str) {
        let Some(sink) = self.sink.as_mut() else { return };
        COORDINATOR.with_borrow_mut(|c| {
            let before = c.shown.len();
            c.shown.retain(|(shown, _)| shown != id);
            if c.shown.len() != before {
                sink.remove(id);
            }
        });
    }
}

/// Request `id` was answered (here, in another window, from its toast, or the server settled
/// it): its notification goes, as the dock's does.
pub fn answered(app: &mut App, id: &str) {
    app.notify.take_out(&format!("req-{id}"));
}

/// A session's run notification goes once the session has been looked at.
pub fn seen(app: &mut App, session: &str) {
    app.notify.take_out(&format!("run-{session}"));
}

/// The app quits: the requests of its own server end with it, so their notifications go.
pub fn quitting(app: &mut App) {
    let Some(sink) = app.notify.sink.as_mut() else { return };
    let own = opencode_kit::servers::ServerKey::sidecar();
    COORDINATOR.with_borrow_mut(|c| {
        c.shown.retain(|(id, server)| {
            let ends = id.starts_with("req-") && click_server(server) == own;
            if ends {
                sink.remove(id);
            }
            !ends
        });
    });
}

impl App {
    pub(crate) fn server_base(&self) -> &str {
        match &*self.source {
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
    // A muted session, or one under a muted root, posts nothing (`notification.tsx`'s `muted`).
    let silenced = |id: &str| crate::session_menu::preferences::silenced(app, crate::servers::runtime::active(app), id);
    if silenced(sid) {
        return None;
    }
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
            if silenced(&asker) || !app.settings.notify.get(kind) || (ty == "permission.asked" && app.auto_accepts(&asker)) {
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
    // A request's toast says what it asks and carries its answers; a run's is per session.
    let (id, body, ask) = match actions::ask(ty, data) {
        Some(ask) => (format!("req-{}", ask.request()), actions::body(&body, &ask), Some(ask)),
        None => (format!("run-{session}"), body, None),
    };
    // The click opens the session on its own server (`openNotificationSession`).
    let server = crate::servers::runtime::active(app).as_str().to_owned();
    let note = Note { id, kind, title, body, session, ask, window: app.window.id.clone() };
    app.notify.post(note, &server);
}

/// Opens clicked notifications' sessions, and connects the system the first time a live window
/// polls (from `poll`).
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    if !app.notify.tried && app.persists() {
        app.notify.tried = true;
        install(app);
    }
    let mut opened = false;
    for mut click in actions::take(app) {
        // A button answers where the app is, without bringing it forward.
        match actions::act(app, &mut click) {
            actions::Outcome::Answered => {
                opened = true;
                continue;
            }
            actions::Outcome::Wait => {
                CLICKS.lock().unwrap_or_else(|e| e.into_inner()).push(click);
                continue;
            }
            actions::Outcome::Open => {}
        }
        let (session, from) = (click.get("session").unwrap_or(""), click.get("server").unwrap_or(""));
        if session.is_empty() {
            continue;
        }
        if app.live() {
            rune_notify::bring_to_front();
        }
        app.close_settings();
        crate::servers::runtime::switch(app, &click_server(from));
        app.open(session, tx);
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
    if let Some(waker) = (*app.waker).clone() {
        WAKERS.lock().unwrap_or_else(|e| e.into_inner()).push(waker);
    }
    rune_notify::on_click(Notifier::click);
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
            .info("server", server)
            .info("window", &note.window);
        rune_notify::post(&note.ask.as_ref().map_or(n.clone(), |ask| actions::buttons(n, ask)));
    }

    fn remove(&mut self, id: &str) {
        rune_notify::remove(&[id]);
    }
}

/// A notification's link, opened with the app (`deep_links`): its session opens as a click's
/// does. On Windows every click comes this way, also the one that starts the app after it quit.
pub fn clicked(_: &mut App, click: &rune_notify::Click) {
    Notifier::click(click.clone());
}

/// Gives a windowed run the app's identity before its window opens, writing nothing: on Windows
/// the `AppUserModelID` its taskbar button groups under (the Start menu shortcut carries it too)
/// and its toasts carry. The setup registered the toasts' name and icon and the app's URL scheme
/// for the install (`win_install::register`); `settle_identity` finds out after the first frame
/// whether this is that install. macOS takes all of it from the bundle.
#[cfg(target_os = "windows")]
pub fn identify() {
    if crate::headless() {
        return;
    }
    let product = crate::launch::product();
    if let Err(e) = rune_notify::adopt(&rune_notify::Identity::new(product.id(), product.name())) {
        eprintln!("notifications: {e}");
    }
}

#[cfg(not(target_os = "windows"))]
pub fn identify() {}

/// After the first frame, off the UI thread: the registered install's toasts open the app's
/// scheme, so a toast left in the notification center starts it again; another copy (a zip, a
/// build folder, a harness run) reads only, and its clicks reach it while it runs. A launch
/// writes no registration. Then the toast activator is served (`rune_notify::serve_activations`).
#[cfg(target_os = "windows")]
pub fn settle_identity() {
    static SETTLED: std::sync::Once = std::sync::Once::new();
    if crate::headless() {
        return;
    }
    SETTLED.call_once(settle_once);
}

#[cfg(target_os = "windows")]
fn settle_once() {
    let spawned = std::thread::Builder::new().name("opencode-identity".into()).spawn(|| {
        let started = rune::time::Instant::now();
        let product = crate::launch::product();
        let Some(root) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(std::path::Path::to_path_buf)) else { return };
        let found = win_install::settle_identity(&product, &root);
        if found == win_install::Identity::Installed
            && let Err(e) = rune_notify::adopt(&rune_notify::Identity::new(product.id(), product.name()).activation(product.scheme()))
        {
            eprintln!("notifications: {e}");
        }
        // A toast's button reaches this process straight away where the install registered the
        // activator; elsewhere the registration is inert and buttons act through the toast's event.
        if let Err(e) = rune_notify::serve_activations(&product.toast_activator()) {
            eprintln!("notifications: toast activator: {e}");
        }
        eprintln!("notifications: {} ({found:?}), settled in {} µs off the UI thread", product.id(), started.elapsed().as_micros());
    });
    if let Err(e) = spawned {
        eprintln!("notifications: {e}");
    }
}

#[cfg(not(target_os = "windows"))]
pub fn settle_identity() {}

/// A launch COM made for a toast's button after the app quit (`LocalServer32 ... --toast-activated`,
/// which the install registers with the activator; COM adds `-Embedding`): it takes the
/// activation (a few ms after the class is up) and starts as the app does, its first window
/// answering the click. The app's own server ended with it, so its requests did too; a request
/// the window no longer finds opens its session instead.
pub fn take_activation(args: &[String]) {
    if !args.iter().any(|a| a == "--toast-activated" || a.eq_ignore_ascii_case("-Embedding")) {
        return;
    }
    let product = crate::launch::product();
    if let Some(click) = rune_notify::wait_activation(&product.toast_activator(), std::time::Duration::from_secs(5)) {
        Notifier::click(click);
    }
}


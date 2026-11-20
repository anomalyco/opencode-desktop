//! Agent widgets in the session timeline (`specs/v2/widgets.md`): this window runs every widget
//! message it shows. Per widget it keeps the lifecycle machine (`opencode_widget::Widget`) and
//! the view's local state (`opencode_widget_view::Local`); per template version, its compiled
//! program, fetched once by `name@version` and kept (a version never changes).
//!
//! The host fetches programs and posts responses off the UI thread ([`WidgetsHost`]); steps
//! run here, synchronously, as the contract allows for v1 (well under a millisecond each, under
//! a 250 ms budget). Drawing goes through `session_view::widget::frame` ([`view`]).

#![forbid(unsafe_code)]

mod engine;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use opencode_kit::model::{Widget as Message, WidgetStatus};
use opencode_widget::{Closed, Effect, Event, Face, Phase, Status, Widget, WidgetEvent};
use opencode_widget_view::{Hooks, Local};
use rune::{El, Tx};
use session_view::TimelineHost;
use session_view::palette::Pal;
use serde_json::Value;

pub use engine::{Program, compile};
pub use opencode_widget::{RespondError, fixtures};

/// A template version's program, as far as this window has it.
pub enum Template {
    Fetching,
    Ready(Arc<Program>),
    Failed(String),
}

/// One widget message this window runs.
pub struct Entry {
    pub session: String,
    /// Its template's `name@version`.
    pub template: String,
    pub machine: Widget,
    pub local: Local,
    /// The message's status as last read, so a change reaches the machine once.
    seen: WidgetStatus,
}

/// The window's widgets, in `opencode_kernel::StateSlots`.
#[derive(Default)]
pub struct Widgets {
    templates: HashMap<String, Template>,
    entries: HashMap<String, Entry>,
    /// The transcript version each session's widgets were last read at.
    read: HashMap<String, u64>,
    /// What a lens gets for a widget that went away under a queued handler.
    spare: Local,
}

impl Widgets {
    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries.get(id)
    }

    /// Widget `id`'s view-local state (a spare one if it went away).
    pub fn local_mut(&mut self, id: &str) -> &mut Local {
        match self.entries.get_mut(id) {
            Some(e) => &mut e.local,
            None => &mut self.spare,
        }
    }

    /// Whether any widget has typing waiting to be committed.
    fn pending(&self) -> bool {
        self.entries.values().any(|e| e.local.has_pending())
    }
}

/// Object-safe host boundary, implemented by the desktop adapter (`apps/desktop/src/session/widgets.rs`).
pub trait WidgetsHost {
    fn widgets(&self) -> &Widgets;
    fn widgets_mut(&mut self) -> &mut Widgets;
    /// The session the timeline shows, and its transcript's version.
    fn widget_session(&self) -> Option<(String, u64)>;
    /// The widget messages of `session`'s transcript.
    fn widget_messages(&self, session: &str) -> Vec<Message>;
    /// Fetches and compiles `name@version`'s program off the UI thread; answers with [`loaded`].
    fn fetch_program(&mut self, name: &str, version: &str);
    /// Posts `body` as widget `message`'s response off the UI thread; answers with [`responded`].
    fn post_response(&mut self, session: &str, message: &str, body: Value, attempt: u32);
    /// Posts `body` (`{ value, wake }`) as a message from open widget `message` off the UI
    /// thread; answers with [`sent`].
    fn post_message(&mut self, session: &str, message: &str, body: Value);
}

fn status(m: &WidgetStatus) -> Status {
    match m {
        WidgetStatus::Open => Status::Open,
        WidgetStatus::Submitted(v) => Status::Closed(Closed::Submitted(v.clone())),
        WidgetStatus::Dismissed => Status::Closed(Closed::Dismissed),
    }
}

/// The view a machine shows now, for the local state to catch up with.
fn shown(w: &Widget) -> Option<&opencode_widget::Node> {
    match &w.phase {
        Phase::Ready { view, .. } | Phase::Stepping { view, .. } | Phase::Responding { view, .. } => Some(view),
        Phase::Closed { view, .. } => view.as_ref(),
        Phase::Loading { .. } | Phase::Starting { .. } | Phase::Failed { .. } => None,
    }
}

/// Steps widget `id` by `event` and runs what it asks for, steps included, until it rests.
pub fn advance(host: &mut dyn WidgetsHost, id: &str, event: Event) {
    let mut queue = vec![event];
    while let Some(event) = queue.pop() {
        let Some(e) = host.widgets_mut().entries.get_mut(id) else { return };
        let effects = opencode_widget::machine::step(&mut e.machine, event);
        if let Some(view) = shown(&e.machine) {
            e.local.sync(view);
        }
        for effect in effects {
            if let Some(next) = run(host, id, effect) {
                queue.push(next);
            }
        }
    }
}

/// Runs one effect; the event it answers with at once, if any (a fetch or post answers later).
fn run(host: &mut dyn WidgetsHost, id: &str, effect: Effect) -> Option<Event> {
    let (session, key) = {
        let e = host.widgets().entries.get(id)?;
        (e.session.clone(), e.template.clone())
    };
    match effect {
        Effect::Fetch => match host.widgets().templates.get(&key) {
            Some(Template::Ready(_)) => Some(Event::Loaded),
            Some(Template::Failed(m)) => Some(Event::LoadFailed(m.clone())),
            Some(Template::Fetching) => None,
            None => {
                host.widgets_mut().templates.insert(key.clone(), Template::Fetching);
                let (name, version) = key.split_once('@').unwrap_or((&key, ""));
                let (name, version) = (name.to_owned(), version.to_owned());
                host.fetch_program(&name, &version);
                None
            }
        },
        Effect::Run(input) => {
            let result = match host.widgets().templates.get(&key) {
                Some(Template::Ready(p)) => engine::step(p, &input),
                _ => Err("the widget's program isn't loaded".to_owned()),
            };
            Some(Event::Ran(result))
        }
        Effect::Respond { response, attempt } => {
            host.post_response(&session, id, response.body(), attempt);
            None
        }
        Effect::Send(message) => {
            host.post_message(&session, id, message.body());
            None
        }
    }
}

/// `name@version`'s program came back (compiled), or why it couldn't.
pub fn loaded(host: &mut dyn WidgetsHost, name: &str, version: &str, result: Result<Arc<Program>, String>) {
    let key = format!("{name}@{version}");
    let event = match &result {
        Ok(_) => Event::Loaded,
        Err(m) => Event::LoadFailed(m.clone()),
    };
    let slot = match result {
        Ok(p) => Template::Ready(p),
        Err(m) => Template::Failed(m),
    };
    host.widgets_mut().templates.insert(key.clone(), slot);
    let waiting: Vec<String> = host
        .widgets()
        .entries
        .iter()
        .filter(|(_, e)| e.template == key && matches!(e.machine.phase, Phase::Loading { .. }))
        .map(|(id, _)| id.clone())
        .collect();
    for id in waiting {
        advance(host, &id, event.clone());
    }
}

/// Seeds `name@version` with a program compiled here (stories, fixtures).
pub fn preload(host: &mut dyn WidgetsHost, name: &str, version: &str, program: &str) {
    let result = compile(program).map(Arc::new);
    loaded(host, name, version, result);
}

/// Widget `id`'s response for `attempt` came back: `Ok`, a conflict (409), or a failure.
pub fn responded(host: &mut dyn WidgetsHost, id: &str, attempt: u32, result: Result<(), RespondError>) {
    advance(host, id, Event::Responded { attempt, result });
}

/// A message widget `id` sent came back: taken, or a conflict (409: no longer open), or a failure.
pub fn sent(host: &mut dyn WidgetsHost, id: &str, result: Result<(), RespondError>) {
    advance(host, id, Event::Sent(result));
}

/// What the user did in widget `id`'s view.
pub fn input(host: &mut dyn WidgetsHost, id: &str, event: WidgetEvent) {
    advance(host, id, Event::Input(event));
}

/// Try again on a failed widget: its template is fetched afresh.
pub fn retry(host: &mut dyn WidgetsHost, id: &str) {
    let Some(key) = host.widgets().entries.get(id).map(|e| e.template.clone()) else { return };
    if matches!(host.widgets().templates.get(&key), Some(Template::Failed(_))) {
        host.widgets_mut().templates.remove(&key);
    }
    advance(host, id, Event::Retry);
}

/// Opens a machine for every widget message of the shown session it doesn't run yet, and tells
/// the running ones what changed on the server. Reads the transcript only when it moved.
pub fn sync(host: &mut dyn WidgetsHost) -> bool {
    let Some((session, version)) = host.widget_session() else { return false };
    if host.widgets().read.get(&session) == Some(&version) {
        return false;
    }
    host.widgets_mut().read.insert(session.clone(), version);
    let mut changed = false;
    for m in host.widget_messages(&session) {
        match host.widgets().entries.get(&m.id).map(|e| e.seen == m.status) {
            Some(true) => {}
            Some(false) => {
                if let Some(e) = host.widgets_mut().entries.get_mut(&m.id) {
                    e.seen = m.status.clone();
                }
                advance(host, &m.id, Event::Status(status(&m.status)));
                changed = true;
            }
            None => {
                let (machine, effects) = Widget::open(m.data.clone(), status(&m.status));
                let entry = Entry { session: session.clone(), template: m.template_key(), machine, local: Local::default(), seen: m.status.clone() };
                host.widgets_mut().entries.insert(m.id.clone(), entry);
                for effect in effects {
                    if let Some(next) = run(host, &m.id, effect) {
                        advance(host, &m.id, next);
                    }
                }
                changed = true;
            }
        }
    }
    changed
}

/// Commits typing whose debounce ran out or whose field lost focus, and asks to wake for the
/// next debounce.
pub fn poll(host: &mut dyn WidgetsHost, now: f64, tx: &mut Tx) -> bool {
    if !host.widgets().pending() {
        return false;
    }
    let ids: Vec<String> = host.widgets().entries.iter().filter(|(_, e)| e.local.has_pending()).map(|(id, _)| id.clone()).collect();
    let mut changed = false;
    for id in ids {
        let events = host.widgets_mut().local_mut(&id).poll(now);
        for ev in events {
            input(host, &id, ev);
            changed = true;
        }
        if let Some(due) = host.widgets().entries.get(&id).and_then(|e| e.local.next_due()) {
            tx.wake_at(due);
        }
    }
    changed
}

/// Lets go of the widgets of sessions `keep` doesn't name (a window at rest).
pub fn rest(host: &mut dyn WidgetsHost, keep: &dyn Fn(&str) -> bool) {
    let w = host.widgets_mut();
    w.entries.retain(|_, e| keep(&e.session));
    w.read.retain(|s, _| keep(s));
}

/// Widget message `m` of session `session` as the timeline draws it.
pub fn view<H: WidgetsHost + TimelineHost>(s: &H, p: &Pal, session: &str, m: &Message) -> El {
    let Some(e) = s.widgets().entries.get(&m.id) else {
        // Not opened yet (the next frame's sync opens it): a closed one already says how it
        // ended, an open one shows its placeholder.
        let how = session_view::widget::closed(m);
        let face = match &how {
            Some(how) => Face::Closed { view: None, how },
            None => Face::Loading,
        };
        return session_view::widget::frame(s, p, session, m, face, &Local::default(), None, None);
    };
    let (lens, emit, again) = (m.id.clone(), m.id.clone(), m.id.clone());
    let hooks: Hooks<H> = Hooks {
        local: Rc::new(move |s: &mut H| s.widgets_mut().local_mut(&lens)),
        emit: Rc::new(move |s: &mut H, ev: WidgetEvent, _tx: &mut Tx| input(s, &emit, ev)),
    };
    let retry: opencode_widget_view::Action<H> = Rc::new(move |s: &mut H, _tx: &mut Tx| retry(s, &again));
    session_view::widget::frame(s, p, session, m, e.machine.face(), &e.local, Some(&hooks), Some(retry))
}

//! Agent widgets in the session timeline (`specs/v2/widgets.md`): this window runs every widget
//! the timeline shows. A widget is a `widget` tool call; the widgets plugin keeps its state
//! (RPC `opencode.widget`: `states` when a session opens, then its `changed` event). Per widget
//! it keeps the lifecycle machine (`opencode_widget::Widget`) and the view's local state
//! (`opencode_widget_view::Local`); per template version, its compiled program, fetched once by
//! `name@version` and kept (a version never changes).
//!
//! The host fetches states and programs and posts answers off the UI thread ([`WidgetsHost`]);
//! steps run here, synchronously, as the contract allows for v1 (well under a millisecond each,
//! under a 250 ms budget). Drawing goes through `session_view::widget::frame` ([`view`]).

#![forbid(unsafe_code)]

mod engine;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use opencode_kit::model::{WidgetCall, WidgetState, WidgetStatus, widget_key};
use opencode_widget::{Closed, Effect, Event, Face, Phase, Status, Widget, WidgetEvent};
use opencode_widget_view::{Hooks, Local};
use rune::{El, Tx};
use session_view::TimelineHost;
use session_view::palette::Pal;
use serde_json::Value;

pub use engine::{Program, compile};
pub use opencode_widget::{RespondError, fixtures};
/// The shader slots' GPU side, for the host's compositor.
pub use opencode_widget_view::shader;

/// A template version's program, as far as this window has it.
pub enum Template {
    Fetching,
    Ready(Arc<Program>),
    Failed(String),
}

/// One widget this window runs.
pub struct Entry {
    pub session: String,
    /// Its tool call: the assistant message and the call id.
    pub message: String,
    pub call: String,
    /// Its template's `name@version`.
    pub template: String,
    /// Its title, else its template's name.
    pub label: String,
    pub machine: Widget,
    pub local: Local,
    /// The status as last read, so a change reaches the machine once.
    seen: WidgetStatus,
}

/// A session's widget states, as the plugin keeps them.
#[derive(Default)]
pub enum States {
    /// `states` is on its way.
    #[default]
    Fetching,
    /// By widget key ([`widget_key`]).
    Ready(HashMap<String, WidgetState>),
    /// The plugin isn't there (the `widgets` flag is off) or didn't answer: calls show as calls.
    Failed,
}

#[derive(Default)]
struct Session {
    states: States,
    /// Bumped on every change, so [`sync`] reads again.
    version: u64,
}

/// The window's widgets, in `opencode_kernel::StateSlots`.
#[derive(Default)]
pub struct Widgets {
    templates: HashMap<String, Template>,
    /// By widget key ([`widget_key`]).
    entries: HashMap<String, Entry>,
    sessions: HashMap<String, Session>,
    /// The transcript and states versions each session's widgets were last read at.
    read: HashMap<String, (u64, u64)>,
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

    /// Session `session`'s widget states, once read.
    pub fn states(&self, session: &str) -> Option<&States> {
        self.sessions.get(session).map(|s| &s.states)
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
    /// The `widget` tool calls of `session`'s transcript.
    fn widget_calls(&self, session: &str) -> Vec<WidgetCall>;
    /// Reads `session`'s widget states (`states`) off the UI thread; answers with [`states`].
    fn fetch_states(&mut self, session: &str);
    /// Fetches and compiles `name@version`'s program (`program`, in `session`'s location) off the
    /// UI thread; answers with [`loaded`].
    fn fetch_program(&mut self, session: &str, name: &str, version: &str);
    /// Posts `response` (`{ value }` or `{ dismiss: true }`) as widget `call` of `message`'s
    /// answer (`respond`) off the UI thread; answers with [`responded`].
    fn post_response(&mut self, session: &str, message: &str, call: &str, response: Value, attempt: u32);
    /// Posts `value` as a message from open widget `call` of `message` (`send`) off the UI
    /// thread; answers with [`sent`].
    fn post_message(&mut self, session: &str, message: &str, call: &str, value: Value, wake: bool);
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
    let (session, message, call, key) = {
        let e = host.widgets().entries.get(id)?;
        (e.session.clone(), e.message.clone(), e.call.clone(), e.template.clone())
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
                host.fetch_program(&session, &name, &version);
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
            host.post_response(&session, &message, &call, response.body(), attempt);
            None
        }
        Effect::Send(sent) => {
            host.post_message(&session, &message, &call, sent.value, sent.wake);
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

/// Session `session`'s widget states came back (`states`), or the plugin isn't there. A state a
/// `changed` event brought meanwhile that is already answered wins over an open one read
/// before it: a widget only ever closes.
pub fn states(host: &mut dyn WidgetsHost, session: &str, result: Result<Vec<WidgetState>, String>) {
    let s = host.widgets_mut().sessions.entry(session.to_owned()).or_default();
    s.version += 1;
    let Ok(list) = result else {
        if !matches!(s.states, States::Ready(_)) {
            s.states = States::Failed;
        }
        return;
    };
    let mut next: HashMap<String, WidgetState> = list.into_iter().map(|w| (w.key(), w)).collect();
    if let States::Ready(old) = &s.states {
        for (k, w) in old {
            if w.status != WidgetStatus::Open && next.get(k).is_none_or(|n| n.status == WidgetStatus::Open) {
                next.insert(k.clone(), w.clone());
            }
        }
    }
    s.states = States::Ready(next);
}

/// The plugin's `changed` event (`{ sessionID, state }`): a widget was posted or answered.
/// Whether it touched a session this window reads.
pub fn changed(host: &mut dyn WidgetsHost, data: &Value) -> bool {
    let Some(state) = WidgetState::parse(&data["state"]) else { return false };
    let session = data["sessionID"].as_str().map_or_else(|| state.session.clone(), str::to_owned);
    let Some(s) = host.widgets_mut().sessions.get_mut(&session) else { return false };
    match &mut s.states {
        States::Ready(map) => {
            map.insert(state.key(), state);
        }
        // Read before the plugin answered, or it failed then: this one shows it's there now.
        States::Fetching | States::Failed => s.states = States::Ready(HashMap::from([(state.key(), state)])),
    }
    s.version += 1;
    true
}

/// The connection came back: events published meanwhile were missed, so each session's states
/// are read again when it next shows.
pub fn reconnected(host: &mut dyn WidgetsHost) {
    let w = host.widgets_mut();
    w.sessions.clear();
    w.read.clear();
}

/// Opens a machine for every widget of the shown session it doesn't run yet, and tells the
/// running ones what changed on the server. Reads the session's states once, and its widgets
/// only when its transcript or its states moved.
pub fn sync(host: &mut dyn WidgetsHost) -> bool {
    let Some((session, version)) = host.widget_session() else { return false };
    // Read once when the session first shows; a read that failed (the plugin wasn't up in the
    // session's location yet) again when its transcript moves.
    let moved = host.widgets().read.get(&session).is_none_or(|r| r.0 != version);
    let fetch = match host.widgets().sessions.get(&session) {
        None => true,
        Some(s) => matches!(s.states, States::Failed) && moved,
    };
    if fetch {
        host.widgets_mut().sessions.entry(session.clone()).or_default().states = States::Fetching;
        host.fetch_states(&session);
    }
    let read = (version, host.widgets().sessions.get(&session).map_or(0, |s| s.version));
    if host.widgets().read.get(&session) == Some(&read) {
        return false;
    }
    host.widgets_mut().read.insert(session.clone(), read);
    let mut changed = false;
    for c in host.widget_calls(&session) {
        let key = c.key();
        let Some(States::Ready(map)) = host.widgets().states(&session) else { break };
        let Some(state) = map.get(&key).cloned() else { continue };
        match host.widgets().entries.get(&key).map(|e| e.seen == state.status) {
            Some(true) => {}
            Some(false) => {
                if let Some(e) = host.widgets_mut().entries.get_mut(&key) {
                    e.seen = state.status.clone();
                }
                advance(host, &key, Event::Status(status(&state.status)));
                changed = true;
            }
            None => {
                let (machine, effects) = Widget::open(c.data().clone(), status(&state.status));
                let label = c.input["title"].as_str().filter(|t| !t.is_empty()).unwrap_or(state.label()).to_owned();
                let entry = Entry { session: session.clone(), message: c.message.clone(), call: c.call.clone(), template: state.template_key(), label, machine, local: Local::default(), seen: state.status.clone() };
                host.widgets_mut().entries.insert(key.clone(), entry);
                for effect in effects {
                    if let Some(next) = run(host, &key, effect) {
                        advance(host, &key, next);
                    }
                }
                changed = true;
            }
        }
    }
    changed
}

/// Draws the canvases that are due (shown, sized, and changed or at their next frame), and asks
/// to wake for the next frame of those that animate.
pub fn draw_canvases(host: &mut dyn WidgetsHost, now: f64, tx: &mut Tx) -> bool {
    let mut wake: Option<f64> = None;
    let mut any = false;
    for e in host.widgets().entries.values() {
        let Some(view) = e.machine.view() else { continue };
        any |= e.local.canvases_want(view, now);
        if let Some(at) = e.local.canvas_next(view) {
            wake = Some(wake.map_or(at, |w: f64| w.min(at)));
        }
    }
    if let Some(at) = wake {
        tx.wake_at(at.max(now));
    }
    if !any {
        return false;
    }
    let (scale, dark) = opencode_widget_view::appearance();
    let Widgets { templates, entries, .. } = host.widgets_mut();
    for e in entries.values_mut() {
        let Entry { machine, local, template, .. } = e;
        let (Some(view), Some(state)) = (machine.view(), machine.state()) else { continue };
        let Some(Template::Ready(program)) = templates.get(template.as_str()) else { continue };
        for (path, frame) in local.canvases_due(view, now, f64::from(scale), dark) {
            let result = engine::draw(program, &machine.data, state, &frame);
            local.canvas_drawn(view, &path, now, result);
        }
    }
    true
}

/// Commits typing whose debounce ran out or whose field lost focus, and asks to wake for the
/// next debounce; draws due canvases; and builds widgets again whose shader's status changed.
pub fn poll(host: &mut dyn WidgetsHost, now: f64, tx: &mut Tx) -> bool {
    let mut changed = draw_canvases(host, now, tx);
    if opencode_widget_view::shader::take_changed() {
        // The shader's slot failed or froze: its widget builds again to say so.
        host.widgets_mut();
        changed = true;
    }
    if !host.widgets().pending() {
        return changed;
    }
    let ids: Vec<String> = host.widgets().entries.iter().filter(|(_, e)| e.local.has_pending()).map(|(id, _)| id.clone()).collect();
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
    w.sessions.retain(|s, _| keep(s));
    w.read.retain(|s, _| keep(s));
}

/// Widget call `call` of assistant message `message` in session `session` as the timeline draws
/// it: the widget, a placeholder while its state or program is on the way, or `None` to show the
/// call as any other (the plugin isn't there, or the call failed before it posted a widget).
pub fn view<H: WidgetsHost + TimelineHost>(s: &H, p: &Pal, session: &str, message: &str, call: &session_view::tools::Call) -> Option<El> {
    let key = widget_key(message, call.id);
    let Some(e) = s.widgets().entries.get(&key) else {
        // Not opened yet (the next frame's sync opens it): a closed one already says how it
        // ended; an open one, or one whose state is on the way, shows its placeholder.
        let label = call.input["title"].as_str().filter(|t| !t.is_empty()).or_else(|| call.input["template"].as_str()).unwrap_or("widget");
        let how = match s.widgets().states(session) {
            Some(States::Ready(map)) => match map.get(&key) {
                Some(w) => closed(&w.status),
                None if *call.status == opencode_kit::model::ToolStatus::Running => None,
                None => return None,
            },
            Some(States::Fetching) | None if *call.status != opencode_kit::model::ToolStatus::Error => None,
            _ => return None,
        };
        let face = match &how {
            Some(how) => Face::Closed { view: None, how },
            None => Face::Loading,
        };
        return Some(session_view::widget::frame(s, p, session, &key, label, face, &Local::default(), None, None));
    };
    let (lens, emit, again) = (key.clone(), key.clone(), key.clone());
    let hooks: Hooks<H> = Hooks {
        local: Rc::new(move |s: &mut H| s.widgets_mut().local_mut(&lens)),
        emit: Rc::new(move |s: &mut H, ev: WidgetEvent, _tx: &mut Tx| input(s, &emit, ev)),
    };
    let retry: opencode_widget_view::Action<H> = Rc::new(move |s: &mut H, _tx: &mut Tx| retry(s, &again));
    Some(session_view::widget::frame(s, p, session, &key, &e.label, e.machine.face(), &e.local, Some(&hooks), Some(retry)))
}

/// How a widget ended, from its state alone.
fn closed(status: &WidgetStatus) -> Option<Closed> {
    match status {
        WidgetStatus::Open => None,
        WidgetStatus::Submitted(v) => Some(Closed::Submitted(v.clone())),
        WidgetStatus::Dismissed => Some(Closed::Dismissed),
    }
}

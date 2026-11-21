//! Agent widgets in the session timeline (`specs/v2/widgets.md`): this window runs every widget
//! the timeline shows. A widget is a `widget` tool call; the widgets plugin keeps its state
//! (RPC `opencode.widget`: `states` when a session opens, then its `changed` event). Per widget
//! it keeps the lifecycle machine (`opencode_widget::Widget`) and the view's local state
//! (`opencode_widget_view::Local`); per template version, its compiled program, fetched once by
//! `name@version` and kept (a version never changes).
//!
//! The host fetches states and programs and posts answers off the UI thread ([`WidgetsHost`]);
//! steps run here, synchronously, as the contract allows for v1 (well under a millisecond each,
//! under a 250 ms budget). A live template's program runs on a thread of its own ([`live`]).
//! Drawing goes through `session_view::widget::frame` ([`view`]).

#![forbid(unsafe_code)]

mod engine;
pub mod fullscreen;
pub mod input;
mod live;
mod view;
#[cfg(not(target_arch = "wasm32"))]
mod transport;

use std::collections::HashMap;
use std::sync::Arc;

use opencode_kit::model::{WidgetCall, WidgetState, WidgetStatus};
use opencode_widget::{Closed, Effect, Event, Mode, Phase, Status, Widget, WidgetEvent};
use opencode_widget_view::Local;
use rune::Tx;
use serde_json::Value;

pub use engine::{Compiled, Grants, Kind, LoadError, Program, compile, compile_as, offset};
pub use opencode_widget::{RespondError, fixtures};
pub use view::{screen_key, screen_view, view};
/// The shader slots' GPU side, for the host's compositor.
pub use opencode_widget_view::shader;

/// A template version's program, as far as this window has it.
pub enum Template {
    Fetching,
    Ready(Arc<Compiled>),
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
    /// The machine's `data`, converted once for its program.
    data: engine::Data,
    pub local: Local,
    /// The status as last read, so a change reaches the machine once.
    seen: WidgetStatus,
    /// Its live program, when it runs one.
    run: live::Run,
    /// The errors reported for it (phase and message), each once.
    reported: std::collections::HashSet<String>,
    /// Its live program asked for the pointer (`widget.capturePointer()`).
    capture_wanted: bool,
    /// The user's "Ask agent to fix it" is on its way.
    asking: bool,
}

impl Entry {
    /// The user's "Ask agent to fix it" is on its way.
    pub fn asking(&self) -> bool {
        self.asking
    }

    /// Whether it asks for pointer capture: its live program did, or its view's root says
    /// `capture: "pointer"`.
    pub fn wants_capture(&self) -> bool {
        self.capture_wanted || self.machine.view().is_some_and(opencode_widget::Node::wants_capture)
    }
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
    /// The widget full screen, if any.
    pub screen: fullscreen::Screen,
    /// The widget with the keyboard (and the pointer), if any.
    pub focus: input::Focus,
    /// The window's size in points, as full screen last laid out.
    screen_size: [f32; 2],
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
    /// Records the program error widget `call` of `message` hit here (`report`, `error`: `{ phase,
    /// message, line?, column? }`), off the UI thread. With `ask` (the user's "Ask agent to fix
    /// it"), it goes to the agent too, and the job answers with [`asked`].
    fn post_report(&mut self, session: &str, message: &str, call: &str, error: Value, ask: bool);
    /// Where the OS cursor is on screen, for a widget capturing the pointer; `None` where it
    /// can't be read.
    fn pointer_at(&self) -> Option<[f64; 2]>;
    /// Puts the OS cursor back at `to` (from [`WidgetsHost::pointer_at`]).
    fn warp_pointer(&mut self, to: [f64; 2]);
    /// Whether the window is OS full screen now; `None` without a window (a headless run).
    fn window_full_screen(&self) -> Option<bool>;
    /// Dev mode: a live widget's `fetch` may reach the local network (`allowLocalNetwork`).
    fn widget_dev_mode(&self) -> bool {
        false
    }
    /// Wakes the window from another thread: a live program posted or wants a frame.
    fn widget_notify(&self) -> Arc<dyn Fn() + Send + Sync>;
}

/// The one place a template's declared capabilities become what it may use. For now every
/// declared capability is granted (`widgets-live.md`, Capabilities); a permission prompt would
/// decide here.
pub fn grant(capabilities: &Value) -> Grants {
    Grants::declared(capabilities)
}

fn status(m: &WidgetStatus) -> Status {
    match m {
        WidgetStatus::Open => Status::Open,
        WidgetStatus::Submitted(v) => Status::Closed(Closed::Submitted(v.clone())),
        WidgetStatus::Dismissed => Status::Closed(Closed::Dismissed),
        WidgetStatus::Superseded => Status::Closed(Closed::Superseded),
    }
}

/// Steps widget `id` by `event` and runs what it asks for, steps included, until it rests.
pub fn advance(host: &mut dyn WidgetsHost, id: &str, event: Event) {
    let mut queue = vec![event];
    while let Some(event) = queue.pop() {
        let Some(e) = host.widgets_mut().entries.get_mut(id) else { return };
        let effects = opencode_widget::machine::step(&mut e.machine, event);
        if let Some(view) = e.machine.view() {
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
            Some(Template::Ready(c)) => Some(Event::Loaded(mode(c))),
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
            let w = host.widgets();
            let result = match (w.templates.get(&key), w.entries.get(id)) {
                (Some(Template::Ready(c)), Some(e)) => match &c.kind {
                    Kind::Step(p) => engine::step(p, &e.data, &input).map_err(|f| (f, c.offset)),
                    Kind::Live { .. } => return Some(Event::Ran(Err("a live widget has no steps".to_owned()))),
                },
                _ => return Some(Event::Ran(Err("the widget's program isn't loaded".to_owned()))),
            };
            let result = result.map_err(|(f, offset)| {
                let phase = if f.view { "view" } else if matches!(input, opencode_widget::Step::Init) { "init" } else { "event" };
                if f.program {
                    report(host, id, phase, &f.message, f.location, offset);
                } else {
                    client_error(host, id, phase, &f.message);
                }
                in_source(&f.message, f.location, offset)
            });
            Some(Event::Ran(result))
        }
        Effect::Start => {
            live::start(host, id);
            None
        }
        Effect::Deliver(event) => {
            live::deliver(host, id, &event);
            None
        }
        Effect::Stop => {
            live::stop(host, id);
            None
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

fn mode(c: &Compiled) -> Mode {
    match c.kind {
        Kind::Live { .. } => Mode::Live,
        Kind::Step(_) => Mode::Step,
    }
}

/// `name@version`'s program came back (compiled), or why it couldn't.
pub fn loaded(host: &mut dyn WidgetsHost, name: &str, version: &str, result: Result<Arc<Compiled>, engine::LoadError>) {
    let key = format!("{name}@{version}");
    let event = match &result {
        Ok(c) => Event::Loaded(mode(c)),
        Err(e) => Event::LoadFailed(e.message().to_owned()),
    };
    // Only a program that didn't compile is the template's to fix.
    let program = matches!(result, Err(engine::LoadError::Program(_)));
    let slot = match result {
        Ok(p) => Template::Ready(p),
        Err(e) => Template::Failed(e.message().to_owned()),
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
        if let Event::LoadFailed(m) = &event {
            if program {
                report(host, &id, "load", m, None, 0);
            } else {
                client_error(host, &id, "load", m);
            }
        }
        advance(host, &id, event.clone());
    }
}

/// Seeds `name@version` with a program compiled here (stories, fixtures): `mode` `"step"` or
/// `"live"`, granted the `capabilities` it declares.
pub fn preload(host: &mut dyn WidgetsHost, name: &str, version: &str, program: &str, mode: &str, capabilities: &Value) {
    let lines = fixtures::PRELUDE.matches('\n').count() as u32 + 1;
    let result = compile_as(program, mode, grant(capabilities), lines).map(Arc::new);
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

/// Plugins changed in a location (`plugin.updated`): sessions whose states couldn't be read
/// (the widgets plugin wasn't loaded yet) are read again.
pub fn plugins_changed(host: &mut dyn WidgetsHost) {
    let failed: Vec<String> = host.widgets().sessions.iter().filter(|(_, s)| matches!(s.states, States::Failed)).map(|(id, _)| id.clone()).collect();
    for session in failed {
        if let Some(s) = host.widgets_mut().sessions.get_mut(&session) {
            s.states = States::Fetching;
        }
        host.fetch_states(&session);
    }
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
                let data = engine::Data::new(&machine.data);
                let label = c.input["title"].as_str().filter(|t| !t.is_empty()).unwrap_or(state.label()).to_owned();
                let entry = Entry { session: session.clone(), message: c.message.clone(), call: c.call.clone(), template: state.template_key(), label, machine, data, local: Local::default(), seen: state.status.clone(), run: live::Run::default(), reported: Default::default(), capture_wanted: false, asking: false };
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
    let w = host.widgets();
    // A live program draws its canvases itself.
    let steps = |e: &Entry| matches!(w.templates.get(&e.template), Some(Template::Ready(c)) if matches!(c.kind, Kind::Step(_)));
    for e in w.entries.values().filter(|e| steps(e)) {
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
    let mut failed = Vec::new();
    let Widgets { templates, entries, .. } = host.widgets_mut();
    for (id, e) in entries.iter_mut() {
        let Entry { machine, local, template, data, .. } = e;
        let (Some(view), Some(state)) = (machine.view(), machine.state()) else { continue };
        let Some(Template::Ready(c)) = templates.get(template.as_str()) else { continue };
        let Kind::Step(program) = &c.kind else { continue };
        for (path, frame) in local.canvases_due(view, now, f64::from(scale), dark) {
            let result = engine::draw(program, data, state, &frame).map_err(|f| {
                let message = in_source(&f.message, f.location, c.offset);
                failed.push((id.clone(), f, c.offset));
                message
            });
            local.canvas_drawn(view, &path, now, result);
        }
    }
    for (id, f, offset) in failed {
        if f.program {
            report(host, &id, "draw", &f.message, f.location, offset);
        } else {
            client_error(host, &id, "draw", &f.message);
        }
    }
    true
}

/// An interpreter message as the template's author reads it: its `(line N, col M)`, counted in
/// the program, counted in the template's source instead (less the prelude's `offset` lines).
pub(crate) fn in_source(message: &str, location: Option<(u32, u32)>, offset: u32) -> String {
    let Some((line, column)) = location.filter(|(l, _)| *l > offset) else { return message.to_owned() };
    match message.strip_suffix(&format!(" (line {line}, col {column})")) {
        Some(head) => format!("{head} (line {}, col {column})", line - offset),
        None => message.to_owned(),
    }
}

/// A client error widget `id` hit (`widgets.md`, Failures): this client's bug, never reported;
/// logged in dev mode only. The widget shows it as it would any failure.
pub(crate) fn client_error(host: &mut dyn WidgetsHost, id: &str, phase: &str, message: &str) {
    if host.widget_dev_mode() {
        eprintln!("widget {id}: client error in {phase}: {message}");
    }
}

/// The user's "Ask agent to fix it" on widget `id`'s recorded failure: `report` again with the
/// same error and `ask`, once, while nothing else asks.
pub fn ask(host: &mut dyn WidgetsHost, id: &str) {
    let Some(e) = host.widgets().entries.get(id) else { return };
    if e.asking {
        return;
    }
    let (session, message, call) = (e.session.clone(), e.message.clone(), e.call.clone());
    let Some(failure) = failure_of(host.widgets(), &session, id).filter(|f| !f.sent) else { return };
    if let Some(e) = host.widgets_mut().entries.get_mut(id) {
        e.asking = true;
    }
    host.post_report(&session, &message, &call, failure.error(), true);
}

/// The ask for widget `id` went (`Ok`), or didn't. Its `changed` event brings `sent`.
pub fn asked(host: &mut dyn WidgetsHost, id: &str, result: Result<(), String>) {
    if let Err(m) = &result {
        client_error(host, id, "ask", m);
    }
    if let Some(e) = host.widgets_mut().entries.get_mut(id) {
        e.asking = false;
    }
}

/// Widget `id`'s recorded program error, while it's open (`State.failure`).
pub(crate) fn failure_of(w: &Widgets, session: &str, id: &str) -> Option<opencode_kit::model::WidgetFailure> {
    match w.states(session) {
        Some(States::Ready(map)) => map.get(id).filter(|s| s.status == WidgetStatus::Open).and_then(|s| s.failure.clone()),
        _ => None,
    }
}

/// Records a program error widget `id` hit here (`report`), once per distinct error: `phase` is
/// where (`init`, `event`, `view`, `draw`, `main`, `shader`, `load`), `location` the line and
/// column in the program, less `offset` lines to be in the template's source. The server tells
/// the agent when the widget's session made the template; otherwise the user may ask.
pub(crate) fn report(host: &mut dyn WidgetsHost, id: &str, phase: &str, message: &str, location: Option<(u32, u32)>, offset: u32) {
    let Some(e) = host.widgets_mut().entries.get_mut(id) else { return };
    if !e.machine.open_here() && !matches!(e.machine.phase, Phase::Starting { .. } | Phase::Failed { .. } | Phase::Stopped { .. } | Phase::Loading { .. }) {
        return;
    }
    if !e.reported.insert(format!("{phase}\n{message}")) {
        return;
    }
    // The interpreter's message names the program's line; the report gives the source's.
    let text = match location {
        Some((line, column)) => message.strip_suffix(&format!(" (line {line}, col {column})")).unwrap_or(message),
        None => message,
    };
    let mut error = serde_json::json!({ "phase": phase, "message": text });
    if let Some((line, column)) = location.filter(|(l, _)| *l > offset) {
        error["line"] = (line - offset).into();
        error["column"] = column.into();
    }
    let (session, message, call) = (e.session.clone(), e.message.clone(), e.call.clone());
    host.post_report(&session, &message, &call, error, false);
}

/// Commits typing whose debounce ran out or whose field lost focus, and asks to wake for the
/// next debounce; draws due canvases; and builds widgets again whose shader's status changed.
pub fn poll(host: &mut dyn WidgetsHost, now: f64, tx: &mut Tx) -> bool {
    let mut changed = draw_canvases(host, now, tx);
    changed |= fullscreen::poll(host, now, tx);
    input::poll(host, tx);
    changed |= live::poll(host, now, tx);
    if opencode_widget_view::shader::take_changed() {
        // The shader's slot failed or froze: its widget builds again to say so, and the agent
        // hears of a shader that didn't compile.
        for (scope, message, program) in opencode_widget_view::shader::failures() {
            if program {
                report(host, &scope, "shader", &message, None, 0);
            } else {
                client_error(host, &scope, "shader", &message);
            }
        }
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

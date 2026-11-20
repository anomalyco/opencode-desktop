//! One widget message's lifecycle on the client the user works in, as an explicit machine:
//! the template loads, `init` runs, the view is live, each event steps it, and a submit or
//! dismiss is sent once to the server, after which the widget is read-only.
//!
//! ```text
//! Loading ──Loaded──▶ Starting ──Ran(ok)──▶ Ready ◀──Ran(ok, no answer)── Stepping
//!    │                   │                   │  └────────Input────────────▶ │
//! LoadFailed          Ran(err)               │                               │ Ran(ok, submit/dismiss)
//!    ▼                   ▼                   │                               ▼
//! Failed ◀───────────────┘                   └──Status(closed)──▶ Closed ◀─ Responding
//! ```
//!
//! A live template (`widgets-live.md`) runs one program instead of steps:
//!
//! ```text
//! Loading ──Loaded(live)──▶ Waiting ──Shown──▶ Running ──Answered──▶ Responding ──▶ Closed
//!                                               │  ▲ Rendered, Input (delivered), Sending
//!                                     Ended ────┘  └──── Retry (Restart) ──── Stopped
//! ```
//!
//! A step that fails leaves the previous state and view and shows the error inline (`Ready`
//! with an error); the first `init` failing shows an error card (`Failed`). A widget the server
//! already reports submitted or dismissed runs `init` only to draw its final view read-only
//! (`Closed`); clients don't keep local state. Effects ([`Effect`]) are returned for the caller to
//! run: fetching the template, running a step on its program, posting the response. Their
//! results come back as events.

use serde_json::Value;

use crate::node::Node;
use crate::step::{Response, StepInput, StepOutput, WidgetEvent};

/// How a widget ended, as the server reports it.
#[derive(Clone, Debug, PartialEq)]
pub enum Closed {
    Submitted(Value),
    Dismissed,
}

impl Closed {
    fn from_response(r: &Response) -> Closed {
        match r {
            Response::Submit(v) => Closed::Submitted(v.clone()),
            Response::Dismiss => Closed::Dismissed,
        }
    }
}

/// The widget message's `status` (and `response`) on the server.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Open,
    Closed(Closed),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// Its template program is being fetched.
    Loading { status: Status },
    /// The `init` step runs.
    Starting { status: Status },
    /// Live: the view takes input. `error` is the last failed step's, shown inline.
    Ready { state: Value, view: Node, error: Option<String> },
    /// An event's step runs.
    Stepping { state: Value, view: Node },
    /// The step answered (submit or dismiss): the response is on its way to the server. The
    /// view stays, read-only.
    Responding { state: Value, view: Node, response: Response, attempt: u32, live: bool },
    /// A live template's program, loaded, waits for the widget to first show.
    Waiting { status: Status },
    /// A live program runs; `view` is what it last rendered (none yet: a placeholder). `error`: a
    /// message that didn't go, shown inline.
    Running { view: Option<Node>, error: Option<String> },
    /// The live program ended (it returned, threw, or broke a budget): its last view, inert, with
    /// why, and Restart.
    Stopped { view: Option<Node>, error: Option<String> },
    /// Submitted or dismissed: the final view (when `init` could draw one), read-only, with the
    /// state it was drawn from (a canvas in it still draws from it).
    Closed { view: Option<Node>, state: Value, how: Closed },
    /// The template couldn't load, or `init` failed: an error card.
    Failed { message: String, status: Status },
}

// A step's output is the large one and moves once per step; boxing it would cost an allocation
// per step for nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The template's program is compiled: ready to step, or to run live.
    Loaded(Mode),
    LoadFailed(String),
    /// A step's result, for the step [`Effect::Run`] asked for.
    Ran(Result<StepOutput, String>),
    /// The user did something in the view.
    Input(WidgetEvent),
    /// The respond request for `attempt` came back.
    Responded { attempt: u32, result: Result<(), RespondError> },
    /// The message a step sent (`send`) came back: taken, or why not.
    Sent(Result<(), RespondError>),
    /// The message changed on the server (`session.widget.responded`, a page read).
    Status(Status),
    /// Try a failed widget again from the template fetch; restart a stopped live program.
    Retry,
    /// A live widget first showed: its program starts.
    Shown,
    /// The live program rendered a view.
    Rendered(Node),
    /// The live program submitted or dismissed.
    Answered(Response),
    /// The live program sent a message to the agent.
    Sending(crate::step::SendMessage),
    /// The live program ended: returned (`None`) or with an error.
    Ended(Option<String>),
}

/// How a template runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Step,
    Live,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RespondError {
    /// 409: it was already answered, here or on another client; its update is on the way.
    Conflict,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Fetch (or find cached) the template's program; answer with `Loaded` or `LoadFailed`.
    Fetch,
    /// Run this step on the program; answer with `Ran`.
    Run(StepInput),
    /// Post the response; answer with `Responded` carrying `attempt`.
    Respond { response: Response, attempt: u32 },
    /// Post a non-final message to the agent; answer with `Sent`. The widget stays open.
    Send(crate::step::SendMessage),
    /// Start (or restart) the live program.
    Start,
    /// Deliver what the user did to the live program.
    Deliver(WidgetEvent),
    /// Stop the live program.
    Stop,
}

/// What a renderer draws for a widget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Face<'a> {
    Loading,
    Failed(&'a str),
    /// `live`: it takes input now. `error`: a failed step's message, shown inline. `restart`: a
    /// live program that ended, which Restart (the retry action) starts again.
    Live { view: &'a Node, live: bool, sending: bool, error: Option<&'a str>, restart: bool },
    Closed { view: Option<&'a Node>, how: &'a Closed },
}

/// One widget message, as this client runs it.
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    /// The message's `data`, which every step takes.
    pub data: Value,
    pub phase: Phase,
    /// The last respond attempt's number, so a late answer to an older one is dropped.
    attempts: u32,
}

impl Widget {
    /// A widget for a message carrying `data`, as the server reports it; its first effect fetches
    /// the template.
    pub fn open(data: Value, status: Status) -> (Widget, Vec<Effect>) {
        (Widget { data, phase: Phase::Loading { status }, attempts: 0 }, vec![Effect::Fetch])
    }

    pub fn face(&self) -> Face<'_> {
        match &self.phase {
            Phase::Loading { .. } | Phase::Starting { .. } | Phase::Waiting { .. } | Phase::Running { view: None, .. } => Face::Loading,
            Phase::Failed { message, .. } => Face::Failed(message),
            Phase::Ready { view, error, .. } | Phase::Running { view: Some(view), error } => Face::Live { view, live: true, sending: false, error: error.as_deref(), restart: false },
            Phase::Stepping { view, .. } => Face::Live { view, live: false, sending: false, error: None, restart: false },
            Phase::Responding { view, .. } => Face::Live { view, live: false, sending: true, error: None, restart: false },
            Phase::Stopped { view: Some(view), error } => Face::Live { view, live: false, sending: false, error: error.as_deref(), restart: true },
            Phase::Stopped { view: None, error } => Face::Failed(error.as_deref().unwrap_or("The widget stopped.")),
            Phase::Closed { view, how, .. } => Face::Closed { view: view.as_ref(), how },
        }
    }

    /// The state the view shown now was drawn from (what a canvas in it draws from).
    pub fn state(&self) -> Option<&Value> {
        match &self.phase {
            Phase::Ready { state, .. } | Phase::Stepping { state, .. } | Phase::Responding { state, .. } | Phase::Closed { state, .. } => Some(state),
            Phase::Loading { .. } | Phase::Starting { .. } | Phase::Failed { .. } | Phase::Waiting { .. } | Phase::Running { .. } | Phase::Stopped { .. } => None,
        }
    }

    /// The view shown now, when there is one.
    pub fn view(&self) -> Option<&Node> {
        match &self.phase {
            Phase::Ready { view, .. } | Phase::Stepping { view, .. } | Phase::Responding { view, .. } => Some(view),
            Phase::Closed { view, .. } | Phase::Running { view, .. } | Phase::Stopped { view, .. } => view.as_ref(),
            Phase::Loading { .. } | Phase::Starting { .. } | Phase::Failed { .. } | Phase::Waiting { .. } => None,
        }
    }

    /// Whether the user can still answer it here.
    pub fn open_here(&self) -> bool {
        matches!(self.phase, Phase::Ready { .. } | Phase::Stepping { .. } | Phase::Running { .. })
    }

    /// Whether a live program runs it.
    pub fn running(&self) -> bool {
        matches!(self.phase, Phase::Running { .. })
    }

    /// Whether it is a live widget waiting to first show.
    pub fn waiting(&self) -> bool {
        matches!(self.phase, Phase::Waiting { .. })
    }
}

/// The transition function.
pub fn step(w: &mut Widget, event: Event) -> Vec<Effect> {
    let data = &w.data;
    let attempt = w.attempts + 1;
    let phase = std::mem::replace(&mut w.phase, Phase::Loading { status: Status::Open });
    let (next, effects) = match (phase, event) {
        (Phase::Loading { status }, Event::Loaded(Mode::Step)) => (Phase::Starting { status }, vec![Effect::Run(StepInput::Init { data: data.clone() })]),
        // A live widget answered already has no view to show: its program never ran here.
        (Phase::Loading { status }, Event::Loaded(Mode::Live)) => match status {
            Status::Closed(how) => (Phase::Closed { view: None, state: Value::Null, how }, vec![]),
            status => (Phase::Waiting { status }, vec![]),
        },
        (Phase::Waiting { .. }, Event::Shown) => (Phase::Running { view: None, error: None }, vec![Effect::Start]),
        (Phase::Running { error, .. }, Event::Rendered(view)) => (Phase::Running { view: Some(view), error }, vec![]),
        (Phase::Running { view, error }, Event::Input(event)) => (Phase::Running { view, error }, vec![Effect::Deliver(event)]),
        (Phase::Running { view, .. }, Event::Sending(message)) => (Phase::Running { view, error: None }, vec![Effect::Send(message)]),
        (Phase::Running { view, .. }, Event::Answered(response)) => {
            let view = view.unwrap_or_else(|| Node::Column { children: vec![], gap: None, align: Default::default() });
            let effect = Effect::Respond { response: response.clone(), attempt };
            (Phase::Responding { state: Value::Null, view, response, attempt, live: true }, vec![Effect::Stop, effect])
        }
        (Phase::Running { view, .. }, Event::Ended(error)) => (Phase::Stopped { view, error }, vec![]),
        (Phase::Running { view, .. }, Event::Sent(Err(e))) => (Phase::Running { view, error: Some(sent_error(e)) }, vec![]),
        (Phase::Stopped { view, .. }, Event::Retry) => (Phase::Running { view, error: None }, vec![Effect::Start]),
        (Phase::Waiting { .. }, Event::Status(Status::Closed(how))) => (Phase::Closed { view: None, state: Value::Null, how }, vec![]),
        (Phase::Running { view, .. }, Event::Status(Status::Closed(how))) => (Phase::Closed { view, state: Value::Null, how }, vec![Effect::Stop]),
        (Phase::Stopped { view, .. }, Event::Status(Status::Closed(how))) => (Phase::Closed { view, state: Value::Null, how }, vec![]),
        (Phase::Loading { status }, Event::LoadFailed(message)) => match status {
            // A closed widget still says how it ended without its view.
            Status::Closed(how) => (Phase::Closed { view: None, state: Value::Null, how }, vec![]),
            status => (Phase::Failed { message, status }, vec![]),
        },
        // Still loading or starting, it waits for the template or init, which then draws the
        // final view read-only; a failed one just says how it ended.
        (Phase::Loading { .. }, Event::Status(status @ Status::Closed(_))) => (Phase::Loading { status }, vec![]),
        (Phase::Starting { .. }, Event::Status(status @ Status::Closed(_))) => (Phase::Starting { status }, vec![]),
        (Phase::Failed { .. }, Event::Status(Status::Closed(how))) => (Phase::Closed { view: None, state: Value::Null, how }, vec![]),
        (Phase::Starting { status }, Event::Ran(result)) => match (result, status) {
            (Ok(out), Status::Open) => (Phase::Ready { state: out.state, view: out.view, error: None }, vec![]),
            (Ok(out), Status::Closed(how)) => (Phase::Closed { view: Some(out.view), state: out.state, how }, vec![]),
            (Err(message), Status::Open) => (Phase::Failed { message, status: Status::Open }, vec![]),
            (Err(_), Status::Closed(how)) => (Phase::Closed { view: None, state: Value::Null, how }, vec![]),
        },
        (Phase::Ready { state, view, .. }, Event::Input(event)) => {
            let input = StepInput::Event { data: data.clone(), state: state.clone(), event };
            (Phase::Stepping { state, view }, vec![Effect::Run(input)])
        }
        (Phase::Stepping { state, view }, Event::Ran(result)) => match result {
            Ok(out) => match out.response() {
                Some(response) => {
                    let effect = Effect::Respond { response: response.clone(), attempt };
                    (Phase::Responding { state: out.state, view: out.view, response, attempt, live: false }, vec![effect])
                }
                None => {
                    let effects = out.send.map(Effect::Send).into_iter().collect();
                    (Phase::Ready { state: out.state, view: out.view, error: None }, effects)
                }
            },
            Err(message) => (Phase::Ready { state, view, error: Some(message) }, vec![]),
        },
        (Phase::Responding { state, view, response, attempt, live }, Event::Responded { attempt: due, result }) if attempt == due => match result {
            Ok(()) => (Phase::Closed { view: Some(view), state, how: Closed::from_response(&response) }, vec![]),
            // A live program stopped when it answered: the answer that didn't go ends it here.
            Err(e) if live => (Phase::Stopped { view: Some(view), error: Some(sent_error(e)) }, vec![]),
            // Answered elsewhere first: its update closes it; meanwhile it says so.
            Err(RespondError::Conflict) => (Phase::Ready { state, view, error: Some("This widget was already answered.".into()) }, vec![]),
            Err(RespondError::Failed(message)) => (Phase::Ready { state, view, error: Some(message) }, vec![]),
        },
        (Phase::Ready { view, state, .. } | Phase::Stepping { view, state } | Phase::Responding { view, state, .. }, Event::Status(Status::Closed(how))) => {
            (Phase::Closed { view: Some(view), state, how }, vec![])
        }
        // A message that didn't go says so inline; the widget stays open either way.
        (Phase::Ready { state, view, .. }, Event::Sent(Err(e))) => (Phase::Ready { state, view, error: Some(sent_error(e)) }, vec![]),
        (Phase::Failed { status, .. }, Event::Retry) => (Phase::Loading { status }, vec![Effect::Fetch]),
        (phase, _) => (phase, vec![]),
    };
    if effects.iter().any(|e| matches!(e, Effect::Respond { .. })) {
        w.attempts = attempt;
    }
    w.phase = next;
    effects
}

/// What a message that didn't go says.
fn sent_error(e: RespondError) -> String {
    match e {
        RespondError::Conflict => "This widget is no longer open.".to_owned(),
        RespondError::Failed(m) => m,
    }
}

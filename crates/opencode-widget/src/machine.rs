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
    Responding { state: Value, view: Node, response: Response, attempt: u32 },
    /// Submitted or dismissed: the final view (when `init` could draw one), read-only.
    Closed { view: Option<Node>, how: Closed },
    /// The template couldn't load, or `init` failed: an error card.
    Failed { message: String, status: Status },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The template's program is compiled and ready to step.
    Loaded,
    LoadFailed(String),
    /// A step's result, for the step [`Effect::Run`] asked for.
    Ran(Result<StepOutput, String>),
    /// The user did something in the view.
    Input(WidgetEvent),
    /// The respond request for `attempt` came back.
    Responded { attempt: u32, result: Result<(), RespondError> },
    /// The message changed on the server (`session.widget.responded`, a page read).
    Status(Status),
    /// Try a failed widget again from the template fetch.
    Retry,
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
}

/// What a renderer draws for a widget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Face<'a> {
    Loading,
    Failed(&'a str),
    /// `live`: it takes input now. `error`: a failed step's message, shown inline.
    Live { view: &'a Node, live: bool, sending: bool, error: Option<&'a str> },
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
            Phase::Loading { .. } | Phase::Starting { .. } => Face::Loading,
            Phase::Failed { message, .. } => Face::Failed(message),
            Phase::Ready { view, error, .. } => Face::Live { view, live: true, sending: false, error: error.as_deref() },
            Phase::Stepping { view, .. } => Face::Live { view, live: false, sending: false, error: None },
            Phase::Responding { view, .. } => Face::Live { view, live: false, sending: true, error: None },
            Phase::Closed { view, how } => Face::Closed { view: view.as_ref(), how },
        }
    }

    /// Whether the user can still answer it here.
    pub fn open_here(&self) -> bool {
        matches!(self.phase, Phase::Ready { .. } | Phase::Stepping { .. })
    }
}

/// The transition function.
pub fn step(w: &mut Widget, event: Event) -> Vec<Effect> {
    let data = &w.data;
    let attempt = w.attempts + 1;
    let phase = std::mem::replace(&mut w.phase, Phase::Loading { status: Status::Open });
    let (next, effects) = match (phase, event) {
        (Phase::Loading { status }, Event::Loaded) => (Phase::Starting { status }, vec![Effect::Run(StepInput::Init { data: data.clone() })]),
        (Phase::Loading { status }, Event::LoadFailed(message)) => match status {
            // A closed widget still says how it ended without its view.
            Status::Closed(how) => (Phase::Closed { view: None, how }, vec![]),
            status => (Phase::Failed { message, status }, vec![]),
        },
        // Still loading or starting, it waits for the template or init, which then draws the
        // final view read-only; a failed one just says how it ended.
        (Phase::Loading { .. }, Event::Status(status @ Status::Closed(_))) => (Phase::Loading { status }, vec![]),
        (Phase::Starting { .. }, Event::Status(status @ Status::Closed(_))) => (Phase::Starting { status }, vec![]),
        (Phase::Failed { .. }, Event::Status(Status::Closed(how))) => (Phase::Closed { view: None, how }, vec![]),
        (Phase::Starting { status }, Event::Ran(result)) => match (result, status) {
            (Ok(out), Status::Open) => (Phase::Ready { state: out.state, view: out.view, error: None }, vec![]),
            (Ok(out), Status::Closed(how)) => (Phase::Closed { view: Some(out.view), how }, vec![]),
            (Err(message), Status::Open) => (Phase::Failed { message, status: Status::Open }, vec![]),
            (Err(_), Status::Closed(how)) => (Phase::Closed { view: None, how }, vec![]),
        },
        (Phase::Ready { state, view, .. }, Event::Input(event)) => {
            let input = StepInput::Event { data: data.clone(), state: state.clone(), event };
            (Phase::Stepping { state, view }, vec![Effect::Run(input)])
        }
        (Phase::Stepping { state, view }, Event::Ran(result)) => match result {
            Ok(out) => match out.response() {
                Some(response) => {
                    let effect = Effect::Respond { response: response.clone(), attempt };
                    (Phase::Responding { state: out.state, view: out.view, response, attempt }, vec![effect])
                }
                None => (Phase::Ready { state: out.state, view: out.view, error: None }, vec![]),
            },
            Err(message) => (Phase::Ready { state, view, error: Some(message) }, vec![]),
        },
        (Phase::Responding { state, view, response, attempt }, Event::Responded { attempt: due, result }) if attempt == due => match result {
            Ok(()) => (Phase::Closed { view: Some(view), how: Closed::from_response(&response) }, vec![]),
            // Answered elsewhere first: its update closes it; meanwhile it says so.
            Err(RespondError::Conflict) => (Phase::Ready { state, view, error: Some("This widget was already answered.".into()) }, vec![]),
            Err(RespondError::Failed(message)) => (Phase::Ready { state, view, error: Some(message) }, vec![]),
        },
        (Phase::Ready { view, .. } | Phase::Stepping { view, .. } | Phase::Responding { view, .. }, Event::Status(Status::Closed(how))) => {
            (Phase::Closed { view: Some(view), how }, vec![])
        }
        (Phase::Failed { status, .. }, Event::Retry) => (Phase::Loading { status }, vec![Effect::Fetch]),
        (phase, _) => (phase, vec![]),
    };
    if effects.iter().any(|e| matches!(e, Effect::Respond { .. })) {
        w.attempts = attempt;
    }
    w.phase = next;
    effects
}

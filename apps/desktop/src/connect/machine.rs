//! One integration's sign-in, from loading its methods to a saved connection (Desktop's
//! `providers/connect/machine.ts`). `attempt` names the async work a phase accepts results
//! from, so an answer for work the dialog moved past is stale; a server attempt opened after
//! the dialog moved on is closed again, and a credential saved meanwhile still reaches the
//! catalogs.

use std::rc::Rc;

use opencode_kit::integration::{Authorization, Integration, Method, MethodKind};

use super::form::{self, Answer};

/// What the server describes for the integration.
#[derive(Clone, Debug, PartialEq)]
pub struct Catalog {
    pub integration: Option<Integration>,
    pub methods: Vec<Method>,
}

/// What the picked method produced so far; it decides the view.
#[derive(Clone, Debug, PartialEq)]
pub enum Signin {
    Key(Method),
    Browser { method: Method, auth: Authorization, failed: bool, paste: Paste },
    Code { method: Method, auth: Authorization },
    Command { method: Method, attempt: String, output: String },
    External(Method),
}

impl Signin {
    pub fn method(&self) -> &Method {
        match self {
            Self::Key(m) | Self::External(m) => m,
            Self::Browser { method, .. } | Self::Code { method, .. } | Self::Command { method, .. } => method,
        }
    }

    fn auth(&self) -> Option<&Authorization> {
        match self {
            Self::Browser { auth, .. } | Self::Code { auth, .. } => Some(auth),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EntryError {
    Required,
    Rejected(String),
}

/// `ProviderPaste` (providers/connect/machine.ts). The editor owns the transient URL; queued
/// status polls need the attempt ID but must not retain a copy of the URL the user pasted.
#[derive(Clone, Debug, PartialEq)]
pub enum Paste {
    Unsupported,
    Editing { open: Option<bool>, error: Option<EntryError> },
    Sending,
    Accepted,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Retry {
    Start,
    Poll(Signin),
    Refresh(Signin),
}

/// Every phase after a method is picked.
#[derive(Clone, Debug, PartialEq)]
pub struct Sel {
    pub attempt: u32,
    pub catalog: Rc<Catalog>,
    pub method: usize,
    pub answer: Option<Answer>,
}

impl Sel {
    fn at(&self, attempt: u32) -> Sel {
        Sel { attempt, ..self.clone() }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Loading { attempt: u32 },
    Choosing { attempt: u32, catalog: Rc<Catalog> },
    Form(Sel, form::Form),
    Starting(Sel),
    Entering(Sel, Signin, Option<EntryError>),
    Submitting(Sel, Signin),
    Waiting(Sel, Signin),
    Refreshing(Sel, Signin),
    Connected(Sel, Signin),
    Failed(Sel, String, Retry),
    Disposed,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// A command's output so far.
    Pending(String),
    Complete,
    /// The attempt is over on the server; a retry starts a new one.
    Failed(String),
    /// The status request failed; the attempt may still be open on the server.
    Unreachable(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Loaded { attempt: u32, catalog: Catalog, manual: bool },
    MethodSelected(usize),
    Form(form::Input),
    FormBrowserOpened { attempt: u32, browser: u32, opened: bool },
    EntrySubmitted(String),
    PasteToggle(bool),
    PasteEdited,
    PasteSubmit(String),
    PasteResult { attempt: u32, result: Result<(), String> },
    OauthStarted { attempt: u32, auth: Authorization },
    CommandStarted { attempt: u32, id: String },
    ExternalSaved { attempt: u32 },
    StartFailed { attempt: u32, error: String },
    BrowserOpen,
    BrowserOpened { attempt_id: String, opened: bool },
    Polled { attempt: u32, outcome: Outcome },
    SubmitSucceeded { attempt: u32 },
    SubmitFailed { attempt: u32, error: String },
    RefreshSucceeded { attempt: u32 },
    RefreshFailed { attempt: u32, error: String },
    Retry,
    Refresh,
    Back,
    Disposed,
}

impl Event {
    fn attempt(&self) -> Option<u32> {
        match self {
            Self::Loaded { attempt, .. }
            | Self::FormBrowserOpened { attempt, .. }
            | Self::OauthStarted { attempt, .. }
            | Self::CommandStarted { attempt, .. }
            | Self::ExternalSaved { attempt }
            | Self::StartFailed { attempt, .. }
            | Self::Polled { attempt, .. }
            | Self::PasteResult { attempt, .. }
            | Self::SubmitSucceeded { attempt }
            | Self::SubmitFailed { attempt, .. }
            | Self::RefreshSucceeded { attempt }
            | Self::RefreshFailed { attempt, .. } => Some(*attempt),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Load { attempt: u32 },
    Start { attempt: u32, method: Method, answer: Option<Answer> },
    OpenBrowser { attempt_id: String, url: String },
    FormOpenBrowser { attempt: u32, browser: u32, url: String },
    Poll { attempt: u32, signin: Signin, delay: bool },
    PollStop,
    Cancel { command: bool, attempt_id: String },
    Submit { attempt: u32, signin: Signin, value: String, answer: Option<Answer> },
    Paste { attempt: u32, attempt_id: String, value: String },
    Refresh { attempt: u32, method: Method },
    /// Reloads the catalogs without reporting back, for a credential saved after the dialog moved on.
    Sync,
    Complete { method: Method },
}

pub type Transition = (State, Vec<Effect>);

pub fn start() -> Transition {
    (State::Loading { attempt: 0 }, vec![Effect::Load { attempt: 0 }])
}

fn same(state: &State, effects: Vec<Effect>) -> Transition {
    (state.clone(), effects)
}

pub fn transition(state: &State, event: Event) -> Transition {
    if let Event::OauthStarted { attempt, auth } = &event
        && !started_here(state, *attempt, MethodKind::Oauth)
    {
        return same(state, vec![Effect::Cancel { command: false, attempt_id: auth.attempt.clone() }]);
    }
    if let Event::CommandStarted { attempt, id } = &event
        && !started_here(state, *attempt, MethodKind::Command)
    {
        return same(state, vec![Effect::Cancel { command: true, attempt_id: id.clone() }]);
    }
    if saved_elsewhere(state, &event) {
        return same(state, vec![Effect::Sync]);
    }
    if *state == State::Disposed {
        return same(state, vec![]);
    }
    if event == Event::Disposed {
        return (State::Disposed, cleanup(state));
    }
    if let Some(a) = event.attempt()
        && Some(a) != attempt(state)
    {
        return same(state, vec![]);
    }
    match state {
        State::Loading { attempt } => match event {
            Event::Loaded { catalog, manual, .. } if !manual && catalog.methods.len() == 1 => select(Rc::new(catalog), 0, attempt + 1, None),
            Event::Loaded { catalog, .. } => (State::Choosing { attempt: *attempt, catalog: Rc::new(catalog) }, vec![]),
            _ => same(state, vec![]),
        },
        _ => after_load(state, event),
    }
}

fn after_load(state: &State, event: Event) -> Transition {
    let (catalog, at) = match state {
        State::Choosing { attempt, catalog } => (catalog.clone(), *attempt),
        _ => match sel(state) {
            Some(s) => (s.catalog.clone(), s.attempt),
            None => return same(state, vec![]),
        },
    };
    if let Event::MethodSelected(index) = event {
        if index >= catalog.methods.len() {
            return same(state, vec![]);
        }
        let (next, effects) = select(catalog, index, at + 1, None);
        return (next, [cleanup(state), effects].concat());
    }
    let State::Choosing { .. } = state else { return selected(state, event, catalog, at) };
    same(state, vec![])
}

fn selected(state: &State, event: Event, catalog: Rc<Catalog>, at: u32) -> Transition {
    let Some(s) = sel(state) else { return same(state, vec![]) };
    match event {
        Event::Back => return (State::Choosing { attempt: at + 1, catalog }, cleanup(state)),
        Event::BrowserOpen => {
            return match signin(state).and_then(Signin::auth) {
                Some(auth) => same(state, vec![Effect::OpenBrowser { attempt_id: auth.attempt.clone(), url: auth.url.clone() }]),
                None => same(state, vec![]),
            };
        }
        Event::BrowserOpened { attempt_id, opened } => {
            let mut next = state.clone();
            if let State::Waiting(_, si) | State::Refreshing(_, si) | State::Connected(_, si) = &mut next
                && let Signin::Browser { auth, failed, .. } = si && auth.attempt == attempt_id {
                *failed = !opened;
            }
            return (next, vec![]);
        }
        _ => {}
    }
    match state {
        State::Form(s, f) => match event {
            Event::Form(input) => form_step(s, f, input),
            Event::FormBrowserOpened { browser, opened, .. } => form_step(s, f, form::Input::ExternalOpened { attempt: browser, opened }),
            _ => same(state, vec![]),
        },
        State::Starting(s) => started(s, event),
        State::Entering(s, signin, _) => match event {
            Event::EntrySubmitted(value) if value.trim().is_empty() => (State::Entering(s.clone(), signin.clone(), Some(EntryError::Required)), vec![]),
            Event::EntrySubmitted(value) => {
                let attempt = s.attempt + 1;
                let effect = Effect::Submit { attempt, signin: signin.clone(), value, answer: s.answer.clone() };
                (State::Submitting(s.at(attempt), signin.clone()), vec![effect])
            }
            _ => same(state, vec![]),
        },
        State::Submitting(s, signin) => match event {
            Event::SubmitSucceeded { .. } => refresh(s, signin.clone()),
            Event::SubmitFailed { error, .. } => (State::Entering(s.clone(), signin.clone(), Some(EntryError::Rejected(error))), vec![]),
            _ => same(state, vec![]),
        },
        State::Waiting(s, signin) => polled(s, signin, event),
        State::Refreshing(s, signin) => match event {
            Event::RefreshSucceeded { .. } => (State::Connected(s.clone(), signin.clone()), vec![Effect::Complete { method: signin.method().clone() }]),
            Event::RefreshFailed { error, .. } => (State::Failed(s.clone(), error, Retry::Refresh(signin.clone())), vec![]),
            _ => same(state, vec![]),
        },
        State::Connected(s, signin) => match event {
            Event::Refresh => refresh(s, signin.clone()),
            _ => same(state, vec![]),
        },
        State::Failed(s, _, retry) => match (event, retry) {
            (Event::Retry, Retry::Refresh(signin)) => refresh(s, signin.clone()),
            (Event::Retry, Retry::Poll(signin)) => {
                let mut next = signin.clone();
                // A paste reply can only settle its old attempt. Unlock the field on retry.
                if let Signin::Browser { paste: p @ (Paste::Sending | Paste::Accepted), .. } = &mut next {
                    *p = Paste::Editing { open: Some(true), error: None };
                }
                poll(&s.at(s.attempt + 1), next)
            }
            // The form was answered, so a retry goes straight back to the server.
            (Event::Retry, Retry::Start) => select(catalog, s.method, at + 1, Some(s.answer.clone().unwrap_or_default())),
            _ => same(state, vec![]),
        },
        _ => {
            let _ = s;
            same(state, vec![])
        }
    }
}

/// What a screen shows for the state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Busy,
    Methods,
    EnvironmentOnly,
    Form,
    Failed,
    Key,
    Code,
    Browser,
    Command,
}

pub fn screen(state: &State) -> Screen {
    match state {
        State::Choosing { catalog, .. } if catalog.methods.is_empty() => Screen::EnvironmentOnly,
        State::Choosing { .. } => Screen::Methods,
        State::Form(..) => Screen::Form,
        State::Failed(..) => Screen::Failed,
        _ => match signin(state) {
            Some(Signin::Key(_)) => Screen::Key,
            Some(Signin::Code { .. }) => Screen::Code,
            Some(Signin::Browser { .. }) => Screen::Browser,
            Some(Signin::Command { .. }) => Screen::Command,
            _ => Screen::Busy,
        },
    }
}

pub fn attempt(state: &State) -> Option<u32> {
    match state {
        State::Loading { attempt } | State::Choosing { attempt, .. } => Some(*attempt),
        State::Disposed => None,
        _ => sel(state).map(|s| s.attempt),
    }
}

pub fn sel(state: &State) -> Option<&Sel> {
    match state {
        State::Form(s, _)
        | State::Starting(s)
        | State::Entering(s, ..)
        | State::Submitting(s, _)
        | State::Waiting(s, _)
        | State::Refreshing(s, _)
        | State::Connected(s, _)
        | State::Failed(s, ..) => Some(s),
        _ => None,
    }
}

pub fn signin(state: &State) -> Option<&Signin> {
    match state {
        State::Entering(_, s, _) | State::Submitting(_, s) | State::Waiting(_, s) | State::Refreshing(_, s) | State::Connected(_, s) => Some(s),
        _ => None,
    }
}

pub fn catalog(state: &State) -> Option<&Catalog> {
    match state {
        State::Choosing { catalog, .. } => Some(catalog),
        _ => sel(state).map(|s| &*s.catalog),
    }
}

/// The picked method, if any.
pub fn method(state: &State) -> Option<&Method> {
    sel(state).and_then(|s| s.catalog.methods.get(s.method))
}

pub fn authorization(state: &State) -> Option<&Authorization> {
    signin(state).and_then(Signin::auth)
}

/// A typed secret is on its way or the catalogs refresh: the form takes no second submit.
pub fn submitting(state: &State) -> bool {
    matches!(state, State::Submitting(..) | State::Refreshing(..))
}

/// A browser sign-in the user may be finishing: a stray backdrop press mustn't drop it.
pub fn authorizing(state: &State) -> bool {
    authorization(state).is_some() && matches!(state, State::Waiting(..) | State::Entering(..) | State::Submitting(..) | State::Refreshing(..))
}

fn select(catalog: Rc<Catalog>, index: usize, attempt: u32, answer: Option<Answer>) -> Transition {
    let method = catalog.methods[index].clone();
    // An answered form already holds its hidden fields' defaults; one with nothing to ask answers itself.
    let created = if answer.is_some() || method.kind == MethodKind::Command { None } else { Some(form::create(&method.form)) };
    let resolved = match (answer, created) {
        (_, Some(form::State::Active(f))) => return (State::Form(Sel { attempt, catalog, method: index, answer: None }, f), vec![]),
        (Some(a), _) => a,
        (None, Some(form::State::Completed(a))) => a,
        (None, None) => Answer::new(),
    };
    let answer = (!resolved.is_empty()).then_some(resolved);
    let s = Sel { attempt, catalog, method: index, answer: answer.clone() };
    if method.kind == MethodKind::Key {
        return (State::Entering(s, Signin::Key(method), None), vec![]);
    }
    (State::Starting(s), vec![Effect::Start { attempt, method, answer }])
}

fn form_step(s: &Sel, f: &form::Form, input: form::Input) -> Transition {
    let (next, open) = form::transition(f, input);
    match next {
        None => (State::Form(s.clone(), f.clone()), vec![]),
        Some(form::State::Completed(answer)) => select(s.catalog.clone(), s.method, s.attempt + 1, Some(answer)),
        Some(form::State::Active(next)) => {
            let effects = open.into_iter().map(|o| Effect::FormOpenBrowser { attempt: s.attempt, browser: o.attempt, url: o.url }).collect();
            (State::Form(s.clone(), next), effects)
        }
    }
}

fn started(s: &Sel, event: Event) -> Transition {
    let method = s.catalog.methods[s.method].clone();
    match (event, method.kind) {
        (Event::StartFailed { error, .. }, _) => (State::Failed(s.clone(), error, Retry::Start), vec![]),
        (Event::ExternalSaved { .. }, MethodKind::External) => refresh(s, Signin::External(method)),
        (Event::CommandStarted { id, .. }, MethodKind::Command) => poll(s, Signin::Command { method, attempt: id, output: String::new() }),
        (Event::OauthStarted { auth, .. }, MethodKind::Oauth) => {
            // As `opencode auth login`: straight to the browser, no link to click first.
            let open = Effect::OpenBrowser { attempt_id: auth.attempt.clone(), url: auth.url.clone() };
            if auth.code {
                return (State::Entering(s.clone(), Signin::Code { method, auth }, None), vec![open]);
            }
            let paste = if auth.paste { Paste::Editing { open: None, error: None } } else { Paste::Unsupported };
            let (next, effects) = poll(s, Signin::Browser { method, auth, failed: false, paste });
            (next, [vec![open], effects].concat())
        }
        _ => (State::Starting(s.clone()), vec![]),
    }
}

fn polled(s: &Sel, signin: &Signin, event: Event) -> Transition {
    if let Signin::Browser { .. } = signin && !matches!(event, Event::Polled { .. }) {
        let mut next = signin.clone();
        let Signin::Browser { auth, paste, .. } = &mut next else { unreachable!() };
        let mut effects = vec![];
        match (&mut *paste, event) {
            (Paste::Editing { open, .. }, Event::PasteToggle(value)) => *open = Some(value),
            (Paste::Editing { error, .. }, Event::PasteEdited) => *error = None,
            (Paste::Editing { .. }, Event::PasteSubmit(value)) if !value.trim().is_empty() => {
                *paste = Paste::Sending;
                effects.push(Effect::Paste { attempt: s.attempt, attempt_id: auth.attempt.clone(), value: value.trim().to_owned() });
            }
            (Paste::Editing { .. }, Event::PasteSubmit(_)) => *paste = Paste::Editing { open: Some(true), error: Some(EntryError::Required) },
            (Paste::Sending, Event::PasteResult { result, .. }) => *paste = match result {
                Ok(()) => Paste::Accepted,
                Err(message) => Paste::Editing { open: Some(true), error: Some(EntryError::Rejected(message)) },
            },
            _ => {}
        }
        return (State::Waiting(s.clone(), next), effects);
    }
    let Event::Polled { outcome, .. } = event else { return (State::Waiting(s.clone(), signin.clone()), vec![]) };
    match outcome {
        Outcome::Complete => refresh(s, signin.clone()),
        Outcome::Pending(output) => {
            let next = match signin {
                Signin::Command { method, attempt, .. } => Signin::Command { method: method.clone(), attempt: attempt.clone(), output },
                other => other.clone(),
            };
            (State::Waiting(s.clone(), next.clone()), vec![Effect::Poll { attempt: s.attempt, signin: next, delay: true }])
        }
        // Only a browser attempt stays open on the server after its status request fails.
        Outcome::Unreachable(error) if matches!(signin, Signin::Browser { .. }) => (State::Failed(s.clone(), error, Retry::Poll(signin.clone())), vec![]),
        Outcome::Unreachable(error) | Outcome::Failed(error) => (State::Failed(s.clone(), error, Retry::Start), vec![]),
    }
}

fn poll(s: &Sel, signin: Signin) -> Transition {
    (State::Waiting(s.clone(), signin.clone()), vec![Effect::Poll { attempt: s.attempt, signin, delay: false }])
}

fn refresh(s: &Sel, signin: Signin) -> Transition {
    let attempt = s.attempt + 1;
    let method = signin.method().clone();
    (State::Refreshing(s.at(attempt), signin), vec![Effect::Refresh { attempt, method }])
}

/// Stops the work the state left running: its poll and any server attempt still open.
pub fn cleanup(state: &State) -> Vec<Effect> {
    let signin = match state {
        State::Failed(_, _, Retry::Poll(s)) => Some(s),
        State::Waiting(_, s) | State::Entering(_, s, _) | State::Submitting(_, s) => Some(s),
        _ => None,
    };
    let mut out = if matches!(state, State::Waiting(..)) { vec![Effect::PollStop] } else { vec![] };
    match signin {
        Some(Signin::Browser { auth, .. } | Signin::Code { auth, .. }) => out.push(Effect::Cancel { command: false, attempt_id: auth.attempt.clone() }),
        Some(Signin::Command { attempt, .. }) => out.push(Effect::Cancel { command: true, attempt_id: attempt.clone() }),
        _ => {}
    }
    out
}

fn saved_elsewhere(state: &State, event: &Event) -> bool {
    match event {
        Event::SubmitSucceeded { attempt } => !matches!(state, State::Submitting(s, _) if s.attempt == *attempt),
        Event::ExternalSaved { attempt } => !started_here(state, *attempt, MethodKind::External),
        Event::Polled { attempt, outcome: Outcome::Complete } => !matches!(state, State::Waiting(s, _) if s.attempt == *attempt),
        _ => false,
    }
}

fn started_here(state: &State, attempt: u32, kind: MethodKind) -> bool {
    matches!(state, State::Starting(s) if s.attempt == attempt && s.catalog.methods.get(s.method).is_some_and(|m| m.kind == kind))
}

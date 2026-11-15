//! The location dialog's steps, as Desktop's machine runs them (`new-session/location/machine.ts`):
//! Projects, Other projects and a project's worktrees, each entered from another step that it
//! returns to intact (`history`), with the row that was lit there. A worktree step the dialog
//! opened on (`direct`) closes on Escape (`machine.ts:167-168`).

use rune::machine::Effects;

use super::StepMode;

/// A step to return to, with its query and the row lit on it (`LocationHistoryItem`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Frame {
    pub mode: StepMode,
    pub query: String,
    pub active: Option<String>,
}

pub(crate) enum Event {
    /// → on a Git project, or its chevron (`drilled`), on `server` (None: the window's).
    Drilled { project: String, server: Option<opencode_kit::servers::ServerKey>, active: Option<String> },
    /// Add project with several servers: which server (`chosen` with `add`, `choosesServer`).
    Servers { active: Option<String> },
    /// Other projects, chosen (`chosen` with an `other` entry).
    Others { active: Option<String> },
    Base { directory: String },
    BranchChosen(String),
    /// The step's title, or a key that steps back (`back`).
    Back,
    /// A key in the field (`key.down`): `forward` and `backward` are → and ← as the reading
    /// direction has them.
    Key(KeyDown),
}

pub(crate) struct KeyDown {
    pub key: Pressed,
    pub at_start: bool,
    pub at_end: bool,
    pub empty: bool,
    pub active: Option<Lit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pressed {
    Forward,
    Backward,
    Backspace,
    Escape,
    Other,
}

/// What the lit row is, for the keys that act on it.
pub(crate) enum Lit {
    Project { id: String, server: Option<opencode_kit::servers::ServerKey>, key: String, drillable: bool },
    Other,
    /// Add project, while it asks which server.
    Add,
    Row,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Light `active` (or the first row) and keep the keys in the field (`focus`).
    Focus(Option<String>),
    /// The new step's field starts on `query`, the caret at its end.
    Query(String),
    Close,
}

/// The dialog's step machine. Returns the effects and whether the key was the dialog's.
pub(crate) fn step(mode: &mut StepMode, query: &str, history: &mut Vec<Frame>, event: Event) -> (Effects<Effect>, bool) {
    match event {
        Event::Drilled { project, server, active } => {
            if !matches!(mode, StepMode::Projects | StepMode::Others) {
                return (Effects::none(), false);
            }
            (enter(mode, query, history, active, StepMode::Worktrees { project, server, direct: false, base: None }), false)
        }
        Event::Base { directory } => {
            let StepMode::Worktrees { project, server, .. } = &*mode else { return (Effects::none(), false) };
            let next = StepMode::Branches { project: project.clone(), server: server.clone(), direct: false, directory, list: Default::default() };
            (enter(mode, query, history, Some("base".to_owned()), next), true)
        }
        Event::BranchChosen(branch) => {
            let StepMode::Branches { project, server, direct, .. } = &*mode else { return (Effects::none(), false) };
            // A branch sets only the dialog's proposed base. The draft changes when Create is chosen.
            if history.last().is_some_and(|f| matches!(f.mode, StepMode::Worktrees { .. })) {
                let effects = back(mode, history);
                if let StepMode::Worktrees { base, .. } = mode { *base = Some(branch); }
                return (effects, true);
            }
            *mode = StepMode::Worktrees { project: project.clone(), server: server.clone(), direct: *direct, base: Some(branch) };
            (Effects::two(Effect::Query(String::new()), Effect::Focus(None)), true)
        }
        // Picking a server again from the server step keeps its search and the way back.
        Event::Servers { active } => {
            if *mode == StepMode::Servers {
                return (Effects::none(), false);
            }
            (enter(mode, query, history, active, StepMode::Servers), false)
        }
        Event::Others { active } => {
            if *mode != StepMode::Projects {
                return (Effects::none(), false);
            }
            (enter(mode, query, history, active, StepMode::Others), false)
        }
        Event::Back => (back(mode, history), false),
        Event::Key(k) => key(mode, query, history, k),
    }
}

fn key(mode: &mut StepMode, query: &str, history: &mut Vec<Frame>, k: KeyDown) -> (Effects<Effect>, bool) {
    if k.key == Pressed::Forward && k.at_end {
        match k.active {
            Some(Lit::Project { id, server, key, drillable: true }) => return (step(mode, query, history, Event::Drilled { project: id, server, active: Some(key) }).0, true),
            Some(Lit::Other) if *mode == StepMode::Projects => return (step(mode, query, history, Event::Others { active: Some("other".into()) }).0, true),
            Some(Lit::Add) => return (step(mode, query, history, Event::Servers { active: Some("+add".into()) }).0, true),
            _ => {}
        }
    }
    if *mode == StepMode::Projects {
        return (Effects::none(), false);
    }
    if k.key == Pressed::Escape && matches!(mode, StepMode::Worktrees { direct: true, .. } | StepMode::Branches { direct: true, .. }) {
        return (Effects::one(Effect::Close), true);
    }
    if (k.key == Pressed::Backward && k.at_start) || (k.key == Pressed::Backspace && k.empty) || k.key == Pressed::Escape {
        return (back(mode, history), true);
    }
    (Effects::none(), false)
}

/// Leaves this step for `next`, remembering it and its lit row to come back to.
fn enter(mode: &mut StepMode, query: &str, history: &mut Vec<Frame>, active: Option<String>, next: StepMode) -> Effects<Effect> {
    let from = std::mem::replace(mode, next);
    history.push(Frame { mode: from, query: query.to_owned(), active });
    Effects::two(Effect::Query(String::new()), Effect::Focus(None))
}

fn back(mode: &mut StepMode, history: &mut Vec<Frame>) -> Effects<Effect> {
    let Some(top) = history.pop() else { return Effects::none() };
    *mode = top.mode;
    Effects::two(Effect::Query(top.query), Effect::Focus(top.active))
}

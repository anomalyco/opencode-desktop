//! Desktop's jump-to-message machine (`session/commands/jump-to-message-machine.ts`): the dialog
//! browses the loaded timeline's prompts at once while older ones load, and opens a prompt that
//! isn't loaded by paging the timeline back to it. `Failed` is a history that didn't load;
//! `Retry` loads it again. A failed open returns to `Ready` with `failed` set, and choosing the
//! prompt again tries again.
//!
//! Beyond Desktop's: attempts keep counting across openings (`Closed` keeps the last), so a read
//! from a dialog already closed can't land in the next one; and an open's 60 s bound
//! (`AbortSignal.timeout(60_000)` in `reveal`) is the `Opening` state's own `due`.

use rune::machine::Effects;

/// How long an open may page back before it fails (`dialog-jump-to-message.tsx`).
pub const OPEN_SECS: f64 = 60.0;

/// A sent prompt (`JumpPrompt`): its words and when it was sent, in Unix ms.
#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    pub id: String,
    pub text: String,
    pub created: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Closed { last: u32 },
    Loading { attempt: u32 },
    Failed { attempt: u32 },
    Ready { attempt: u32, history: Vec<Prompt>, failed: bool },
    Opening { attempt: u32, history: Vec<Prompt>, target: String, due: f64 },
}

impl Default for State {
    fn default() -> Self {
        State::Closed { last: 0 }
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    /// The dialog opened (`startJumpToMessage`).
    Start,
    HistoryLoaded { attempt: u32, history: Vec<Prompt> },
    HistoryFailed { attempt: u32 },
    Retry,
    /// A prompt was chosen; `loaded` when the timeline already holds it.
    Selected { id: String, loaded: bool, now: f64 },
    Opened { attempt: u32 },
    OpenFailed { attempt: u32 },
    Disposed,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    HistoryLoad { attempt: u32 },
    OpenLoad { attempt: u32, target: String },
    Jump { id: String },
    Cancel,
}

impl State {
    fn attempt(&self) -> u32 {
        match self {
            State::Closed { last } => *last,
            State::Loading { attempt } | State::Failed { attempt } | State::Ready { attempt, .. } | State::Opening { attempt, .. } => *attempt,
        }
    }

    pub fn is_open(&self) -> bool {
        !matches!(self, State::Closed { .. })
    }

    /// `selectJumpHistory`.
    pub fn history(&self) -> &[Prompt] {
        match self {
            State::Ready { history, .. } | State::Opening { history, .. } => history,
            _ => &[],
        }
    }

    /// `selectJumpOpening`.
    pub fn opening(&self) -> bool {
        matches!(self, State::Opening { .. })
    }

    /// `selectJumpStatus`: the note's catalog key, if any.
    pub fn status(&self) -> Option<opencode_i18n::Key> {
        match self {
            State::Opening { .. } => Some(opencode_i18n::key!("dialog.jump.opening")),
            State::Failed { .. } => Some(opencode_i18n::key!("dialog.jump.failed")),
            State::Ready { failed: true, .. } => Some(opencode_i18n::key!("dialog.jump.openFailed")),
            State::Loading { .. } => Some(opencode_i18n::key!("dialog.jump.loading")),
            _ => None,
        }
    }

    /// `transitionJumpToMessage`.
    pub fn step(&mut self, event: Event) -> Effects<Effect> {
        let attempt = self.attempt();
        match (&*self, event) {
            (State::Closed { last }, Event::Start) => {
                let attempt = last + 1;
                *self = State::Loading { attempt };
                Effects::one(Effect::HistoryLoad { attempt })
            }
            (State::Closed { .. }, _) => Effects::none(),
            (_, Event::Start) => Effects::none(),
            (_, Event::Disposed) => {
                *self = State::Closed { last: attempt };
                Effects::one(Effect::Cancel)
            }
            (State::Opening { .. }, Event::Selected { .. }) => Effects::none(),
            // Rows beyond the loaded timeline only exist once the older history has loaded.
            (_, Event::Selected { id, loaded: true, .. }) => {
                *self = State::Closed { last: attempt };
                Effects::two(Effect::Cancel, Effect::Jump { id })
            }
            (State::Ready { history, .. }, Event::Selected { id, loaded: false, now }) => {
                let attempt = attempt + 1;
                *self = State::Opening { attempt, history: history.clone(), target: id.clone(), due: now + OPEN_SECS };
                Effects::one(Effect::OpenLoad { attempt, target: id })
            }
            (_, Event::Selected { .. }) => Effects::none(),
            (State::Failed { .. }, Event::Retry) => {
                *self = State::Loading { attempt: attempt + 1 };
                Effects::one(Effect::HistoryLoad { attempt: attempt + 1 })
            }
            (_, Event::Retry) => Effects::none(),
            (State::Loading { .. }, Event::HistoryLoaded { attempt: a, history }) if a == attempt => {
                *self = State::Ready { attempt, history, failed: false };
                Effects::none()
            }
            (State::Loading { .. }, Event::HistoryFailed { attempt: a }) if a == attempt => {
                *self = State::Failed { attempt };
                Effects::none()
            }
            (State::Opening { target, .. }, Event::Opened { attempt: a }) if a == attempt => {
                let id = target.clone();
                *self = State::Closed { last: attempt };
                Effects::one(Effect::Jump { id })
            }
            (State::Opening { history, .. }, Event::OpenFailed { attempt: a }) if a == attempt => {
                *self = State::Ready { attempt, history: history.clone(), failed: true };
                Effects::none()
            }
            _ => Effects::none(),
        }
    }
}

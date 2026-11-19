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
pub enum State<T = ()> {
    Closed { last: u32 },
    Loading { attempt: u32, open: T },
    Failed { attempt: u32, open: T },
    Ready { attempt: u32, history: Vec<Prompt>, failed: bool, open: T },
    Opening { attempt: u32, history: Vec<Prompt>, target: String, due: f64, open: T },
}

impl<T> Default for State<T> {
    fn default() -> Self {
        State::Closed { last: 0 }
    }
}

#[derive(Clone, Debug)]
pub enum Event<T = ()> {
    /// The dialog opened (`startJumpToMessage`), carrying open-only dialog state `T`.
    Start(T),
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
pub enum Effect<T = ()> {
    HistoryLoad { attempt: u32 },
    OpenLoad { attempt: u32, target: String },
    Jump { id: String, open: T },
    Cancel,
    Closed(T),
}

impl<T> State<T> {
    pub fn is_open(&self) -> bool {
        !matches!(self, State::Closed { .. })
    }

    pub fn open_data(&self) -> Option<&T> {
        match self {
            State::Closed { .. } => None,
            State::Loading { open, .. }
            | State::Failed { open, .. }
            | State::Ready { open, .. }
            | State::Opening { open, .. } => Some(open),
        }
    }

    pub fn open_data_mut(&mut self) -> Option<&mut T> {
        match self {
            State::Closed { .. } => None,
            State::Loading { open, .. }
            | State::Failed { open, .. }
            | State::Ready { open, .. }
            | State::Opening { open, .. } => Some(open),
        }
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
    pub fn step(&mut self, event: Event<T>) -> Effects<Effect<T>> {
        let prev = std::mem::replace(self, State::Closed { last: 0 });
        let (next, effects) = match (prev, event) {
            (State::Closed { last }, Event::Start(open)) => {
                let attempt = last + 1;
                (State::Loading { attempt, open }, Effects::one(Effect::HistoryLoad { attempt }))
            }
            (state @ State::Closed { .. }, _) => (state, Effects::none()),
            (state, Event::Start(_)) => (state, Effects::none()),
            (
                State::Loading { attempt, open }
                | State::Failed { attempt, open }
                | State::Ready { attempt, open, .. }
                | State::Opening { attempt, open, .. },
                Event::Disposed,
            ) => (State::Closed { last: attempt }, Effects::two(Effect::Cancel, Effect::Closed(open))),
            (state @ State::Opening { .. }, Event::Selected { .. }) => (state, Effects::none()),
            // Rows beyond the loaded timeline only exist once the older history has loaded.
            (
                State::Loading { attempt, open }
                | State::Failed { attempt, open }
                | State::Ready { attempt, open, .. },
                Event::Selected { id, loaded: true, .. },
            ) => (State::Closed { last: attempt }, Effects::two(Effect::Cancel, Effect::Jump { id, open })),
            (State::Ready { attempt, history, open, .. }, Event::Selected { id, loaded: false, now }) => {
                let attempt = attempt + 1;
                (
                    State::Opening { attempt, history, target: id.clone(), due: now + OPEN_SECS, open },
                    Effects::one(Effect::OpenLoad { attempt, target: id }),
                )
            }
            (state, Event::Selected { .. }) => (state, Effects::none()),
            (State::Failed { attempt, open }, Event::Retry) => {
                let attempt = attempt + 1;
                (State::Loading { attempt, open }, Effects::one(Effect::HistoryLoad { attempt }))
            }
            (state, Event::Retry) => (state, Effects::none()),
            (State::Loading { attempt, open }, Event::HistoryLoaded { attempt: a, history }) if a == attempt => {
                (State::Ready { attempt, history, failed: false, open }, Effects::none())
            }
            (State::Loading { attempt, open }, Event::HistoryFailed { attempt: a }) if a == attempt => {
                (State::Failed { attempt, open }, Effects::none())
            }
            (State::Opening { attempt, target, open, .. }, Event::Opened { attempt: a }) if a == attempt => {
                (State::Closed { last: attempt }, Effects::one(Effect::Jump { id: target, open }))
            }
            (State::Opening { attempt, history, open, .. }, Event::OpenFailed { attempt: a }) if a == attempt => {
                (State::Ready { attempt, history, failed: true, open }, Effects::none())
            }
            (state, _) => (state, Effects::none()),
        };
        *self = next;
        effects
    }
}

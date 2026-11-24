//! Recovering a session whose folder is gone: OpenCode Desktop's
//! `packages/app/src/session/composer/location-missing-machine.ts`. `Picking`: the directory
//! picker is open. `Moving`: the worktree or move request runs. `Moved`: the server moved the
//! session and the card waits for the session's new folder to take it away. `restore_focus`: a
//! cancelled or failed move returns focus to Choose directory once the dialog over it closes.
//!
//! The menu's search text and lit row are the view's (`super::Menu`): the machine only says
//! whether the menu is open and whether its list is still loading.

use rune::machine::{Attempt, Effects, Seq};

/// `WORKTREES_LOAD_TIMEOUT_MS`: the menu shows what it already has, with an error, once a
/// worktree refresh and list take this long.
pub const WORKTREES_LOAD_TIMEOUT: f64 = 15.0;
/// `LOCATION_MOVE_SETTLE_TIMEOUT_MS`: a move the server accepted waits this long for the
/// session's new folder to take the card away.
pub const MOVE_SETTLE_TIMEOUT: f64 = 5.0;

/// `LocationMissingSource`: which button the running move came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Worktree,
    Directory,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    Idle { restore_focus: bool },
    MenuLoading { attempt: Attempt, due: f64 },
    MenuReady,
    Picking { attempt: Attempt },
    Moving { attempt: Attempt, source: Source },
    Moved { attempt: Attempt, source: Source, due: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Card {
    issued: Seq,
    pub state: State,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    MenuToggled { open: bool, now: f64 },
    /// The worktree refresh and list answered; `listed` is false when the list failed.
    WorktreesLoaded { attempt: Attempt, listed: bool },
    ChooseRequested,
    PickerSettled { attempt: Attempt, directory: Option<String>, dialog_open: bool },
    /// A worktree's row, or New worktree (no directory).
    MoveWorktree { directory: Option<String> },
    MoveSucceeded { attempt: Attempt, now: f64 },
    MoveFailed { attempt: Attempt, error: String, dialog_open: bool },
    /// `dialog.changed`: whether a dialog is open over the card.
    DialogChanged { open: bool },
    /// The clock: the worktrees taking too long (`worktrees.timed-out`) or the moved session's
    /// folder not arriving (`move.settle-timed-out`).
    Tick { now: f64 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    WorktreesLoad { attempt: Attempt },
    PickerOpen { attempt: Attempt },
    /// Without `directory`, the move first creates a worktree from the project's checkout.
    MoveRun { attempt: Attempt, directory: Option<String> },
    WakeAt(f64),
    FocusChoose,
    ToastWorktreesFailed,
    ToastMoveFailed { error: String },
}

/// `createLocationMissingState`.
impl Default for Card {
    fn default() -> Card {
        Card { issued: Seq::default(), state: State::Idle { restore_focus: false } }
    }
}

impl Card {
    pub fn menu_open(&self) -> bool {
        matches!(self.state, State::MenuLoading { .. } | State::MenuReady)
    }

    /// One move at a time: the desktop's native picker leaves the window usable while it is open.
    pub fn disabled(&self) -> bool {
        matches!(self.state, State::Picking { .. } | State::Moving { .. } | State::Moved { .. })
    }

    /// The button that shows the running move.
    pub fn pending(&self) -> Option<Source> {
        match self.state {
            State::Moving { source, .. } | State::Moved { source, .. } => Some(source),
            _ => None,
        }
    }

    /// Whether a timer has run out at `now` (the poll then feeds it the clock).
    pub fn due(&self, now: f64) -> bool {
        match self.state {
            State::MenuLoading { due, .. } | State::Moved { due, .. } => now >= due,
            _ => false,
        }
    }
}

/// `transitionLocationMissing`.
pub fn step(card: &mut Card, event: Event) -> Effects<Effect> {
    let choosing = matches!(card.state, State::Idle { .. } | State::MenuLoading { .. } | State::MenuReady);
    match (card.state, event) {
        (State::Idle { .. }, Event::MenuToggled { open: true, now }) => {
            let (attempt, due) = (card.issued.next(), now + WORKTREES_LOAD_TIMEOUT);
            card.state = State::MenuLoading { attempt, due };
            Effects::two(Effect::WorktreesLoad { attempt }, Effect::WakeAt(due))
        }
        (State::MenuLoading { .. } | State::MenuReady, Event::MenuToggled { open: false, .. }) => {
            card.state = State::Idle { restore_focus: false };
            Effects::none()
        }
        (State::MenuLoading { attempt, .. }, Event::WorktreesLoaded { attempt: of, listed }) if attempt == of => {
            card.state = State::MenuReady;
            if listed { Effects::none() } else { Effects::one(Effect::ToastWorktreesFailed) }
        }
        (State::MenuLoading { due, .. }, Event::Tick { now }) if now >= due => {
            card.state = State::MenuReady;
            Effects::one(Effect::ToastWorktreesFailed)
        }
        (_, Event::ChooseRequested) if choosing => {
            let attempt = card.issued.next();
            card.state = State::Picking { attempt };
            Effects::one(Effect::PickerOpen { attempt })
        }
        (State::Picking { attempt }, Event::PickerSettled { attempt: of, directory, dialog_open }) if attempt == of => match directory {
            Some(directory) => run(card, Source::Directory, Some(directory)),
            None => cancelled(card, dialog_open),
        },
        (_, Event::MoveWorktree { directory }) if choosing => run(card, Source::Worktree, directory),
        (State::Moving { attempt, source }, Event::MoveSucceeded { attempt: of, now }) if attempt == of => {
            let due = now + MOVE_SETTLE_TIMEOUT;
            card.state = State::Moved { attempt, source, due };
            Effects::one(Effect::WakeAt(due))
        }
        (State::Moving { attempt, .. }, Event::MoveFailed { attempt: of, error, dialog_open }) if attempt == of => {
            let mut effects = Effects::one(Effect::ToastMoveFailed { error });
            effects.extend(cancelled(card, dialog_open));
            effects
        }
        (State::Moved { due, .. }, Event::Tick { now }) if now >= due => {
            card.state = State::Idle { restore_focus: false };
            Effects::none()
        }
        (State::Idle { restore_focus: true }, Event::DialogChanged { open: false }) => {
            card.state = State::Idle { restore_focus: false };
            Effects::one(Effect::FocusChoose)
        }
        _ => Effects::none(),
    }
}

fn run(card: &mut Card, source: Source, directory: Option<String>) -> Effects<Effect> {
    let attempt = card.issued.next();
    card.state = State::Moving { attempt, source };
    Effects::one(Effect::MoveRun { attempt, directory })
}

/// Focus returns to Choose directory now, or once the dialog over it closes.
fn cancelled(card: &mut Card, dialog_open: bool) -> Effects<Effect> {
    card.state = State::Idle { restore_focus: dialog_open };
    if dialog_open { Effects::none() } else { Effects::one(Effect::FocusChoose) }
}

//! The composer's mode, prompt or shell, as Desktop's composer machine keeps it
//! (`composer/suggestions/machine.ts`, `transitionComposer`):
//! - the draft becoming exactly `!` in prompt mode switches to shell and empties it
//!   (`inputChanged`);
//! - `mode.shell` (the + menu's Shell command, the palette, Ctrl+Shift+X) closes the
//!   suggestion lists and switches (`enterShell`); `mode.normal` (Exit, the palette) switches
//!   back (`leaveShell`);
//! - in shell mode with no list open, Escape, and Backspace in an empty draft, switch back
//!   (`keyDown`), whatever modifiers are held;
//! - a send, a revert or another draft's mode replaces it as it is (`mode.synced`).

/// What the composer sends: prompts, or shell commands (`ComposerMode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    #[default]
    Normal,
    Shell,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The draft's text changed by typing or pasting; `bang` when it is now exactly `!`.
    Input { bang: bool },
    /// `mode.shell`.
    Shell,
    /// `mode.normal`.
    Normal,
    /// Escape reached the composer with no suggestion list open.
    Escape,
    /// Backspace reached the composer with no suggestion list open; `empty` when the draft has
    /// no text.
    Backspace { empty: bool },
    /// Something else wrote the mode: a send cleared the draft, or another draft showed.
    Synced(Mode),
}

/// What the composer does for a step, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Close the + menu and the `@` and `/` lists.
    ClosePopovers,
    /// Empty the draft (the `!` that switched to shell).
    ClearDraft,
}

/// A step's effects, and whether it took the key that caused it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Step {
    pub effects: Vec<Effect>,
    pub handled: bool,
}

impl Step {
    fn none() -> Step {
        Step::default()
    }

    fn took(effects: Vec<Effect>) -> Step {
        Step { effects, handled: true }
    }
}

pub fn step(mode: &mut Mode, event: Event) -> Step {
    match (*mode, event) {
        (Mode::Normal, Event::Input { bang: true }) => {
            *mode = Mode::Shell;
            Step::took(vec![Effect::ClosePopovers, Effect::ClearDraft])
        }
        (_, Event::Input { .. }) => Step::none(),
        // Already in shell, it still closes what is open.
        (_, Event::Shell) => {
            *mode = Mode::Shell;
            Step { effects: vec![Effect::ClosePopovers], handled: false }
        }
        (_, Event::Normal) => {
            *mode = Mode::Normal;
            Step::none()
        }
        (Mode::Shell, Event::Escape | Event::Backspace { empty: true }) => {
            *mode = Mode::Normal;
            Step::took(Vec::new())
        }
        (Mode::Normal, Event::Escape) | (_, Event::Backspace { .. }) => Step::none(),
        (_, Event::Synced(next)) => {
            *mode = next;
            Step::none()
        }
    }
}

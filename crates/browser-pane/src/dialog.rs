//! A page's dialogs, as the app holds them for the user and the agent: Desktop's dialog gate
//! (`packages/gui-plugins/src/browser/dialog-machine.ts`) and its input protection
//! (`activation-delay.ts`).
//!
//! The engine holds one dialog at a time and the page waits on it. The person (in the pane) and
//! the agent (`browser_dialog`) answer the same dialog: the first answer of a dialog's id goes to
//! the engine, a later one finds it closed. A scripted dialog that opens within
//! [`REPEAT_DIALOG_MS`] of the last one closing offers "Stop this page from showing more
//! messages"; once chosen, the document's scripted dialogs are dismissed at once until the next
//! one (`Reset`). The pane's buttons ignore presses for [`ACTIVATION_DELAY_MS`] after a dialog
//! appears (`Guarded` until `Armed`), so a click already on its way can't answer it.

use std::time::Duration;

use rune::time::Instant;

use crate::protocol::{DialogKind, PageDialog};

/// Chrome offers to stop a page's dialogs when one opens this soon after the last one closed
/// (`dialog-machine.ts` `REPEAT_DIALOG_MS`).
pub const REPEAT_DIALOG_MS: u64 = 1_000;

/// How long a dialog's buttons ignore presses after it appears (`activation-delay.ts`).
pub const ACTIVATION_DELAY_MS: u64 = 500;

/// The longest page text a dialog shows (`ipc.ts` `PROMPT_TEXT_MAX`).
pub const PROMPT_TEXT_MAX: usize = 2_000;

/// Whether the pane's buttons take presses yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// Too soon after the dialog appeared; `until` arms them (`DialogEvent::Tick`).
    Guarded { until: Instant },
    Live,
}

/// A tab's dialogs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogGate {
    /// No dialog waits. `stopped`: the person stopped this document's scripted dialogs.
    /// `closed_at`: when the last scripted dialog closed, for the repeat offer.
    Idle { stopped: bool, closed_at: Option<Instant> },
    /// The page waits on `dialog`. `repeat`: it opened soon after the last one closed, so the
    /// pane offers to stop the page's dialogs.
    Open { dialog: PageDialog, repeat: bool, input: Input },
}

impl Default for DialogGate {
    fn default() -> Self {
        Self::Idle { stopped: false, closed_at: None }
    }
}

#[derive(Clone, Debug)]
pub enum DialogEvent {
    /// The engine holds a new dialog of the page.
    Opened { dialog: PageDialog, now: Instant },
    /// The person or the agent answers dialog `id`. `stop`: stop the document's dialogs too.
    Answer { id: u64, accept: bool, text: String, stop: bool, now: Instant },
    /// The page's held dialogs ended without an answer (a navigation), or the engine went away.
    Reset,
    /// Time passed: a guard whose time is up arms the buttons.
    Tick { now: Instant },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogEffect {
    /// Send this answer to the engine.
    Answer { id: u64, accept: bool, text: String },
}

fn scripted(kind: DialogKind) -> bool {
    kind != DialogKind::Leave
}

/// Page-supplied text as the pane shows it: without the controls that reorder or hide what follows,
/// and bounded (`prompt-machine.ts` `promptText`).
pub fn prompt_text(text: &str) -> String {
    text.chars()
        .filter(|&c| {
            let n = c as u32;
            !matches!(n, 0x00..=0x08 | 0x0b..=0x1f | 0x7f..=0x9f | 0x061c | 0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069)
        })
        .take(PROMPT_TEXT_MAX)
        .collect()
}

impl DialogGate {
    /// When the buttons of the open dialog arm, for the caller to `Tick` then.
    pub fn wake_at(&self) -> Option<Instant> {
        match self {
            Self::Open { input: Input::Guarded { until }, .. } => Some(*until),
            _ => None,
        }
    }

    /// The dialog the page waits on.
    pub fn open(&self) -> Option<&PageDialog> {
        match self {
            Self::Open { dialog, .. } => Some(dialog),
            Self::Idle { .. } => None,
        }
    }

    pub fn step(&mut self, event: DialogEvent) -> Vec<DialogEffect> {
        match (&*self, event) {
            (Self::Idle { stopped: true, .. }, DialogEvent::Opened { dialog, .. }) if scripted(dialog.kind) => {
                vec![DialogEffect::Answer { id: dialog.id, accept: false, text: String::new() }]
            }
            (state, DialogEvent::Opened { mut dialog, now }) => {
                let closed_at = match state {
                    Self::Idle { closed_at, .. } => *closed_at,
                    Self::Open { .. } => None,
                };
                let repeat = scripted(dialog.kind) && closed_at.is_some_and(|at| now.saturating_duration_since(at) <= Duration::from_millis(REPEAT_DIALOG_MS));
                dialog.message = prompt_text(&dialog.message);
                dialog.default_text = prompt_text(&dialog.default_text);
                let until = now + Duration::from_millis(ACTIVATION_DELAY_MS);
                *self = Self::Open { dialog, repeat, input: Input::Guarded { until } };
                Vec::new()
            }
            (Self::Open { dialog, repeat, .. }, DialogEvent::Answer { id, accept, text, stop, now }) if dialog.id == id => {
                let kind = dialog.kind;
                let text = if kind == DialogKind::Prompt && accept { text } else { String::new() };
                *self = Self::Idle { stopped: stop && *repeat, closed_at: scripted(kind).then_some(now) };
                vec![DialogEffect::Answer { id, accept, text }]
            }
            (_, DialogEvent::Answer { .. }) => Vec::new(),
            (_, DialogEvent::Reset) => {
                *self = Self::default();
                Vec::new()
            }
            (Self::Open { input: Input::Guarded { until }, .. }, DialogEvent::Tick { now }) => {
                if now >= *until
                    && let Self::Open { input, .. } = self
                {
                    *input = Input::Live;
                }
                Vec::new()
            }
            (_, DialogEvent::Tick { .. }) => Vec::new(),
        }
    }
}

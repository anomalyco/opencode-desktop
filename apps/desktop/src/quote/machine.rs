//! The Quote toolbar over a timeline selection and the composer's quote-comment editor, as
//! Desktop's `transitionQuote` (`packages/app/src/session/timeline/quote-machine.ts`) and
//! `transitionQuoteComment` (`packages/app/src/composer/quote-comment-machine.ts`) run them.
//!
//! A changed selection waits [`SETTLE`] without changing again before the toolbar reads it; the
//! wait keeps its attempt, so a timer for an earlier selection does nothing. An open toolbar
//! stays while a changed selection settles. Escape closes it for that selection, which stays
//! dismissed until the selection changes or clears. A selection being made with the pointer is
//! read once the pointer lets go.

use std::ops::Range;

use rune::TextEdit;
use rune::machine::{Attempt, Effects, Seq};

/// `QUOTE_SETTLE_MS`.
pub const SETTLE: f64 = 0.15;

/// Which texts a selection covers and where, to tell one selection from another (`Range`).
pub type Ident = Vec<(String, Range<usize>)>;

/// What the toolbar offers to quote (`ActiveQuote`).
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub session: String,
    pub text: String,
    pub truncated: bool,
    pub backward: bool,
}

/// `QuoteState`'s status.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Phase {
    #[default]
    Idle,
    Shown(Offer),
    /// Escape closed the toolbar over `Ident`.
    Dismissed(Ident),
}

/// A changed selection waiting to settle (`settle`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settle {
    pub attempt: Attempt,
    pub due: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Toolbar {
    pub phase: Phase,
    /// `pressing`: the pointer is making a selection; it's read on release.
    pub pressing: bool,
    pub settle: Option<Settle>,
}

pub enum Event<'a> {
    /// The timeline's selection changed (`selection.changed`), cleared (`None`), or is being
    /// made by a drag (`pointer.down` and its collapse).
    Changed { cleared: bool, dragging: bool, now: f64 },
    /// The settle timer came due (`settle.ready`).
    Due { now: f64 },
    /// The settled selection as read now (`quote.measured`): what to offer, if anything, and
    /// whether it is still the one Escape dismissed.
    Read { offer: Option<Offer>, ident: &'a Ident },
    /// Escape (`escape`).
    Escape { ident: Option<Ident> },
    /// The route, the composer's availability or a click took it away (`reset`).
    Reset,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    WakeAt(f64),
    /// Read the selection now (`quote.read`).
    Read,
}

pub fn step(seq: &mut Seq, t: &mut Toolbar, event: Event<'_>) -> Effects<Effect> {
    match event {
        Event::Changed { cleared: true, .. } | Event::Reset => {
            // A cleared selection closes at once, even mid-press.
            let pressing = t.pressing && !matches!(event, Event::Reset);
            *t = Toolbar { phase: Phase::Idle, pressing, settle: None };
            Effects::none()
        }
        Event::Changed { dragging: true, .. } => {
            // A press that makes a new selection collapses the old one first, which ends a
            // dismissal too; the drag is read once released.
            *t = Toolbar { phase: Phase::Idle, pressing: true, settle: None };
            Effects::none()
        }
        Event::Changed { now, .. } => {
            let settle = Settle { attempt: seq.next(), due: now + SETTLE };
            t.pressing = false;
            t.settle = Some(settle);
            Effects::one(Effect::WakeAt(settle.due))
        }
        Event::Due { now } => match t.settle {
            Some(s) if !t.pressing && now + 1e-3 >= s.due => {
                t.settle = None;
                Effects::one(Effect::Read)
            }
            _ => Effects::none(),
        },
        Event::Read { offer, ident } => {
            if matches!(&t.phase, Phase::Dismissed(d) if d == ident) {
                return Effects::none();
            }
            t.phase = offer.map_or(Phase::Idle, Phase::Shown);
            Effects::none()
        }
        Event::Escape { ident } => {
            if !matches!(t.phase, Phase::Shown(_)) {
                return Effects::none();
            }
            t.settle = None;
            t.phase = ident.map_or(Phase::Idle, Phase::Dismissed);
            Effects::none()
        }
    }
}

/// The quote whose comment editor is open, and the comment being written (`QuoteCommentState`'s
/// `closed` and `open`; the draft starts as the saved comment).
#[derive(Clone, Debug, Default)]
pub enum Comment {
    #[default]
    Closed,
    Open { key: u64, draft: TextEdit },
}

impl Comment {
    pub fn key(&self) -> Option<u64> {
        match self {
            Comment::Closed => None,
            Comment::Open { key, .. } => Some(*key),
        }
    }
}

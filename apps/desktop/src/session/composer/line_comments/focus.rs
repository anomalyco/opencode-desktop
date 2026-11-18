//! A session's comment focus hand-off and its active comment, as Desktop's
//! `transitionCommentFocus` (`app/src/composer/comment-focus-machine.ts`) runs it: a new comment,
//! or a card opened from the composer, asks the view that shows its file to reveal it; that view
//! acknowledges, or the request lapses after [`TIMEOUT`] so a view opened minutes later doesn't
//! jump to an old comment. `active` marks the card the composer shows opened.

use rune::machine::{Attempt, Effects, Seq};

/// `COMMENT_FOCUS_TIMEOUT_MS`.
pub const TIMEOUT: f64 = 5.0;

/// `CommentTarget`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub file: String,
    pub id: String,
}

/// `CommentFocusState` (a session's comments here live as long as the app, so no `disposed`).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Focus {
    #[default]
    Idle,
    /// Idle, with the card the composer marks opened.
    Active(Target),
    /// The view that shows `target` reveals it, until `due`.
    Pending { attempt: Attempt, due: f64, target: Target, active: Option<Target> },
}

pub enum Event {
    /// A view or a new comment asks for `target`; `None` is a view's acknowledgement
    /// (`focus.set`).
    Set(Option<Target>, f64),
    /// The user opened a comment from its card: reveal it and mark it active.
    Opened(Target, f64),
    Removed(Target),
    Reset,
    TimedOut { attempt: Attempt, now: f64 },
}

pub enum Effect {
    WakeAt(f64),
}

impl Focus {
    /// The comment a view should reveal now (`commentFocusTarget`).
    pub fn target(&self) -> Option<&Target> {
        match self {
            Focus::Pending { target, .. } => Some(target),
            _ => None,
        }
    }

    /// The card the composer marks opened (`commentActiveTarget`).
    pub fn active(&self) -> Option<&Target> {
        match self {
            Focus::Idle => None,
            Focus::Active(t) => Some(t),
            Focus::Pending { active, .. } => active.as_ref(),
        }
    }

    fn due(&self) -> Option<(Attempt, f64)> {
        match self {
            Focus::Pending { attempt, due, .. } => Some((*attempt, *due)),
            _ => None,
        }
    }

    /// When a pending request lapses, for the poll.
    pub fn deadline(&self) -> Option<(Attempt, f64)> {
        self.due()
    }
}

pub fn step(seq: &mut Seq, f: &mut Focus, event: Event) -> Effects<Effect> {
    match event {
        Event::Set(None, _) => settle(f, f.active().cloned()),
        Event::Set(Some(target), now) => {
            if f.target() == Some(&target) {
                return Effects::none();
            }
            let active = f.active().cloned();
            request(seq, f, target, active, now)
        }
        Event::Opened(target, now) => {
            if f.target() == Some(&target) {
                if let Focus::Pending { active, .. } = f {
                    *active = Some(target);
                }
                return Effects::none();
            }
            request(seq, f, target.clone(), Some(target), now)
        }

        Event::Removed(target) => {
            if f.target() != Some(&target) {
                return Effects::none();
            }
            settle(f, f.active().cloned())
        }
        Event::Reset => settle(f, None),
        Event::TimedOut { attempt, now } => {
            if !f.due().is_some_and(|(a, due)| a == attempt && now + 1e-3 >= due) {
                return Effects::none();
            }
            settle(f, f.active().cloned())
        }
    }
}

fn request(seq: &mut Seq, f: &mut Focus, target: Target, active: Option<Target>, now: f64) -> Effects<Effect> {
    let due = now + TIMEOUT;
    *f = Focus::Pending { attempt: seq.next(), due, target, active };
    Effects::one(Effect::WakeAt(due))
}

fn settle(f: &mut Focus, active: Option<Target>) -> Effects<Effect> {
    *f = active.map_or(Focus::Idle, Focus::Active);
    Effects::none()
}

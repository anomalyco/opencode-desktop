//! A question form's reply: the request part of Desktop's `QuestionDockState` and
//! `transitionQuestionDock` (`session/requests/question-machine.ts:31-246`). Submitting or
//! dismissing sends once; the dock stays, its answers and buttons off, until the server settles
//! the form (`form.replied`, `form.cancelled`), which drops it with its answers. A failure or the
//! 30 s timeout gives the dock back as it was, with a toast. The answers themselves stay in
//! `session_view::dock::question::Dock`, which outlives the dock as Desktop's `questionCache` does.

use rune::machine::{Attempt, Effects};
use serde_json::Value;

/// `QUESTION_REQUEST_TIMEOUT_MS`.
pub const TIMEOUT_S: f64 = 30.0;

/// `QuestionSendAction`.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Reply(Value),
    Dismiss,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Answering { attempt: Attempt },
    Sending { attempt: Attempt, action: Action },
    /// The server took the reply; the form stays busy until the server settles it.
    Replied { attempt: Attempt },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event<'a> {
    Send(Action),
    Succeeded { attempt: Attempt },
    Failed { attempt: Attempt, error: &'a str },
    TimedOut { attempt: Attempt },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// `request.send`: sends `action` and starts its timeout.
    Send { attempt: Attempt, action: Action },
    /// `request.abort` of a reply still in flight: its answer is dropped when it comes.
    Abort,
    /// `toast.fail`.
    ToastFailed(Option<String>),
}

/// `transitionQuestionDock`, for the reply.
pub fn step(state: &mut Reply, event: Event<'_>) -> Effects<Effect> {
    match (&*state, event) {
        // The request leaves before anything else hears of the submit.
        (Reply::Answering { attempt }, Event::Send(action)) => {
            let attempt = Attempt(attempt.0 + 1);
            *state = Reply::Sending { attempt, action: action.clone() };
            Effects::one(Effect::Send { attempt, action })
        }
        (Reply::Sending { attempt, .. }, Event::Succeeded { attempt: due }) if *attempt == due => {
            *state = Reply::Replied { attempt: due };
            Effects::none()
        }
        (Reply::Sending { attempt, .. }, Event::Failed { attempt: due, error }) if *attempt == due => {
            *state = Reply::Answering { attempt: due };
            Effects::one(Effect::ToastFailed(Some(error.to_owned())))
        }
        (Reply::Sending { attempt, .. }, Event::TimedOut { attempt: due }) if *attempt == due => {
            *state = Reply::Answering { attempt: due };
            Effects::two(Effect::Abort, Effect::ToastFailed(None))
        }
        _ => Effects::none(),
    }
}

/// `isQuestionBusy`.
pub fn busy(state: &Reply) -> bool {
    !matches!(state, Reply::Answering { .. })
}

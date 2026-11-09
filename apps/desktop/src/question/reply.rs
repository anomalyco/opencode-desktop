//! A question form's reply: the request part of Desktop's `QuestionDockState` and
//! `transitionQuestionDock` (`session/requests/question-machine.ts:31-246`). Submitting or
//! dismissing sends once; the dock stays, its answers and buttons off, until the server settles
//! the form (`form.replied`, `form.cancelled`), which drops it with its answers. A failure or the
//! 30 s timeout gives the dock back as it was, with a toast. The answers themselves stay in
//! `session_view::dock::question::Dock`, which outlives the dock as Desktop's `questionCache` does.

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
    Answering { attempt: u32 },
    Sending { attempt: u32, action: Action },
    /// The server took the reply; the form stays busy until the server settles it.
    Replied { attempt: u32 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event<'a> {
    Send(Action),
    Succeeded { attempt: u32 },
    Failed { attempt: u32, error: &'a str },
    TimedOut { attempt: u32 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// `request.send`: sends `action` and starts its timeout.
    Send { attempt: u32, action: Action },
    /// `request.abort` of a reply still in flight: its answer is dropped when it comes.
    Abort,
    /// `toast.fail`.
    ToastFailed(Option<String>),
}

/// `transitionQuestionDock`, for the reply.
pub fn step(state: &mut Reply, event: Event<'_>) -> Vec<Effect> {
    match (&*state, event) {
        // The request leaves before anything else hears of the submit.
        (Reply::Answering { attempt }, Event::Send(action)) => {
            let attempt = attempt + 1;
            *state = Reply::Sending { attempt, action: action.clone() };
            vec![Effect::Send { attempt, action }]
        }
        (Reply::Sending { attempt, .. }, Event::Succeeded { attempt: due }) if *attempt == due => {
            *state = Reply::Replied { attempt: due };
            vec![]
        }
        (Reply::Sending { attempt, .. }, Event::Failed { attempt: due, error }) if *attempt == due => {
            *state = Reply::Answering { attempt: due };
            vec![Effect::ToastFailed(Some(error.to_owned()))]
        }
        (Reply::Sending { attempt, .. }, Event::TimedOut { attempt: due }) if *attempt == due => {
            *state = Reply::Answering { attempt: due };
            vec![Effect::Abort, Effect::ToastFailed(None)]
        }
        _ => vec![],
    }
}

/// `isQuestionBusy`.
pub fn busy(state: &Reply) -> bool {
    !matches!(state, Reply::Answering { .. })
}

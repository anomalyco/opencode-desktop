//! One permission request's reply from the dock: Desktop's `PermissionReplyState` and
//! `transitionPermissionReply` (`session/requests/permission-machine.ts`). The first press
//! claims the request; it stays on the dock, its buttons off, until the server takes the reply,
//! then leaves at once without waiting for `permission.replied`. A failure or the 15 s timeout
//! gives the buttons back with a toast. `permission.replied` (Desktop's `resolved`) drops the
//! request and its reply together (`App::forget`).

/// `PERMISSION_REPLY_TIMEOUT_MS`.
pub const TIMEOUT_S: f64 = 15.0;

use rune::machine::{Attempt, Effects};

#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Idle { attempt: Attempt },
    Responding { attempt: Attempt, session: String, decision: String },
    /// Answered: it never comes back before `permission.replied` drops it.
    Replied,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event<'a> {
    Decide { session: &'a str, decision: &'a str },
    Succeeded { attempt: Attempt },
    Failed { attempt: Attempt, error: &'a str },
    TimedOut { attempt: Attempt },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Sends the reply and starts its timeout.
    Send { attempt: Attempt, session: String, decision: String },
    /// Calls off a reply still in flight: Rune can't stop a write the worker has taken, so its
    /// answer is dropped when it comes. (Desktop also aborts after an answer, to clear the
    /// timeout; the attempt fences that timeout here.)
    Abort,
    ToastFailed(Option<String>),
}

/// `transitionPermissionReply`.
pub fn step(state: &mut Reply, event: Event<'_>) -> Effects<Effect> {
    if let Event::Decide { session, decision } = event {
        let Reply::Idle { attempt } = state else { return Effects::none() };
        let attempt = Attempt(attempt.0 + 1);
        *state = Reply::Responding { attempt, session: session.to_owned(), decision: decision.to_owned() };
        return Effects::one(Effect::Send { attempt, session: session.to_owned(), decision: decision.to_owned() });
    }
    let Reply::Responding { attempt, .. } = *state else { return Effects::none() };
    let due = match event {
        Event::Succeeded { attempt } | Event::Failed { attempt, .. } | Event::TimedOut { attempt } => attempt,
        Event::Decide { .. } => return Effects::none(),
    };
    if due != attempt {
        return Effects::none();
    }
    match event {
        Event::Succeeded { .. } => {
            *state = Reply::Replied;
            Effects::none()
        }
        Event::Failed { error, .. } => {
            *state = Reply::Idle { attempt };
            Effects::one(Effect::ToastFailed(Some(error.to_owned())))
        }
        _ => {
            *state = Reply::Idle { attempt };
            Effects::two(Effect::Abort, Effect::ToastFailed(None))
        }
    }
}

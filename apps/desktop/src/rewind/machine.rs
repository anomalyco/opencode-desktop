//! One session's rewind, undo and redo, after Desktop's `session/revert-machine.ts`: one
//! operation at a time, each with its attempt, so a late result or a click on an older Rewound
//! toast changes nothing; a 30 s bound (`REVERT_TIMEOUT_MS`) so a hung request can't leave undo
//! and redo disabled; and the boundaries this window's requests will echo on the event stream,
//! in send order, so a late echo of an earlier request isn't taken for a change made elsewhere.
//!
//! Rune shows a rewind on the press (the rows leave, the prompt comes back to the composer, the
//! toast offers Undo) where Desktop waits for the server, so each operation also keeps what a
//! failure puts back (`Rollback`).

use rune::machine::{Attempt, Effects, Seq};

use super::SavedDraft;

/// What a failed or timed-out operation puts back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rollback {
    pub revert: Option<String>,
    /// Runs the rewind stopped, which show again.
    pub stopped: Vec<String>,
    pub draft: Option<SavedDraft>,
    /// The Rewound toast it showed.
    pub toast: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    /// An undelivered prompt has no history to rewind, so it's withdrawn from the inbox instead.
    /// `draft` is its words as the composer takes them, read when it was asked for: the inbox
    /// may drop the prompt before the cancel's answer lands.
    Withdraw { message: String, draft: SavedDraft },
    Rewind { message: String, prior: Option<String>, draft: SavedDraft, rollback: Rollback },
    Redo { rollback: Rollback },
    /// The Rewound toast's Undo: stage the earlier boundary and put the draft from before the
    /// rewind back.
    UndoRewind { prior: Option<String>, rollback: Rollback },
}

impl Operation {
    /// The boundary its request echoes on the stream (`None`: a clear); a withdrawal echoes none.
    fn echo(&self) -> Option<Option<String>> {
        match self {
            Operation::Withdraw { .. } => None,
            Operation::Rewind { message, .. } => Some(Some(message.clone())),
            Operation::Redo { .. } => Some(None),
            Operation::UndoRewind { prior, .. } => Some(prior.clone()),
        }
    }

    fn rollback(&self) -> Option<&Rollback> {
        match self {
            Operation::Withdraw { .. } => None,
            Operation::Rewind { rollback, .. } | Operation::Redo { rollback } | Operation::UndoRewind { rollback, .. } => Some(rollback),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    #[default]
    Idle,
    Running { attempt: Attempt, operation: Operation, due: f64 },
    /// A rewind landed and its toast can undo it.
    Restored { attempt: Attempt, boundary: String, prior: Option<String>, draft: SavedDraft },
}

#[derive(Clone, Debug, Default)]
pub struct SessionRevert {
    pub state: State,
    /// The boundaries this window's requests will echo, oldest first.
    echoes: Vec<Option<String>>,
    seq: Seq,
}

pub enum Event {
    Requested { operation: Operation, now: f64 },
    /// The Rewound toast's Undo, for the rewind of `attempt`.
    ToastUndo { attempt: Attempt, now: f64, rollback: Rollback },
    Succeeded { attempt: Attempt },
    Failed { attempt: Attempt, why: String },
    Due { now: f64 },
    /// The stream: a boundary staged (`Some`) or cleared (`None`).
    Reported(Option<String>),
    /// The stream: a sent prompt committed the revert.
    Committed,
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    /// Send the operation's request under its attempt.
    Run { attempt: Attempt, operation: Operation },
    /// Put back what a failed or timed-out operation changed (nothing for a withdrawal), and say so.
    Undo { rollback: Option<Rollback>, why: String },
    /// A withdrawn prompt's words go into the composer.
    RestoreMessage { message: String, draft: SavedDraft },
    /// The draft from before the rewind comes back (the toast's Undo).
    RestoreDraft { draft: SavedDraft },
    /// The Rewound toast of this session goes: a newer boundary replaced its rewind.
    DismissToast,
}

/// `REVERT_TIMEOUT_MS`.
pub const TIMEOUT_SECS: f64 = 30.0;

impl SessionRevert {
    pub fn running(&self) -> bool {
        matches!(self.state, State::Running { .. })
    }

    /// The attempt the next request will run under.
    pub fn next_attempt(&self) -> Attempt {
        let mut seq = self.seq;
        seq.next()
    }

    /// The boundary to show while this window's requests are out: the last one's.
    pub fn expected(&self) -> Option<&Option<String>> {
        self.echoes.last()
    }

    /// When a running operation times out.
    pub fn due(&self) -> Option<f64> {
        match self.state {
            State::Running { due, .. } => Some(due),
            _ => None,
        }
    }

    pub fn step(&mut self, event: Event) -> Effects<Effect> {
        match event {
            Event::Requested { operation, now } => {
                if self.running() {
                    return Effects::none();
                }
                self.start(operation, now)
            }
            // Rune's toast shows on the press, so its Undo can come before the rewind's answer:
            // the undo's request follows the rewind's on the one writer, and the rewind's late
            // answer belongs to an attempt no longer current.
            Event::ToastUndo { attempt, now, rollback } => {
                let (prior, draft) = match &self.state {
                    State::Restored { attempt: a, prior, draft, .. } if *a == attempt => (prior.clone(), draft.clone()),
                    State::Running { attempt: a, operation: Operation::Rewind { prior, draft, .. }, .. } if *a == attempt => (prior.clone(), draft.clone()),
                    _ => return Effects::none(),
                };
                let mut effects = self.start(Operation::UndoRewind { prior, rollback }, now);
                effects.push(Effect::RestoreDraft { draft });
                effects
            }
            Event::Succeeded { attempt } => {
                let State::Running { attempt: a, operation, .. } = &self.state else { return Effects::none() };
                if *a != attempt {
                    return Effects::none();
                }
                match operation.clone() {
                    Operation::Withdraw { message, draft } => {
                        self.state = State::Idle;
                        Effects::one(Effect::RestoreMessage { message, draft })
                    }
                    Operation::Rewind { message, prior, draft, .. } => {
                        self.state = State::Restored { attempt, boundary: message, prior, draft };
                        Effects::none()
                    }
                    Operation::Redo { .. } | Operation::UndoRewind { .. } => {
                        self.state = State::Idle;
                        Effects::none()
                    }
                }
            }
            Event::Failed { attempt, why } => self.fail(attempt, why),
            Event::Due { now } => match &self.state {
                State::Running { attempt, due, .. } if now + 1e-9 >= *due => self.fail(*attempt, String::new()),
                _ => Effects::none(),
            },
            Event::Reported(boundary) => {
                if let Some(at) = self.echoes.iter().position(|e| *e == boundary) {
                    // The stream reported a request this window sent: a restored rewind keeps its toast.
                    self.echoes.drain(..=at);
                    return Effects::none();
                }
                // A change from elsewhere: the stream is the truth again.
                self.restored_replaced()
            }
            Event::Committed => {
                // A commit lands after every request already sent, but a running operation may
                // still be interrupting or waiting, so its own echo stays due.
                let keep = matches!(&self.state, State::Running { operation, .. } if !matches!(operation, Operation::Withdraw { .. }));
                let last = self.echoes.pop();
                self.echoes.clear();
                if keep && let Some(last) = last {
                    self.echoes.push(last);
                }
                self.restored_replaced()
            }
        }
    }

    // Claims the session synchronously, so a second request before the first settles is refused.
    fn start(&mut self, operation: Operation, now: f64) -> Effects<Effect> {
        let attempt = self.seq.next();
        let restored = matches!(self.state, State::Restored { .. });
        // The echo is expected from the moment the request is sent: the stream can report it
        // before the response lands.
        if let Some(echo) = operation.echo() {
            self.echoes.push(echo);
        }
        let due = now + TIMEOUT_SECS;
        self.state = State::Running { attempt, operation: operation.clone(), due };
        // `poll` wakes for `due` (`App::revert_timeouts`).
        let mut effects = Effects::one(Effect::Run { attempt, operation });
        if restored {
            effects.push(Effect::DismissToast);
        }
        effects
    }

    fn fail(&mut self, attempt: Attempt, why: String) -> Effects<Effect> {
        let State::Running { attempt: a, operation, .. } = &self.state else { return Effects::none() };
        if *a != attempt {
            return Effects::none();
        }
        // A request that failed sends no echo.
        if let Some(echo) = operation.echo()
            && let Some(at) = self.echoes.iter().rposition(|e| *e == echo)
        {
            self.echoes.remove(at);
        }
        let rollback = operation.rollback().cloned();
        self.state = State::Idle;
        Effects::one(Effect::Undo { rollback, why })
    }

    // A newer boundary replaced a restored rewind, so its Undo would put back a stale draft.
    fn restored_replaced(&mut self) -> Effects<Effect> {
        if matches!(self.state, State::Restored { .. }) {
            self.state = State::Idle;
            return Effects::one(Effect::DismissToast);
        }
        Effects::none()
    }
}

//! One session's rewind, undo and redo, after Desktop's `session/revert-machine.ts`: one
//! operation at a time, each with its attempt, so a late result or a click on an older Rewound
//! toast changes nothing; a 30 s bound (`REVERT_TIMEOUT_MS`) so a hung request can't leave undo
//! and redo disabled; and the boundaries this window's requests will echo on the event stream,
//! in send order, so a late echo of an earlier request isn't taken for a change made elsewhere.
//!
//! Rune shows a rewind on the press (the rows leave, the prompt comes back to the composer, the
//! toast offers Undo) where Desktop waits for the server, so each operation also keeps what a
//! failure puts back (`Rollback`).
//!
//! A failed answer doesn't always mean the server didn't act: the request can reach it and the
//! answer be lost, or it can land after the app gave up. So the server's word decides. A rewind
//! whose boundary the stream reported before its answer failed landed, and stays. One that
//! failed unreported is rolled back, but stays `Doubtful` for `DOUBT_SECS`: if the server then
//! reports that boundary (the stream, or a fresh read after a reconnect), the rewind landed after
//! all and its prompt comes back. A withdrawal whose cancel failed waits the same way for the
//! prompt to leave the inbox undelivered.

use rune::machine::{Attempt, Effects, Seq};

use crate::draft::SavedDraft;

/// What a failed or timed-out operation puts back.
#[derive(Clone, Debug, PartialEq)]
pub struct Rollback<D = SavedDraft> {
    pub revert: Option<String>,
    /// Runs the rewind stopped, which show again.
    pub stopped: Vec<String>,
    pub draft: Option<D>,
    /// The Rewound toast it showed.
    pub toast: Option<u64>,
}

impl<D> Default for Rollback<D> {
    fn default() -> Self {
        Self { revert: None, stopped: Vec::new(), draft: None, toast: None }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Operation<D = SavedDraft> {
    /// An undelivered prompt has no history to rewind, so it's withdrawn from the inbox instead.
    /// `draft` is its words as the composer takes them, read when it was asked for: the inbox
    /// may drop the prompt before the cancel's answer lands.
    Withdraw { message: String, draft: D },
    /// `draft` is the composer's before the press, `restored` the rewound prompt's words.
    Rewind { message: String, prior: Option<String>, draft: D, restored: D, rollback: Rollback<D> },
    Redo { rollback: Rollback<D> },
    /// The Rewound toast's Undo: stage the earlier boundary and put the draft from before the
    /// rewind back.
    UndoRewind { prior: Option<String>, rollback: Rollback<D> },
}

impl<D> Operation<D> {
    /// The boundary its request echoes on the stream (`None`: a clear); a withdrawal echoes none.
    fn echo(&self) -> Option<Option<String>> {
        match self {
            Operation::Withdraw { .. } => None,
            Operation::Rewind { message, .. } => Some(Some(message.clone())),
            Operation::Redo { .. } => Some(None),
            Operation::UndoRewind { prior, .. } => Some(prior.clone()),
        }
    }

    fn rollback(&self) -> Option<&Rollback<D>> {
        match self {
            Operation::Withdraw { .. } => None,
            Operation::Rewind { rollback, .. } | Operation::Redo { rollback } | Operation::UndoRewind { rollback, .. } => Some(rollback),
        }
    }
}

/// An operation whose request failed but which the server may still have taken.
#[derive(Clone, Debug, PartialEq)]
pub enum Doubt<D = SavedDraft> {
    /// A rewind to `boundary`, rolled back: `draft` is what the rollback put in the composer.
    Rewind { boundary: String, prior: Option<String>, draft: D, restored: D },
    /// A withdrawal of `message`, whose words `draft` are.
    Withdraw { message: String, draft: D },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum State<D = SavedDraft> {
    #[default]
    Idle,
    /// `echoed`: the stream already reported the boundary its request asks for.
    Running { attempt: Attempt, operation: Operation<D>, due: f64, echoed: bool },
    /// A rewind landed and its toast can undo it.
    Restored { attempt: Attempt, boundary: String, prior: Option<String>, draft: D },
    /// A failed request the server may still act on, until `due`.
    Doubtful { attempt: Attempt, doubt: Doubt<D>, due: f64 },
}

#[derive(Clone, Debug)]
pub struct SessionRevert<D = SavedDraft> {
    pub state: State<D>,
    /// The boundaries this window's requests will echo, oldest first.
    echoes: Vec<Option<String>>,
    seq: Seq,
}

impl<D> Default for SessionRevert<D> {
    fn default() -> Self {
        Self { state: State::Idle, echoes: Vec::new(), seq: Seq::default() }
    }
}

pub enum Event<D = SavedDraft> {
    Requested { operation: Operation<D>, now: f64 },
    /// The Rewound toast's Undo, for the rewind of `attempt`.
    ToastUndo { attempt: Attempt, now: f64, rollback: Rollback<D> },
    Succeeded { attempt: Attempt },
    Failed { attempt: Attempt, why: String, now: f64 },
    Due { now: f64 },
    /// The stream: a boundary staged (`Some`) or cleared (`None`).
    Reported(Option<String>),
    /// A fresh read of the session after a reconnect: its boundary as the server holds it.
    Read(Option<String>),
    /// The stream: a sent prompt committed the revert.
    Committed,
    /// Prompt `message` left the inbox without being delivered (cancelled).
    Withdrawn { message: String },
}

#[derive(Debug, PartialEq)]
pub enum Effect<D = SavedDraft> {
    /// Send the operation's request under its attempt.
    Run { attempt: Attempt, operation: Operation<D> },
    /// Put back what a failed or timed-out operation changed (nothing for a withdrawal), and say so.
    Undo { rollback: Option<Rollback<D>>, why: String },
    /// A withdrawn prompt's words go into the composer.
    RestoreMessage { message: String, draft: D },
    /// The draft from before the rewind comes back (the toast's Undo).
    RestoreDraft { draft: D },
    /// The Rewound toast of this session goes: a newer boundary replaced its rewind.
    DismissToast,
    /// A rolled-back rewind landed after all: its boundary shows, and its prompt replaces the
    /// draft the rollback put back, if that's still what the composer holds; its toast shows again.
    Land { attempt: Attempt, boundary: String, rolled_back: D, restored: D },
}

/// `REVERT_TIMEOUT_MS`.
pub const TIMEOUT_SECS: f64 = 30.0;
/// How long a failed request's effect may still land: past the 15 s a write waits for its
/// answer and the 30 s bound, with room for a reconnect's read.
pub const DOUBT_SECS: f64 = 60.0;

impl<D> SessionRevert<D> {
    pub fn running(&self) -> bool {
        matches!(self.state, State::Running { .. })
    }

    /// Whether a failed request may still land, which a fresh read of the session can settle.
    pub fn doubtful(&self) -> bool {
        matches!(self.state, State::Doubtful { .. })
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

    /// When a running operation times out, or a doubt ends.
    pub fn due(&self) -> Option<f64> {
        match self.state {
            State::Running { due, .. } | State::Doubtful { due, .. } => Some(due),
            _ => None,
        }
    }
}

impl<D: Clone> SessionRevert<D> {
    pub fn step(&mut self, event: Event<D>) -> Effects<Effect<D>> {
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
            Event::Succeeded { attempt } => self.succeed(attempt),
            Event::Failed { attempt, why, now } => self.fail(attempt, why, now),
            Event::Due { now } => match &self.state {
                State::Running { attempt, due, .. } if now + 1e-9 >= *due => self.fail(*attempt, String::new(), now),
                State::Doubtful { due, .. } if now + 1e-9 >= *due => {
                    self.state = State::Idle;
                    Effects::none()
                }
                _ => Effects::none(),
            },
            Event::Reported(boundary) => {
                if let Some(effects) = self.landed_late(&boundary) {
                    return effects;
                }
                if let State::Running { operation, echoed, .. } = &mut self.state
                    && operation.echo().as_ref() == Some(&boundary)
                {
                    *echoed = true;
                }
                if let Some(at) = self.echoes.iter().position(|e| *e == boundary) {
                    // The stream reported a request this window sent: a restored rewind keeps its toast.
                    self.echoes.drain(..=at);
                    return Effects::none();
                }
                // A change from elsewhere: the stream is the truth again.
                self.restored_replaced()
            }
            Event::Read(boundary) => self.landed_late(&boundary).unwrap_or_default(),
            Event::Committed => {
                // A commit lands after every request already sent, but a running operation may
                // still be interrupting or waiting, so its own echo stays due.
                let keep = matches!(&self.state, State::Running { operation, .. } if !matches!(operation, Operation::Withdraw { .. }));
                let last = self.echoes.pop();
                self.echoes.clear();
                if keep && let Some(last) = last {
                    self.echoes.push(last);
                }
                if matches!(self.state, State::Doubtful { doubt: Doubt::Rewind { .. }, .. }) {
                    self.state = State::Idle;
                }
                self.restored_replaced()
            }
            Event::Withdrawn { message } => match &self.state {
                State::Doubtful { doubt: Doubt::Withdraw { message: m, draft }, .. } if *m == message => {
                    let draft = draft.clone();
                    self.state = State::Idle;
                    Effects::one(Effect::RestoreMessage { message, draft })
                }
                _ => Effects::none(),
            },
        }
    }

    /// The server reports the boundary a rolled-back rewind asked for: it landed after all.
    fn landed_late(&mut self, boundary: &Option<String>) -> Option<Effects<Effect<D>>> {
        let State::Doubtful { attempt, doubt: Doubt::Rewind { boundary: b, prior, draft, restored }, .. } = &self.state else {
            return None;
        };
        if boundary.as_deref() != Some(b.as_str()) {
            return None;
        }
        let (attempt, boundary, rolled_back, restored) = (*attempt, b.clone(), draft.clone(), restored.clone());
        self.state = State::Restored { attempt, boundary: boundary.clone(), prior: prior.clone(), draft: rolled_back.clone() };
        Some(Effects::one(Effect::Land { attempt, boundary, rolled_back, restored }))
    }

    // Claims the session synchronously, so a second request before the first settles is refused.
    fn start(&mut self, operation: Operation<D>, now: f64) -> Effects<Effect<D>> {
        let attempt = self.seq.next();
        let restored = matches!(self.state, State::Restored { .. });
        // The echo is expected from the moment the request is sent: the stream can report it
        // before the response lands.
        if let Some(echo) = operation.echo() {
            self.echoes.push(echo);
        }
        let due = now + TIMEOUT_SECS;
        self.state = State::Running { attempt, operation: operation.clone(), due, echoed: false };
        // `poll` wakes for `due` (`App::revert_timeouts`).
        let mut effects = Effects::one(Effect::Run { attempt, operation });
        if restored {
            effects.push(Effect::DismissToast);
        }
        effects
    }

    fn succeed(&mut self, attempt: Attempt) -> Effects<Effect<D>> {
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

    fn fail(&mut self, attempt: Attempt, why: String, now: f64) -> Effects<Effect<D>> {
        let State::Running { attempt: a, operation, echoed, .. } = &self.state else { return Effects::none() };
        if *a != attempt {
            return Effects::none();
        }
        // The stream already said the server holds the boundary this asked for: the answer was
        // lost, not the change.
        if *echoed {
            return self.succeed(attempt);
        }
        // A request that failed sends no echo.
        let operation = operation.clone();
        if let Some(echo) = operation.echo()
            && let Some(at) = self.echoes.iter().rposition(|e| *e == echo)
        {
            self.echoes.remove(at);
        }
        let rollback = operation.rollback().cloned();
        let due = now + DOUBT_SECS;
        self.state = match operation {
            Operation::Rewind { message, prior, draft, restored, .. } => {
                State::Doubtful { attempt, doubt: Doubt::Rewind { boundary: message, prior, draft, restored }, due }
            }
            Operation::Withdraw { message, draft } => State::Doubtful { attempt, doubt: Doubt::Withdraw { message, draft }, due },
            Operation::Redo { .. } | Operation::UndoRewind { .. } => State::Idle,
        };
        Effects::one(Effect::Undo { rollback, why })
    }

    // A newer boundary replaced a restored rewind, so its Undo would put back a stale draft.
    fn restored_replaced(&mut self) -> Effects<Effect<D>> {
        if matches!(self.state, State::Restored { .. }) {
            self.state = State::Idle;
            return Effects::one(Effect::DismissToast);
        }
        Effects::none()
    }
}

//! One session's queue changes, after Desktop's `session/composer/queue-machine.ts`: they run
//! one at a time, in the order they were asked for (`active`, then `pending`), each with its
//! attempt. A steered or removed row leaves the strip at once and comes back if the server
//! refuses; an undone row stays, pending, until the server lets the prompt go, and only then do
//! its words go into the composer, so a prompt is never both queued and back in the composer.
//! Rows the server confirmed gone stay hidden until its events catch up (`gone`), each for at
//! most `GONE_SECS`.
//!
//! Desktop's queue also edits a queued prompt in the composer and reorders the strip by drag;
//! Rune's has neither yet.

use rune::machine::{Attempt, Effects, Seq};

/// `QUEUE_MUTATION_TIMEOUT_MS`, for an undo (a steer or removal is one request, bounded by the
/// request itself).
pub const UNDO_TIMEOUT_SECS: f64 = 60.0;
/// How long a confirmed row stays hidden waiting for the server's event.
pub const GONE_SECS: f64 = 10.0;

#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    /// A queued prompt steers now (Send once the turn stopped).
    Steer { id: String },
    /// A queued prompt goes.
    Remove { id: String },
    /// A queued prompt comes back to the composer, after what's typed there.
    Undo { id: String, text: String, files: Vec<opencode_kit::model::Attachment> },
    /// A pending steer moves to the queue (its menu's Move to queue).
    Requeue { id: String },
    /// A pending steer goes (its menu's Delete).
    Delete { id: String },
}

impl Operation {
    pub fn id(&self) -> &str {
        match self {
            Operation::Steer { id } | Operation::Remove { id } | Operation::Undo { id, .. } | Operation::Requeue { id } | Operation::Delete { id } => id,
        }
    }

    /// The strip hides its row while it runs or waits.
    fn hides(&self) -> bool {
        matches!(self, Operation::Steer { .. } | Operation::Remove { .. })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    #[default]
    Idle,
    Mutating { attempt: Attempt, active: Operation, pending: Vec<Operation>, due: Option<f64> },
}

#[derive(Clone, Debug, Default)]
pub struct Queue {
    pub state: State,
    /// Rows the server confirmed gone, until `expiry`.
    gone: Vec<(String, f64)>,
    seq: Seq,
}

pub enum Event {
    Requested { operation: Operation, now: f64 },
    Succeeded { attempt: Attempt, now: f64 },
    Failed { attempt: Attempt, now: f64 },
    /// The clock: expired gone rows show again, and an undo past its bound fails.
    Tick { now: f64 },
    /// The server's queue as listed now: gone rows it no longer holds stop being tracked.
    Synced { ids: Vec<String> },
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    Run { attempt: Attempt, operation: Operation },
    /// The undone prompt's words join the composer's.
    Merge { text: String, files: Vec<opencode_kit::model::Attachment> },
    /// "Request failed".
    Failed,
}

impl Queue {
    /// Whether a change runs: the strip's actions wait.
    pub fn busy(&self) -> bool {
        matches!(self.state, State::Mutating { .. })
    }

    /// Whether the strip leaves row `id` out.
    pub fn hidden(&self, id: &str) -> bool {
        self.gone.iter().any(|(g, _)| g == id) || self.operations().any(|o| o.hides() && o.id() == id)
    }

    /// Whether row `id` is being undone, which shows it pending.
    pub fn undoing(&self, id: &str) -> bool {
        matches!(&self.state, State::Mutating { active: Operation::Undo { id: undo, .. }, .. } if undo == id)
    }

    /// Whether `id` has a change running or waiting, which its menu's actions wait for.
    pub fn acting(&self, id: &str) -> bool {
        self.operations().any(|o| o.id() == id)
    }

    /// When the next timer is due: an undo's bound or a gone row's expiry.
    pub fn due(&self) -> Option<f64> {
        let undo = match self.state {
            State::Mutating { due, .. } => due,
            State::Idle => None,
        };
        self.gone.iter().map(|(_, e)| *e).chain(undo).reduce(f64::min)
    }

    fn operations(&self) -> impl Iterator<Item = &Operation> {
        let (active, pending) = match &self.state {
            State::Mutating { active, pending, .. } => (Some(active), pending.as_slice()),
            State::Idle => (None, &[][..]),
        };
        active.into_iter().chain(pending)
    }

    pub fn step(&mut self, event: Event) -> Effects<Effect> {
        match event {
            Event::Requested { operation, now } => {
                // A row already changing ignores another press.
                if self.acting(operation.id()) || self.gone.iter().any(|(g, _)| g == operation.id()) {
                    return Effects::none();
                }
                // An undo waits for nothing else and nothing waits for it (`undo` needs `idle`).
                if matches!(operation, Operation::Undo { .. }) && self.busy() {
                    return Effects::none();
                }
                match &mut self.state {
                    State::Mutating { pending, .. } => {
                        pending.push(operation);
                        Effects::none()
                    }
                    State::Idle => self.start(operation, now),
                }
            }
            Event::Succeeded { attempt, now } => {
                let State::Mutating { attempt: a, active, .. } = &self.state else { return Effects::none() };
                if *a != attempt {
                    return Effects::none();
                }
                let active = active.clone();
                self.gone.push((active.id().to_owned(), now + GONE_SECS));
                let mut effects = self.advance(now);
                if let Operation::Undo { text, files, .. } = active {
                    effects.push(Effect::Merge { text, files });
                }
                effects
            }
            Event::Failed { attempt, now } => {
                let State::Mutating { attempt: a, .. } = &self.state else { return Effects::none() };
                if *a != attempt {
                    return Effects::none();
                }
                let mut effects = Effects::one(Effect::Failed);
                for e in self.advance(now) {
                    effects.push(e);
                }
                effects
            }
            Event::Tick { now } => {
                self.gone.retain(|(_, expiry)| *expiry > now);
                match &self.state {
                    State::Mutating { attempt, due: Some(due), .. } if now + 1e-9 >= *due => {
                        let attempt = *attempt;
                        self.step(Event::Failed { attempt, now })
                    }
                    _ => Effects::none(),
                }
            }
            Event::Synced { ids } => {
                self.gone.retain(|(g, _)| ids.contains(g));
                Effects::none()
            }
        }
    }

    fn start(&mut self, operation: Operation, now: f64) -> Effects<Effect> {
        let attempt = self.seq.next();
        let due = matches!(operation, Operation::Undo { .. }).then_some(now + UNDO_TIMEOUT_SECS);
        self.state = State::Mutating { attempt, active: operation.clone(), pending: Vec::new(), due };
        Effects::one(Effect::Run { attempt, operation })
    }

    // The next waiting change starts.
    fn advance(&mut self, now: f64) -> Effects<Effect> {
        let State::Mutating { pending, .. } = std::mem::take(&mut self.state) else { return Effects::none() };
        let mut rest = pending.into_iter();
        match rest.next() {
            None => Effects::none(),
            Some(next) => {
                let effects = self.start(next, now);
                if let State::Mutating { pending, .. } = &mut self.state {
                    pending.extend(rest);
                }
                effects
            }
        }
    }
}

//! Stop before the model has answered takes the prompt back, after Desktop's composer
//! (`session/composer/adapter.ts:57-70`) and `session/revert.ts` `takeBack` (opencode#65505): the
//! likeliest reason to stop that early is to reword what was just sent.
//!
//! It is decided at the press: only a stop made while the composer held nothing and nothing
//! waited takes a prompt back. Then two things must come in, in either order: the server's word
//! that the stop interrupted a run, and the run's end, when the session's latest prompt is read:
//! it comes back only when nothing has answered it. Its revert is then staged and committed, and
//! once the server has agreed the prompt is the composer's draft again. Nothing of an answer is
//! lost, so there is no Undo offer, and a failure says nothing, as Desktop's does not.

use rune::machine::{Attempt, Effects, Seq};

/// The prompt to take back: its message, and the draft that puts it in the composer again.
#[derive(Clone, Debug, PartialEq)]
pub struct Prompt<D> {
    pub message: String,
    pub draft: D,
}

#[derive(Clone, Debug, PartialEq)]
pub enum State<D> {
    Idle,
    /// Stop went out while the composer held nothing and nothing waited: neither the
    /// interrupt's answer nor the run's end is in.
    Armed,
    /// The server said the stop interrupted a run, which has not ended yet.
    Interrupted,
    /// The run ended with `prompt` unanswered; the interrupt's answer is not in.
    Ended { prompt: Prompt<D> },
    /// The prompt's revert is being staged and committed, under `attempt`.
    Committing { attempt: Attempt, prompt: Prompt<D> },
}

#[derive(Clone, Debug)]
pub struct TakeBack<D> {
    pub state: State<D>,
    seq: Seq,
}

impl<D> Default for TakeBack<D> {
    fn default() -> Self {
        Self { state: State::Idle, seq: Seq::default() }
    }
}

pub enum Event<D> {
    /// Stop was asked for. `clear`: the composer held nothing, and nothing was queued or pending.
    Stopped { clear: bool },
    /// The interrupt's answer: whether it interrupted a run. A repeated Stop finds the run
    /// already stopping and interrupts nothing.
    Acknowledged { interrupted: bool },
    /// The run ended. `unanswered`: the session's latest prompt, when nothing has answered it.
    RunEnded { unanswered: Option<Prompt<D>> },
    /// The commit of `attempt` was answered.
    Settled { attempt: Attempt, ok: bool },
    /// A prompt went to the session, a run started in it, or its stop failed.
    Forgotten,
}

#[derive(Debug, PartialEq)]
pub enum Effect<D> {
    /// Stage the revert at `message`, leaving the files as they are, and commit it.
    Commit { attempt: Attempt, message: String },
    /// The prompt is the composer's draft again.
    Restore { prompt: Prompt<D> },
}

impl<D> TakeBack<D> {
    pub fn step(&mut self, event: Event<D>) -> Effects<Effect<D>> {
        let state = std::mem::replace(&mut self.state, State::Idle);
        let (next, effects) = match (state, event) {
            // A commit that went out is answered whatever else happens meanwhile.
            (State::Committing { attempt, prompt }, Event::Settled { attempt: a, ok }) if attempt == a => {
                (State::Idle, if ok { Effects::one(Effect::Restore { prompt }) } else { Effects::none() })
            }
            (state @ State::Committing { .. }, _) => (state, Effects::none()),
            (_, Event::Stopped { clear }) => (if clear { State::Armed } else { State::Idle }, Effects::none()),
            (State::Armed, Event::Acknowledged { interrupted: true }) => (State::Interrupted, Effects::none()),
            (State::Armed, Event::RunEnded { unanswered: Some(prompt) }) => (State::Ended { prompt }, Effects::none()),
            (State::Interrupted, Event::RunEnded { unanswered: Some(prompt) }) | (State::Ended { prompt }, Event::Acknowledged { interrupted: true }) => {
                let attempt = self.seq.next();
                let message = prompt.message.clone();
                (State::Committing { attempt, prompt }, Effects::one(Effect::Commit { attempt, message }))
            }
            // The stop interrupted nothing, the prompt was answered, or something else came first.
            (State::Armed | State::Ended { .. }, Event::Acknowledged { interrupted: false })
            | (State::Armed | State::Interrupted, Event::RunEnded { unanswered: None })
            | (_, Event::Forgotten) => (State::Idle, Effects::none()),
            (state, _) => (state, Effects::none()),
        };
        self.state = next;
        effects
    }
}

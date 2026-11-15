//! Writer admission at shutdown. A hook is not a seal: the last producer (including stage B's
//! adopter) can still owe rows. Only the owner of `close` can seal. State and the command queue
//! share a lock; an enqueue before the seal belongs to the final drain.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State { #[default] Open, Draining, Sealed }
pub enum Event { Hook, Close }
pub enum Effect { WakeWriter, None }

pub fn step(state: &mut State, event: Event) -> Effect {
    match (*state,event) {
        (State::Open, Event::Hook) => { *state = State::Draining; Effect::WakeWriter }
        (State::Open | State::Draining, Event::Close) => { *state = State::Sealed; Effect::WakeWriter }
        _ => Effect::None,
    }
}

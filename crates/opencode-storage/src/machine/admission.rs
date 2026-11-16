//! Writer admission at shutdown. A hook is not a seal: the last producer (including stage B's
//! adopter) can still owe rows. Only the owner of `close` can seal. State and the command queue
//! share a lock; an enqueue before the seal belongs to the final drain.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State { #[default] Open, Reopened(u64), Saving(u64), Certified(u64), Draining, Sealed }
impl State { pub fn admits(self) -> bool { !matches!(self, Self::Certified(_) | Self::Sealed) } }
pub enum Event { Save(u64), Certify(u64), Resume(u64), Hook, Close }
pub enum Effect { WakeWriter, None }

pub fn step(state: &mut State, event: Event) -> Effect {
    match (*state,event) {
        (State::Open, Event::Save(k)) => { *state = State::Saving(k); Effect::WakeWriter }
        (State::Reopened(old), Event::Save(k)) if k > old => { *state = State::Saving(k); Effect::WakeWriter }
        (State::Saving(k), Event::Certify(r)) if k == r => { *state = State::Certified(k); Effect::None }
        (State::Saving(k) | State::Certified(k), Event::Resume(r)) if k == r => { *state = State::Reopened(r); Effect::WakeWriter }
        (State::Open, Event::Resume(k)) => { *state = State::Reopened(k); Effect::None }
        (State::Reopened(k), Event::Resume(r)) if r > k => { *state = State::Reopened(r); Effect::None }
        (State::Open | State::Reopened(_) | State::Saving(_), Event::Hook) => { *state = State::Draining; Effect::WakeWriter }
        (State::Open | State::Reopened(_) | State::Draining | State::Saving(_) | State::Certified(_), Event::Close) => { *state = State::Sealed; Effect::WakeWriter }
        _ => Effect::None,
    }
}

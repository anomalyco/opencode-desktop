//! The open can write the journal's reservation directly to the database and truncate its WAL
//! without a disk barrier. A later WAL sync cannot make those main-file pages durable.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State { Durable, Owed, Syncing }

/// `Reserved`: the writer reserved the journal after the first frame (`native::writer::reserve`),
/// through the same unsynced checkpoint.
pub enum Event { Reserved, RequireDurable, SyncSucceeded, SyncFailed }
pub enum Effect { None, SyncMainFile }

pub fn step(state: &mut State, event: Event) -> Effect {
    match (*state, event) {
        (State::Durable, Event::Reserved) => { *state = State::Owed; Effect::None }
        (State::Owed, Event::RequireDurable) => { *state = State::Syncing; Effect::SyncMainFile }
        (State::Syncing, Event::SyncSucceeded) => { *state = State::Durable; Effect::None }
        (State::Syncing, Event::SyncFailed) => { *state = State::Owed; Effect::None }
        _ => Effect::None,
    }
}

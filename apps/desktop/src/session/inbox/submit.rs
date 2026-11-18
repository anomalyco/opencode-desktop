//! The composer's submit, as far as Rune's needs a lifecycle (`composer/machine.ts`
//! `ComposerSubmitState`): every other send leaves at once, but one pressed while a session is
//! still opening (`App::opening`, its page held on the old one) is held rather than dropped. It
//! goes through when the session has opened, and is cancelled, its words left in the composer,
//! when the draft changes first or the window goes elsewhere (`hold.changed`, `unmounted`).

use rune::Tx;
use rune::machine::{Attempt, Effects, Seq};

use crate::{App, Route};

#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    #[default]
    Idle,
    /// Enter (or ⌘⏎ when `alternate`) on `draft` in `route`'s composer while it opened.
    Held { attempt: Attempt, alternate: bool, draft: String, route: Route },
}

#[derive(Default)]
pub struct Submit {
    pub state: State,
    seq: Seq,
}

pub enum Event {
    Pressed { alternate: bool, draft: String, route: Route },
    /// What the window shows now, after a frame: the draft, the route, and whether the session
    /// still opens.
    Settled { draft: String, route: Route, opening: bool },
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    Submit { alternate: bool },
}

impl Submit {
    pub fn step(&mut self, event: Event) -> Effects<Effect> {
        match (&self.state, event) {
            (State::Idle, Event::Pressed { alternate, draft, route }) => {
                self.state = State::Held { attempt: self.seq.next(), alternate, draft, route };
                Effects::none()
            }
            (State::Held { alternate, draft, route, .. }, Event::Settled { draft: now, route: at, opening }) => {
                if *draft != now || *route != at {
                    self.state = State::Idle;
                    return Effects::none();
                }
                if opening {
                    return Effects::none();
                }
                let alternate = *alternate;
                self.state = State::Idle;
                Effects::one(Effect::Submit { alternate })
            }
            _ => Effects::none(),
        }
    }
}

impl App {
    /// Holds a send pressed while the session opens. True when it was held.
    pub(crate) fn hold_submit(&mut self, alternate: bool) -> bool {
        if self.opening.is_none() {
            return false;
        }
        let event = Event::Pressed { alternate, draft: self.composer.text.clone(), route: (*self.route).clone() };
        self.submit.step(event);
        true
    }
}

/// Each frame: a held send goes through once the session has opened, or ends when its draft or
/// route changed. True when it went.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    if s.submit.state == State::Idle {
        return false;
    }
    let event = Event::Settled { draft: s.composer.text.clone(), route: (*s.route).clone(), opening: s.opening.is_some() };
    let mut sent = false;
    for effect in s.submit.step(event) {
        let Effect::Submit { alternate } = effect;
        s.submit(alternate, tx);
        sent = true;
    }
    sent
}

//! What lands in an open session while it shows: Desktop's steer arrival
//! (`session-ui/src/timeline/steer-arrival-machine.ts:13-24`, `SteerArrivalState`) as Rune shows
//! it. Rows already there when a session shows, and history loaded above them, appear as they
//! are; a session's first load lands in place rather than growing in. A prompt landing below
//! what was seen grows in: Desktop grows its row (`growIn`), Rune lights it, Kit's glow
//! ([`TimelineHost::user_glow_at`](crate::TimelineHost::user_glow_at)), and the glow ending is
//! Desktop's `grown`. Rune draws a row in the frame it lands, so Desktop's `arriving` (landed,
//! not yet rendered) passes in that frame and has no state here.
//!
//! A host takes [`Seen::of`] before it applies what the stream brought and hands it to
//! [`Arrivals::landed`] after, or names each arrival itself ([`Event::Arrived`]).

use std::collections::HashMap;

use opencode_kit::model::{Message, Transcript};
use rune::Tx;
use rune::machine::Effects;
use rune_knobs::Knobs;

/// Where the open session ended before a poll.
pub struct Seen {
    session: String,
    last: String,
}

impl Seen {
    /// `t`'s last message; None while it has none yet (a first load), so nothing in it glows.
    pub fn of(t: &Transcript) -> Option<Seen> {
        t.messages.last().map(|m| Seen { session: t.session.clone(), last: m.id().to_owned() })
    }
}

/// `SteerArrivalState`.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Arrivals {
    /// No session shows (`INITIAL_STEER_ARRIVAL_STATE`, `disposed`).
    #[default]
    Idle,
    /// Session `session` shows. `growing`: its prompts growing in, with when each landed on the
    /// app clock. `landing`: how its rows land at the next poll.
    Tracking { session: String, growing: HashMap<String, f64>, landing: Landing },
}

/// How the tracked session's rows land at the next poll.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Landing {
    #[default]
    Moving,
    /// Its first load: the rows land in place (`Tx::settle`), once.
    Settling,
}

#[derive(Clone, Copy, Debug)]
pub enum Event<'a> {
    /// `sync`: session `session` shows, `first` when its first rows just loaded. Another session
    /// than the tracked one starts afresh: what it shows is history.
    Synced { session: &'a str, first: bool },
    /// Prompt `id` landed below what was seen, at `now`: it grows in.
    Arrived { id: &'a str, now: f64 },
    /// These messages left (a revert), and their rows with them.
    Gone(&'a [String]),
    /// A frame at `now`: a first load settles, and glows `secs` long end (`grown`).
    Frame { now: f64, secs: f64 },
    /// No session shows (`disposed`).
    Left,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Land the rows in place (`Tx::settle`).
    Settle,
    /// Another frame: a prompt glows.
    Frame,
}

impl Arrivals {
    /// One transition.
    pub fn step(&mut self, event: Event) -> Effects<Effect> {
        match event {
            Event::Left => {
                *self = Arrivals::Idle;
                Effects::none()
            }
            Event::Synced { session, first } => {
                match self {
                    Arrivals::Tracking { session: s, landing, .. } if s == session => {
                        if first {
                            *landing = Landing::Settling;
                        }
                    }
                    _ => {
                        let landing = if first { Landing::Settling } else { Landing::Moving };
                        *self = Arrivals::Tracking { session: session.to_owned(), growing: HashMap::new(), landing };
                    }
                }
                Effects::none()
            }
            Event::Arrived { id, now } => {
                let Arrivals::Tracking { growing, .. } = self else { return Effects::none() };
                growing.insert(id.to_owned(), now);
                Effects::one(Effect::Frame)
            }
            Event::Gone(ids) => {
                if let Arrivals::Tracking { growing, .. } = self {
                    growing.retain(|id, _| !ids.contains(id));
                }
                Effects::none()
            }
            Event::Frame { now, secs } => {
                let Arrivals::Tracking { growing, landing, .. } = self else { return Effects::none() };
                let mut effects = Effects::none();
                if std::mem::take(landing) == Landing::Settling {
                    effects.push(Effect::Settle);
                }
                growing.retain(|_, at| now - *at < secs);
                if !growing.is_empty() {
                    effects.push(Effect::Frame);
                }
                effects
            }
        }
    }

    /// After a poll: each prompt in `t` past what was `seen` starts glowing at `now`. Older
    /// history loaded above stays still; so does a session's first load, which instead lands in
    /// place at the next [`Self::poll`]. True when any glow started.
    pub fn landed(&mut self, seen: Option<&Seen>, t: &Transcript, now: f64) -> bool {
        let Some(seen) = seen.filter(|s| s.session == t.session) else {
            self.step(Event::Synced { session: &t.session, first: !t.messages.is_empty() });
            return false;
        };
        self.step(Event::Synced { session: &t.session, first: false });
        let Some(end) = t.messages.iter().rposition(|m| m.id() == seen.last) else { return false };
        let mut lit = false;
        for m in &t.messages[end + 1..] {
            if let Message::User { id, .. } = m {
                self.step(Event::Arrived { id, now });
                lit = true;
            }
        }
        lit
    }

    /// Whether a first load waits to land in place at the next [`Self::poll`].
    pub fn settling(&self) -> bool {
        matches!(self, Arrivals::Tracking { landing: Landing::Settling, .. })
    }

    /// Whether any prompt grows in.
    pub fn is_growing(&self) -> bool {
        matches!(self, Arrivals::Tracking { growing, .. } if !growing.is_empty())
    }

    /// When prompt `id` landed, while it grows in.
    pub fn glow_at(&self, id: &str) -> Option<f64> {
        match self {
            Arrivals::Tracking { growing, .. } => growing.get(id).copied(),
            Arrivals::Idle => None,
        }
    }

    /// A frame on: a first load settles, finished glows drop, and a frame follows while any
    /// glows. True when what shows changed.
    /// Whether `poll` has anything to do: rows growing or landing. Polls write nothing otherwise.
    pub fn busy(&self) -> bool {
        matches!(self, Arrivals::Tracking { growing, landing, .. } if !growing.is_empty() || *landing != Landing::default())
    }

    /// `held`: the open timeline's place, which the settle keeps (`jump::held`).
    pub fn poll(&mut self, k: &Knobs, now: f64, tx: &mut Tx, held: Option<&(String, String, f32)>) -> bool {
        let mut glowing = false;
        for effect in self.step(Event::Frame { now, secs: crate::knobs::glow_secs(k) }) {
            match effect {
                Effect::Settle => crate::jump::settle_held(tx, held),
                Effect::Frame => {
                    tx.wake_after(std::time::Duration::ZERO);
                    glowing = true;
                }
            }
        }
        glowing
    }
}


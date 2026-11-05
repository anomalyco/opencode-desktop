//! What lands in an open session while it shows: prompts from another client glow as they arrive
//! ([`TimelineHost::user_glow_at`](crate::TimelineHost::user_glow_at)), and a session's first
//! load lands in place rather than growing in. A host takes [`Seen::of`] before it applies what
//! the stream brought and hands it to [`Arrivals::landed`] after.

use std::collections::HashMap;

use opencode_kit::model::{Message, Transcript};
use rune::Tx;
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

/// Glowing prompts by id, with when each landed.
#[derive(Default)]
pub struct Arrivals {
    glows: HashMap<String, f64>,
    settle: bool,
}

impl Arrivals {
    /// After a poll: each prompt in `t` past what was `seen` starts glowing at `now`. Older
    /// history loaded above stays still; so does a session's first load, which instead lands in
    /// place at the next [`Self::poll`]. True when any glow started.
    pub fn landed(&mut self, seen: Option<&Seen>, t: &Transcript, now: f64) -> bool {
        let Some(seen) = seen.filter(|s| s.session == t.session) else {
            self.settle |= !t.messages.is_empty();
            return false;
        };
        let Some(end) = t.messages.iter().rposition(|m| m.id() == seen.last) else { return false };
        let mut lit = false;
        for m in &t.messages[end + 1..] {
            if let Message::User { id, .. } = m {
                self.glows.insert(id.clone(), now);
                lit = true;
            }
        }
        lit
    }

    /// Whether a first load waits to land in place at the next [`Self::poll`].
    pub fn settling(&self) -> bool {
        self.settle
    }

    pub fn glow_at(&self, id: &str) -> Option<f64> {
        self.glows.get(id).copied()
    }

    /// A frame on: a first load settles, finished glows drop, and a frame follows while any
    /// glows. True when what shows changed.
    pub fn poll(&mut self, k: &Knobs, now: f64, tx: &mut Tx) -> bool {
        if std::mem::take(&mut self.settle) {
            tx.settle();
        }
        let secs = crate::knobs::glow_secs(k);
        self.glows.retain(|_, at| now - *at < secs);
        if self.glows.is_empty() {
            return false;
        }
        tx.wake_after(std::time::Duration::ZERO);
        true
    }
}

#[cfg(test)]
#[path = "arrivals_tests.rs"]
mod tests;

//! A steered subagent: each later call to the same agent turns the last card's spinner into a
//! steered dash in place, and only the newest card runs on to its ✓.

use super::Run;
use crate::agent_rows::steered_stories as st;

pub fn steered(r: &mut Run) {
    r.act("an agent starts", |rt| rt.update(|s, tx| st::call(s, tx)));
    r.play(1.0);
    for _ in 0..2 {
        r.act("it is steered", |rt| rt.update(|s, tx| st::call(s, tx)));
        r.play(1.0);
    }
    r.act("it finishes", |rt| rt.update(|s, tx| st::finish(s, tx)));
    // The capsule leaves with the agent and the pills' lane closes after it, about 0.8 s.
    r.play(1.5);
}

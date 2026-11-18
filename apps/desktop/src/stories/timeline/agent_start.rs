//! The storybook's Subagent starting story (Timeline › Subagent starting): a call is written
//! ("Delegating agent…"), its agent starts at 2.4 s and the row wakes, then steps along. R
//! replays from "Delegating agent…"; S starts the agent now, or delegates another once it runs.

use opencode_kit::model::Message;
use rune::{Play, Tx};

use crate::App;
use crate::stories::timeline::agent::{advance, delegate, delegating, spawn};
use crate::stories::{Trigger, replay, sid};

pub const KEYS: &[Trigger] = &[replay(open), Trigger { key: 's', label: "Start the agent", run: |s, tx| start(s, tx) }];

/// When the agent starts, and how long it steps along after.
pub const HANDOFF: f64 = 2.4;
const STEP: f64 = 0.9;

fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    // The turn is still writing the call, as it is in a real session.
    let id = sid(s);
    if let Some(Message::Assistant(a)) = s.transcripts.get_mut(&id).and_then(|t| t.messages.last_mut()) {
        a.completed = None;
    }
    s.running.insert(id);
    tx.after(0.3, |s, tx| delegate(s, tx));
    tx.after(HANDOFF, |s, tx| {
        if delegating(s) {
            spawn(s, tx);
        }
    });
    let mut t = HANDOFF + STEP;
    while t < 20.0 {
        tx.after(t, |s, tx| advance(s, tx));
        t += STEP;
    }
}

/// Starts the call being written, or writes another.
fn start(s: &mut App, tx: &mut Tx) {
    if delegating(s) { spawn(s, tx) } else { delegate(s, tx) }
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Subagent starting", KEYS),
]);

//! The storybook's Stop story (Timeline › Stop): a run streams its answer under Working. S
//! stops it as the composer's Stop does: on the press Stop's square shrinks into Send's arrow
//! and Interrupted rises into Working's place, the words already on their way still land, and
//! a moment later the server ends the step as interrupted.

use rune::Play;
use serde_json::json;

use crate::App;
use crate::stories::{Trigger, emit, replay, sid};

pub const KEYS: &[Trigger] = &[replay(open), Trigger { key: 's', label: "Stop (Esc)", run: |s, _| s.stop() }];

const REPLY: &str = "msg_zzzzstorystop";

fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    let id = sid(s);
    s.running.insert(id.clone());
    s.composer.focused = true;
    emit(s, "session.execution.started", json!({ "assistantMessageID": REPLY }), tx);
    emit(s, "session.step.started", json!({ "assistantMessageID": REPLY, "agent": "build", "model": { "id": "claude-opus-5-5", "providerID": "anthropic" } }), tx);
    emit(s, "session.text.started", json!({ "assistantMessageID": REPLY, "ordinal": 0 }), tx);
    let words = "Reading the reconnect loop first, then the backoff it sleeps on, then the tests that wait on real timers and how long each of them takes to run on a busy machine.";
    for (k, w) in words.split_inclusive(' ').enumerate() {
        s.schedule(s.clock + 0.2 + k as f64 * 0.12, "session.text.delta", json!({ "sessionID": id, "assistantMessageID": REPLY, "ordinal": 0, "delta": w }));
    }
    s.wake_at_playback();
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Stop", KEYS),
]);

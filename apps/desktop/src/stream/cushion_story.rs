//! The storybook's Room under a reply story (Timeline › Room under a reply): a run thinks, reads a
//! file, streams a few lines of answer, reads again and finishes. Working sits its lane's usual
//! gap under each row; only while the answer's words stream in does a line of room open over it,
//! for the next line to land in (p191, p252).

use rune::Play;
use serde_json::{Value, json};

use crate::App;
use crate::stories::{Trigger, replay, sid};

pub const KEYS: &[Trigger] = &[replay(open)];

/// The words the answer streams, one every [`WORD`] seconds.
const ANSWER: &str = "The backoff resets on every message, so a flapping server never waits longer than the first delay. I'll keep the last success time and reset only after a healthy minute.";
const WORD: f64 = 0.05;

/// The run's events from its start, in seconds: (when, type, step, what else).
pub(crate) fn script() -> Vec<(f64, &'static str, &'static str, Value)> {
    let tool = |id: &str| json!({ "id": id, "name": "read", "input": { "filePath": "src/backoff.ts" } });
    let none = json!({});
    let mut v = vec![
        (0.10, "session.execution.started", "msg_zzzzroom_1", none.clone()),
        (0.10, "session.step.started", "msg_zzzzroom_1", none.clone()),
        (0.20, "session.reasoning.started", "msg_zzzzroom_1", json!({ "ordinal": 0 })),
        (0.25, "session.reasoning.delta", "msg_zzzzroom_1", json!({ "ordinal": 0, "delta": "Read the backoff first." })),
        (0.60, "session.reasoning.ended", "msg_zzzzroom_1", json!({ "ordinal": 0 })),
        (0.80, "session.tool.input.started", "msg_zzzzroom_1", tool("call_zzzzroom_1")),
        (0.85, "session.tool.called", "msg_zzzzroom_1", tool("call_zzzzroom_1")),
        (1.10, "session.tool.success", "msg_zzzzroom_1", tool("call_zzzzroom_1")),
        (1.15, "session.step.ended", "msg_zzzzroom_1", json!({ "finish": "tool-calls" })),
        (1.25, "session.step.started", "msg_zzzzroom_2", none.clone()),
        (1.30, "session.text.started", "msg_zzzzroom_2", json!({ "ordinal": 0 })),
    ];
    let words: Vec<&str> = ANSWER.split_inclusive(' ').collect();
    for (k, w) in words.iter().enumerate() {
        v.push((1.40 + k as f64 * WORD, "session.text.delta", "msg_zzzzroom_2", json!({ "ordinal": 0, "delta": w })));
    }
    let t = 1.40 + words.len() as f64 * WORD + 0.1;
    v.extend([
        (t, "session.text.ended", "msg_zzzzroom_2", json!({ "ordinal": 0 })),
        (t + 0.10, "session.tool.input.started", "msg_zzzzroom_2", tool("call_zzzzroom_2")),
        (t + 0.15, "session.tool.called", "msg_zzzzroom_2", tool("call_zzzzroom_2")),
        (t + 0.45, "session.tool.success", "msg_zzzzroom_2", tool("call_zzzzroom_2")),
        (t + 0.50, "session.step.ended", "msg_zzzzroom_2", json!({ "finish": "tool-calls" })),
        (t + 0.70, "session.step.started", "msg_zzzzroom_3", none.clone()),
        (t + 0.75, "session.text.started", "msg_zzzzroom_3", json!({ "ordinal": 0 })),
        (t + 0.80, "session.text.delta", "msg_zzzzroom_3", json!({ "ordinal": 0, "delta": "Done." })),
        (t + 0.90, "session.text.ended", "msg_zzzzroom_3", json!({ "ordinal": 0, "text": "Done." })),
        (t + 1.00, "session.step.ended", "msg_zzzzroom_3", json!({ "finish": "stop" })),
        (t + 1.00, "session.execution.succeeded", "msg_zzzzroom_3", none),
    ]);
    v
}

/// Plays [`script`] on the open session from now.
pub(crate) fn play(s: &mut App) {
    let id = sid(s);
    let at = s.clock;
    for (t, ty, step, more) in script() {
        let mut d = json!({ "sessionID": id, "assistantMessageID": step });
        d.as_object_mut().unwrap().extend(more.as_object().cloned().unwrap_or_default());
        s.schedule(at + t, ty, d);
    }
    s.wake_at_playback();
}

fn open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    play(s);
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Room under a reply", KEYS),
]);

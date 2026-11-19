//! The storybook's Streaming line growth story (Timeline › Streaming line growth): a run thinks,
//! reads a file, streams a few lines of answer, reads again and finishes. Working sits its lane's
//! usual gap under the last row throughout; each line the answer wraps onto opens in a moment
//! (Timeline › Line grow) and Working steps down by just that line. Its variants play each
//! New line reveal at a slow pace and a fast one (p252, `line_reveal`).

use rune::Play;
use rune_knobs::Group;
use serde_json::{Value, json};

use crate::App;
use crate::line_reveal::{LINE_GROW, NEW_LINE_REVEAL, Reveal, SOFT_EDGE};
use crate::stories::{Story, Trigger, replay, sid};

const KEYS: &[Trigger] = &[replay(open)];
const VARIANTS: &[(&str, usize)] = &[
    ("Clip, slow", 0),
    ("Clip, fast", 1),
    ("Grow then show, slow", 2),
    ("Grow then show, fast", 3),
    ("Soft edge, slow", 4),
    ("Soft edge, fast", 5),
    ("Held room, slow", 6),
    ("Held room, fast", 7),
];
const KNOBS: Group = Group::new("Timeline", "timeline", &[NEW_LINE_REVEAL, LINE_GROW, SOFT_EDGE]);

/// The answer it streams, a word at a time.
const ANSWER: &str = "The backoff resets on every message, so a flapping server never waits longer than the first delay. I'll keep the last success time and reset only after a healthy minute, so a server that drops every few seconds backs off properly instead of hammering the socket.";

/// Seconds between words: a slow model's pace, and a fast one's.
pub(crate) const SLOW: f64 = 0.3;
pub(crate) const FAST: f64 = 0.02;

/// When the answer's words start, in seconds from the run's start.
pub(crate) const WORDS_AT: f64 = 1.40;

/// The run's events from its start, a word every `word` seconds: (when, type, step, what else).
pub(crate) fn script(word: f64) -> Vec<(f64, &'static str, &'static str, Value)> {
    let tool = |id: &str| json!({ "id": id, "name": "read", "input": { "filePath": "src/backoff.ts" } });
    let none = json!({});
    let mut v = vec![
        (0.10, "session.execution.started", "msg_zzzzline_1", none.clone()),
        (0.10, "session.step.started", "msg_zzzzline_1", none.clone()),
        (0.20, "session.reasoning.started", "msg_zzzzline_1", json!({ "ordinal": 0 })),
        (0.25, "session.reasoning.delta", "msg_zzzzline_1", json!({ "ordinal": 0, "delta": "Read the backoff first." })),
        (0.60, "session.reasoning.ended", "msg_zzzzline_1", json!({ "ordinal": 0 })),
        (0.80, "session.tool.input.started", "msg_zzzzline_1", tool("call_zzzzline_1")),
        (0.85, "session.tool.called", "msg_zzzzline_1", tool("call_zzzzline_1")),
        (1.10, "session.tool.success", "msg_zzzzline_1", tool("call_zzzzline_1")),
        (1.15, "session.step.ended", "msg_zzzzline_1", json!({ "finish": "tool-calls" })),
        (1.25, "session.step.started", "msg_zzzzline_2", none.clone()),
        (1.30, "session.text.started", "msg_zzzzline_2", json!({ "ordinal": 0 })),
    ];
    let words: Vec<&str> = ANSWER.split_inclusive(' ').collect();
    for (k, w) in words.iter().enumerate() {
        v.push((WORDS_AT + k as f64 * word, "session.text.delta", "msg_zzzzline_2", json!({ "ordinal": 0, "delta": w })));
    }
    let t = WORDS_AT + words.len() as f64 * word + 0.1;
    v.extend([
        (t, "session.text.ended", "msg_zzzzline_2", json!({ "ordinal": 0 })),
        (t + 0.10, "session.tool.input.started", "msg_zzzzline_2", tool("call_zzzzline_2")),
        (t + 0.15, "session.tool.called", "msg_zzzzline_2", tool("call_zzzzline_2")),
        (t + 0.45, "session.tool.success", "msg_zzzzline_2", tool("call_zzzzline_2")),
        (t + 0.50, "session.step.ended", "msg_zzzzline_2", json!({ "finish": "tool-calls" })),
        (t + 0.70, "session.step.started", "msg_zzzzline_3", none.clone()),
        (t + 0.75, "session.text.started", "msg_zzzzline_3", json!({ "ordinal": 0 })),
        (t + 0.80, "session.text.delta", "msg_zzzzline_3", json!({ "ordinal": 0, "delta": "Done." })),
        (t + 0.90, "session.text.ended", "msg_zzzzline_3", json!({ "ordinal": 0, "text": "Done." })),
        (t + 1.00, "session.step.ended", "msg_zzzzline_3", json!({ "finish": "stop" })),
        (t + 1.00, "session.execution.succeeded", "msg_zzzzline_3", none),
    ]);
    v
}

/// Plays [`script`] on the open session from now.
pub(crate) fn play(s: &mut App, word: f64) {
    let id = sid(s);
    let at = *s.clock;
    for (t, ty, step, more) in script(word) {
        let mut d = json!({ "sessionID": id, "assistantMessageID": step });
        if let (Some(dst), Some(src)) = (d.as_object_mut(), more.as_object()) {
            dst.extend(src.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        s.schedule(at + t, ty, d);
    }
    s.wake_at_playback();
}

fn open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    let reveal = Reveal::ALL[(*s.fixture_run.variant / 2).min(3)];
    s.diag.knobs.set(&NEW_LINE_REVEAL, reveal.knob());
    play(s, if *s.fixture_run.variant % 2 == 1 { FAST } else { SLOW });
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story::session("Timeline/Streaming line growth", KEYS).knobs(&[KNOBS]).variants(VARIANTS),
]);

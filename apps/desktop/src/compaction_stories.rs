//! The storybook's Compaction story (Timeline › Compaction): the conversation is summarised, as
//! Desktop shows it. Variants: queued, running, done with its usage, by the provider, failed,
//! and with background compaction on, quiet running, done and opened. R replays, E ends the
//! running one, F fails it, and G starts a quiet one that ends inside its grace.

use opencode_kit::model::{CompactionStatus, Message};
use rune::Tx;
use rune_knobs::{Action, Group, Knob};
use serde_json::json;

use crate::App;
use crate::knobs::ROW_GROWTH;
use crate::stories::{REPLAY, emit, sid};

pub const VARIANT: Knob =
    Knob::choice("Compaction", &["Queued", "Running", "Done", "By the provider", "Failed", "Quiet running", "Quiet done", "Quiet opened"], 2);
pub const KNOBS: &[Group] = &[Group::new("Compaction", "stage", &[VARIANT, ROW_GROWTH])];
pub const KEYS: &[Action<App>] = &[
    REPLAY,
    Action { cap: "E", label: "End it", run: |s, tx| {
        s.play.clear();
        end(s, tx);
    } },
    Action { cap: "F", label: "Fail it", run: |s, tx| {
        s.play.clear();
        fail(s, tx);
    } },
    Action { cap: "G", label: "Quiet, ending inside the grace", run: |s, tx| {
        s.play.clear();
        s.compactions.fallback = true;
        start(s, tx);
        s.play.after(0.2, end, tx);
    } },
];

const EVENT: &str = "evt_storycompaction";
pub const ID: &str = "msg_storycompaction";
const QUEUED: &str = "inb_storycompaction";
const SUMMARY: &str = "## Goal\nMake reconnects survive a laptop sleeping mid-stream.\n\n## Done so far\n- Backoff resets after a successful connect and caps at 30 s.\n- The sidebar re-reads running sessions when the stream comes back.\n\n## Next\n- A test for the reconnect jitter with the fake clock.";

/// A snapshot scene on variant `at`, light or dark.
pub fn scene(s: &mut App, at: usize, dark: bool) {
    crate::stories::scene_variant(s, "Compaction", VARIANT, at);
    s.settings.scheme = if dark { crate::settings::Scheme::Dark } else { crate::settings::Scheme::Light };
}

pub fn open(s: &mut App, tx: &mut Tx) {
    s.finish_stream();
    let v = s.knobs.get(&VARIANT) as usize;
    s.compactions.fallback = v >= 5;
    if v == 0 {
        s.play.after(0.3, |s, tx| emit(s, "session.inbox.enqueued", json!({ "inboxID": QUEUED, "item": { "type": "compaction" } }), tx), tx);
        return;
    }
    s.play.after(0.3, start, tx);
    if (1..=4).contains(&v) {
        s.play.after(0.5, stream, tx);
    }
    match v {
        2 | 3 | 6 | 7 => s.play.after(2.6, end, tx),
        4 => s.play.after(1.6, fail, tx),
        _ => {}
    }
    if v == 7 {
        s.play.after(3.4, |s, tx| {
            tx.instant();
            s.expanded.insert(format!("compaction-{ID}"));
        }, tx);
    }
}

fn start(s: &mut App, tx: &mut Tx) {
    emit(s, "session.compaction.started", json!({ "eventID": EVENT, "reason": "auto" }), tx);
}

/// The summary's next few words, while it runs.
fn stream(s: &mut App, tx: &mut Tx) {
    let Some(c) = story(s) else { return };
    if c.status != CompactionStatus::Running || c.summary.len() >= SUMMARY.len() {
        return;
    }
    let rest = &SUMMARY[c.summary.len()..];
    let cut = rest.char_indices().filter(|(_, ch)| *ch == ' ' || *ch == '\n').nth(3).map_or(rest.len(), |(i, _)| i + 1);
    let chunk = rest[..cut].to_owned();
    emit(s, "session.compaction.delta", json!({ "text": chunk }), tx);
    s.play.after(0.12, stream, tx);
}

fn end(s: &mut App, tx: &mut Tx) {
    let v = s.knobs.get(&VARIANT) as usize;
    let tokens = json!({ "input": 10_412, "output": 1_034, "reasoning": 96, "cache": { "read": 1_880, "write": 0 } });
    let mut d = json!({ "reason": "auto", "text": SUMMARY, "tokens": tokens });
    if v == 3 {
        d = json!({ "reason": "auto", "text": "", "native": { "messages": [] }, "tokens": tokens });
    }
    emit(s, "session.compaction.ended", d, tx);
}

fn fail(s: &mut App, tx: &mut Tx) {
    let error = json!({ "type": "provider", "message": "The model's context window is too small to summarise this session." });
    emit(s, "session.compaction.failed", json!({ "reason": "auto", "error": error }), tx);
}

fn story(s: &App) -> Option<&opencode_kit::model::Compaction> {
    s.transcripts.get(&sid(s))?.messages.iter().rev().find_map(|m| match m {
        Message::Compaction(c) if c.id == ID => Some(c),
        _ => None,
    })
}

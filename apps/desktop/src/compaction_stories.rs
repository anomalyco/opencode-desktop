//! The storybook's Compaction story (Timeline › Compaction): the conversation is summarised, as
//! Desktop shows it. Variants: queued, running, done with its usage, by the provider, failed,
//! and with background compaction on, quiet running, done and opened. R replays, E ends the
//! running one, F fails it, and G starts a quiet one that ends inside its grace.

use opencode_kit::model::{CompactionStatus, Message};
use rune::{Play, Tx};
use serde_json::json;

use crate::App;
use crate::stories::{Trigger, emit, replay, sid};

/// Done first, as the story opens on it.
pub const VARIANTS: &[(&str, usize)] =
    &[("Done", 2), ("Queued", 0), ("Running", 1), ("By the provider", 3), ("Failed", 4), ("Quiet running", 5), ("Quiet done", 6), ("Quiet opened", 7)];
pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'e', label: "End it", run: |s, tx| {
        tx.clear();
        end(s, tx);
    } },
    Trigger { key: 'f', label: "Fail it", run: |s, tx| {
        tx.clear();
        fail(s, tx);
    } },
    Trigger { key: 'g', label: "Quiet, ending inside the grace", run: |s, tx| {
        tx.clear();
        s.compactions.fallback = true;
        start(s, tx);
        tx.after(0.2, |s, tx| end(s, tx));
    } },
];

const EVENT: &str = "evt_storycompaction";
pub const ID: &str = "msg_storycompaction";
const QUEUED: &str = "inb_storycompaction";
const SUMMARY: &str = "## Goal\nMake reconnects survive a laptop sleeping mid-stream.\n\n## Done so far\n- Backoff resets after a successful connect and caps at 30 s.\n- The sidebar re-reads running sessions when the stream comes back.\n\n## Next\n- A test for the reconnect jitter with the fake clock.";

fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    let v = s.variant;
    s.compactions.fallback = v >= 5;
    if v == 0 {
        tx.after(0.3, |s, tx| emit(s, "session.inbox.enqueued", json!({ "inboxID": QUEUED, "item": { "type": "compaction" } }), tx));
        return;
    }
    tx.after(0.3, |s, tx| start(s, tx));
    if (1..=4).contains(&v) {
        tx.after(0.5, stream);
    }
    match v {
        2 | 3 | 6 | 7 => tx.after(2.6, |s, tx| end(s, tx)),
        4 => tx.after(1.6, |s, tx| fail(s, tx)),
        _ => {}
    }
    if v == 7 {
        tx.after(3.4, |s, tx| {
            tx.instant();
            s.expanded.insert(format!("compaction-{ID}"));
        });
    }
}

fn start(s: &mut App, tx: &mut Tx) {
    emit(s, "session.compaction.started", json!({ "eventID": EVENT, "reason": "auto" }), tx);
}

/// The summary's next few words, while it runs.
fn stream(s: &mut App, tx: &mut Play<App>) {
    let Some(c) = story(s) else { return };
    if c.status != CompactionStatus::Running || c.summary.len() >= SUMMARY.len() {
        return;
    }
    let rest = &SUMMARY[c.summary.len()..];
    let cut = rest.char_indices().filter(|(_, ch)| *ch == ' ' || *ch == '\n').nth(3).map_or(rest.len(), |(i, _)| i + 1);
    let chunk = rest[..cut].to_owned();
    emit(s, "session.compaction.delta", json!({ "text": chunk }), tx);
    tx.after(0.12, stream);
}

fn end(s: &mut App, tx: &mut Tx) {
    let v = s.variant;
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

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Compaction", KEYS).knobs(crate::stories::ROW_KNOBS).variants(VARIANTS),
]);

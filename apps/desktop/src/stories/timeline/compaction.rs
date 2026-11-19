//! The storybook's Compaction story (Timeline › Compaction): the conversation is summarised in a
//! quiet divider, as Desktop's `quiet-compaction` draws it. Light gathers along its lines while it
//! runs, then Compacting turns to Compacted. Variants: done, running, one a person asked for (as
//! quiet), by the provider, ending inside its grace, opened, failed (the loud form), queued, and
//! right under a model switch's divider after a prompt, over Working (Kit's capture, p265): the
//! pair, then a variant switch makes a run of three; and one whose session page was read, already
//! holding it settled, before its events arrived (Kit's capture, t1400: it showed twice). R
//! replays, E ends the running one, F fails it, and G starts one that ends inside its grace.

use opencode_kit::model::{Assistant, Compaction, CompactionStatus, Message, Part, Tokens};
use rune::{Key, Play, Tx};
use serde_json::json;

use crate::App;
use crate::stories::{Trigger, emit, replay, sid};

/// Done first, as the story opens on it.
pub const VARIANTS: &[(&str, usize)] = &[
    ("Done", 2),
    ("Running", 1),
    ("Asked for", 5),
    ("By the provider", 3),
    ("Inside the grace", 6),
    ("Opened", 7),
    ("Failed", 4),
    ("Queued", 0),
    ("After a model switch", 8),
    ("Page read first", 9),
];
/// The variant whose compaction a person asked for (`/compact`).
const MANUAL: usize = 5;
const KNOBS: &[rune_knobs::Group] = &[rune_knobs::Group::new("Compaction", "stage", &[crate::knobs::ROW_GROWTH, crate::compaction::DIVIDER_GAP])];
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
        start(s, tx);
        tx.after(0.2, |s, tx| end(s, tx));
    } },
];

const EVENT: &str = "evt_storycompaction";
pub const ID: &str = "msg_storycompaction";
/// "Page read first"'s compaction, whose id sorts after the prompt it follows, as the server's do.
const LATE_EVENT: &str = "evt_zzzzzstorycompaction";
const LATE: &str = "msg_zzzzzstorycompaction";
const QUEUED: &str = "inb_storycompaction";
const SUMMARY: &str = "## Goal\nMake reconnects survive a laptop sleeping mid-stream.\n\n## Done so far\n- Backoff resets after a successful connect and caps at 30 s.\n- The sidebar re-reads running sessions when the stream comes back.\n\n## Next\n- A test for the reconnect jitter with the fake clock.";

fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    let v = *s.fixture_run.variant;
    if v == 0 {
        tx.after(0.3, |s, tx| emit(s, "session.inbox.enqueued", json!({ "inboxID": QUEUED, "item": { "type": "compaction" } }), tx));
        return;
    }
    if v == 9 {
        tx.after(0.05, |s, tx| emit(s, "session.prompted", json!({ "messageID": "msg_zzzzcompactionprompt", "text": "inbox zero plz" }), tx));
        tx.after(0.2, |s, _| settled_page(s));
        tx.after(0.3, |s, tx| emit(s, "session.compaction.started", json!({ "eventID": LATE_EVENT, "reason": "auto" }), tx));
        tx.after(0.35, |s, tx| end(s, tx));
        return;
    }
    if v == 8 {
        tx.after(0.05, |s, tx| {
            emit(s, "session.prompted", json!({ "messageID": "msg_zzzzcompactionprompt", "text": "inbox zero plz" }), tx);
            emit(s, "session.execution.started", json!({}), tx);
        });
        let model = |id: &str, variant: &str| json!({ "id": id, "providerID": "anthropic", "variant": variant });
        let data = json!({ "eventID": "evt_storycompactionmodel", "model": model("claude-opus-4-5", ""), "previous": model("claude-sonnet-4-5", "") });
        tx.after(0.1, move |s, tx| emit(s, "session.model.selected", data.clone(), tx));
        let data = json!({ "eventID": "evt_storycompactionvariant", "model": model("claude-opus-4-5", "high"), "previous": model("claude-opus-4-5", "") });
        tx.after(2.9, move |s, tx| emit(s, "session.model.selected", data.clone(), tx));
    }
    tx.after(0.3, |s, tx| start(s, tx));
    if (1..=5).contains(&v) {
        tx.after(0.5, stream);
    }
    match v {
        2 | 3 | 5 | 7 | 8 => tx.after(2.6, |s, tx| end(s, tx)),
        6 => tx.after(0.5, |s, tx| end(s, tx)),
        4 => tx.after(1.6, |s, tx| fail(s, tx)),
        _ => {}
    }
    if v == 7 {
        tx.after(3.4, |s, tx| {
            tx.instant();
            s.timeline_fx.expanded.insert(crate::compaction::Piece::Quiet.key(ID));
        });
    }
}

fn reason(s: &App) -> &'static str {
    if s.fixture_run.variant == MANUAL { "manual" } else { "auto" }
}

fn start(s: &mut App, tx: &mut Tx) {
    emit(s, "session.compaction.started", json!({ "eventID": EVENT, "reason": reason(s) }), tx);
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

fn tokens() -> serde_json::Value {
    json!({ "input": 10_412, "output": 1_034, "reasoning": 96, "cache": { "read": 1_880, "write": 0 } })
}

/// A read of the session's latest page that lands before the compaction's events, holding it
/// already settled, as the server had it.
fn settled_page(s: &mut App) {
    let id = sid(s);
    let page = json!({ "data": [{ "id": LATE, "type": "compaction", "status": "completed", "reason": "auto", "summary": SUMMARY, "tokens": tokens(), "time": { "created": s.now_ms } }] });
    if let Some(t) = s.transcripts.get_mut(&id) {
        t.merge(opencode_kit::model::parse_messages(&page));
    }
}

fn end(s: &mut App, tx: &mut Tx) {
    let v = *s.fixture_run.variant;
    let tokens = tokens();
    let mut d = json!({ "reason": reason(s), "text": SUMMARY, "tokens": tokens });
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

// ---------------------------------------------------------------------------------------------
// Timeline › Compacted summary: Kit's capture (p162). A quiet compaction with a summary several
// screens long sits near the end of a timeline that's at its bottom. O folds it open or shut on
// the tool group's spring with the end held, as Desktop's pinned timeline keeps it, S presses Page Down to scroll into
// the open summary so the divider sticks to the top, and R replays: open and shut at the end,
// open and shut again mid-glide so it springs back, then open, page down, and shut from the stuck
// divider.

pub const LONG_KEYS: &[Trigger] = &[
    replay(long_open),
    Trigger { key: 'o', label: "Open or shut the summary", run: |s, tx| crate::compaction::press(s, tx, LONG) },
    Trigger { key: 's', label: "Scroll into the summary", run: |s, tx| scroll_in(s, tx) },
];

pub const LONG: &str = "msg_zzlongcompaction";

/// Ends session `session` with an automatic compaction whose summary runs several screens, then
/// a prompt and its short answer.
pub fn add_long_summary(s: &mut App, session: &str) {
    let Some(t) = s.transcripts.get_mut(session) else { return };
    let tokens = Tokens { input: 181_000, output: 4_300, ..Default::default() };
    let c = Compaction { id: LONG.into(), status: CompactionStatus::Completed, summary: long_summary(), tokens: Some(tokens), ..Default::default() };
    t.messages.push(Message::Compaction(c));
    t.messages.push(Message::User { id: "msg_zzlongafter0".into(), text: "Open this in my Seer sidebar.".into() });
    let answer = vec![Part::Text { text: "Opened it in your Seer sidebar.".into() }];
    t.messages.push(Message::Assistant(Assistant { id: "msg_zzlongafter1".into(), completed: Some(1), parts: answer, ..Default::default() }));
}

fn long_summary() -> String {
    let mut out = String::from("## Objective\n- Keep reconnects alive through a laptop sleeping mid-stream, and land the watchdog without a flaky test.\n");
    for (head, n) in [("Requirements", 8), ("Done so far", 10), ("Blocked", 4), ("Next Move", 8)] {
        out.push_str(&format!("\n## {head}\n"));
        for i in 1..=n {
            out.push_str(&format!("- {head} {i}: the backoff resets after a healthy minute, caps at 30 s, and logs each retry with its attempt number.\n"));
        }
    }
    out
}

fn long_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    let id = sid(s);
    add_long_summary(s, &id);
    for at in [0.6, 1.4, 2.2, 2.28, 3.0, 4.6] {
        tx.after(at, |s, tx| crate::compaction::press(s, tx, LONG));
    }
    tx.after(3.8, scroll_in);
}

const LONG_KNOBS: &[rune_knobs::Group] = &[rune_knobs::Group::new("Fold", "stage", &[crate::disclosure::FOLD_SPEED, crate::disclosure::FOLD_FEATHER])];

/// Pages the timeline down into the open summary, under its stuck divider, as Kit's Page Down does.
fn scroll_in(_: &mut App, play: &mut Play<App>) {
    play.press(Key::PageDown);
}

rune_register::register!(crate::stories::scenes::SCENE_LIST: crate::stories::scenes::Scene = [
    ("long-summary", |s| {
        crate::stories::scenes::open_scene(s);
        s.finish_stream();
        if let crate::Route::Session { id, .. } = (*s.route).clone() {
            add_long_summary(s, &id);
        }
    }),
]);

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Compaction", KEYS).knobs(KNOBS).variants(VARIANTS),
    crate::stories::Story::session("Timeline/Compacted summary", LONG_KEYS).knobs(LONG_KNOBS),
]);

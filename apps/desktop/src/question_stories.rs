//! The storybook's Question answers story (Timeline › Question answers): the agent asks, the dock
//! opens while the timeline shows nothing for the call, and the answer lands as the call's row
//! listing each question over its answer. Variants: one question, several, multi-select, a typed
//! answer, dismissed. A answers the pending one, R replays.

use opencode_kit::model::{Message, Part};
use rune::{Play, Tx};
use serde_json::{Value, json};

use crate::App;
use crate::stories::{Trigger, emit, replay};

pub const VARIANTS: &[(&str, usize)] = &[("One", 0), ("Several", 1), ("Multi-select", 2), ("Custom typed", 3), ("Dismissed", 4)];
pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'a', label: "Answer the pending one", run: |s, tx| {
        tx.clear();
        answer(s, tx);
    } },
];

const STEP: &str = "msg_zzzzstoryquestion";
const CALL: &str = "call_story_question";
const FORM: &str = "frm_story_question";

/// A question: header, text, options, whether it takes several.
type Q = (&'static str, &'static str, &'static [&'static str], bool);

const CLOCK: Q = ("Backoff test", "How should the backoff test control time?", &["Fake clock", "Real timers", "Only the math"], false);
const CASES: Q = ("Cases", "Which cases should it cover?", &["Reset after success", "Cap at 30 s", "Jitter stays in range"], true);
const HOME: Q = ("Location", "Where should the test live?", &["Next to backoff.ts", "In test/net/"], false);
const CI: Q = ("CI", "Should it run on every push, or only nightly with the slow suite?", &["Every push", "Nightly"], false);

/// Variant `v`'s questions and the answers given (`None`: dismissed).
fn variant(v: usize) -> (&'static [Q], Option<Vec<Vec<&'static str>>>) {
    match v {
        1 => (&[CLOCK, CASES, HOME, CI], Some(vec![vec!["Fake clock"], vec!["Reset after success", "Cap at 30 s"], vec!["Next to backoff.ts"], vec!["Every push"]])),
        2 => (&[CASES], Some(vec![vec!["Reset after success", "Cap at 30 s", "Jitter stays in range"]])),
        3 => (&[CLOCK], Some(vec![vec!["Fake clock, but keep one slow test on real timers so we still catch a timer that never fires after the laptop wakes"]])),
        4 => (&[CLOCK], None),
        _ => (&[CLOCK], Some(vec![vec!["Fake clock"]])),
    }
}

/// The agent asks a beat in; the answer comes back a beat and a half later.
fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    // The story's question alone: without the one the fixture's turn ends on.
    let id = crate::stories::sid(s);
    if let Some(t) = s.transcripts.get_mut(&id) {
        for m in &mut t.messages {
            if let Message::Assistant(a) = m {
                a.parts.retain(|p| !matches!(p, Part::Tool { name, .. } if name == "question"));
            }
        }
    }
    tx.after(0.3, |s, tx| ask(s, tx));
    tx.after(1.8, |s, tx| answer(s, tx));
}

fn ask(s: &mut App, tx: &mut Tx) {
    for (ty, d) in asked(s.variant) {
        emit(s, ty, d, tx);
    }
}

/// A pending question's answer comes back (or it's dismissed) and the step ends.
fn answer(s: &mut App, tx: &mut Tx) {
    if s.question_shown() {
        for (ty, d) in answered(s.variant) {
            emit(s, ty, d, tx);
        }
    }
}

/// The events of variant `v` being asked: a step calls the question tool and the dock opens.
pub fn asked(v: usize) -> Vec<(&'static str, Value)> {
    let (qs, _) = variant(v);
    let input = json!({ "questions": qs.iter().map(|(header, question, options, multiple)| json!({
        "header": header,
        "question": question,
        "options": options.iter().map(|o| json!({ "label": o, "description": "" })).collect::<Vec<_>>(),
        "multiple": multiple,
    })).collect::<Vec<_>>() });
    let fields: Vec<Value> = qs.iter().enumerate().map(|(i, (header, question, options, multiple))| json!({
        "key": format!("q{i}"),
        "type": if *multiple { "multiselect" } else { "string" },
        "title": header,
        "description": question,
        "custom": true,
        "options": options.iter().map(|o| json!({ "value": o, "label": o })).collect::<Vec<_>>(),
    })).collect();
    vec![
        ("session.step.started", json!({ "assistantMessageID": STEP })),
        ("session.tool.called", json!({ "assistantMessageID": STEP, "id": CALL, "name": "question", "input": input })),
        ("form.created", json!({ "id": FORM, "title": "Questions", "metadata": { "kind": "question", "tool": { "messageID": STEP, "id": CALL } }, "fields": fields })),
    ]
}

/// The events of variant `v` being answered (or dismissed): the dock closes, the call returns
/// and the step ends.
pub fn answered(v: usize) -> Vec<(&'static str, Value)> {
    let call = json!({ "assistantMessageID": STEP, "id": CALL });
    let (close, result) = match variant(v).1 {
        Some(a) => ("form.replied", ("session.tool.success", json!({ "metadata": { "answers": a } }))),
        None => ("form.cancelled", ("session.tool.failed", json!({ "error": { "type": "unknown", "message": "The user dismissed this question" } }))),
    };
    let mut d = call;
    d.as_object_mut().unwrap().extend(result.1.as_object().unwrap().clone());
    vec![(close, json!({ "id": FORM })), (result.0, d), ("session.step.ended", json!({ "assistantMessageID": STEP }))]
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::composer("Question dock/Question", QUESTION_KEYS).variants(QUESTION_VARIANTS),
    crate::stories::Story::composer("Question dock/Custom answer", CUSTOM_KEYS).variants(CUSTOM_VARIANTS),
    crate::stories::Story {
        path: "Question dock/Morph",
        scene: |s| crate::stories::watch(s, crate::stories::session_view),
        knobs: MORPH_KNOBS,
        keys: MORPH_KEYS,
        variants: &[],
    },
    crate::stories::Story::session("Question dock/Under a queue", UNDER_QUEUE_KEYS).knobs(MORPH_KNOBS),
    crate::stories::Story::session("Timeline/Question answers", KEYS).knobs(crate::stories::ROW_KNOBS).variants(VARIANTS),
]);

const BEAT: f64 = 1.6;
const MORPH_KNOBS: &[rune_knobs::Group] = &[crate::knobs::QUESTION_DOCK];
const QUESTION_VARIANTS: &[(&str, usize)] = &[("One", 0), ("Two", 1), ("Multi-select", 2), ("Long", 3), ("Typing", 4)];
const CUSTOM_VARIANTS: &[(&str, usize)] = &[("Typing", 0), ("Long, wrapped", 1), ("Left the field", 2), ("Empty", 3)];
const QUESTION_KEYS: &[Trigger] = &[replay(question_open)];
const CUSTOM_KEYS: &[Trigger] = &[replay(custom_open)];
const MORPH_KEYS: &[Trigger] = &[
    Trigger { key: 'r', label: "Replay the loop", run: morph_open },
    Trigger { key: 'o', label: "Open the dock", run: |s, tx| {
        tx.clear();
        if !s.question_shown() {
            emit(s, "form.created", form(s, &[0]), tx);
        }
    } },
    Trigger { key: 'c', label: "Close it", run: |s, tx| {
        tx.clear();
        if s.question_shown() {
            emit(s, "form.replied", replied(s), tx);
        }
    } },
];
const UNDER_QUEUE_KEYS: &[Trigger] = &[
    replay(under_queue_open),
    Trigger { key: 'q', label: "Queue one", run: crate::stories::queue },
    Trigger { key: 'o', label: "Open the dock", run: |s, tx| if !s.question_shown() { emit(s, "form.created", form(s, &[0]), tx) } },
    Trigger { key: 'c', label: "Close it", run: |s, tx| if s.question_shown() { emit(s, "form.replied", replied(s), tx) } },
    Trigger { key: 'd', label: "Deliver the oldest", run: crate::stories::deliver_queued },
];

fn form(s: &App, keep: &[usize]) -> Value {
    let mut q = s.fixture["question"].clone();
    let fields = q["fields"].as_array().cloned().unwrap_or_default();
    q["fields"] = keep.iter().filter_map(|i| fields.get(*i).cloned()).collect();
    q
}

fn replied(s: &App) -> Value {
    let q = &s.fixture["question"];
    json!({ "id": q["id"], "sessionID": q["sessionID"] })
}

fn question_open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    let q = match s.variant {
        1 => form(s, &[0, 1]),
        2 => form(s, &[1]),
        3 => long(s),
        _ => form(s, &[0]),
    };
    s.apply_event("form.created", &q);
    if s.variant == 4 {
        s.question_type("Fake clock, but keep one slow test on real timers");
    }
}

fn custom_open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    let q = form(s, &[0, 1]);
    s.apply_event("form.created", &q);
    s.question_last();
    let text = match s.variant {
        1 | 2 => "Fake clock for the loop, but keep one slow end-to-end test on real timers so a regression in the scheduler still shows up somewhere",
        3 => "",
        _ => "All of them",
    };
    s.question_type(text);
    if matches!(s.variant, 2 | 3) {
        s.question_leave_field();
    }
}

fn long(s: &App) -> Value {
    let mut q = form(s, &[0]);
    q["fields"][0]["title"] = "Backoff test and where its clock comes from".into();
    q["fields"][0]["description"] = "The reconnect loop sleeps between attempts, doubling each time up to 30 seconds with some jitter. A test that waits on real timers would take minutes and flake on a busy machine, so how should the backoff test control time, and should it also cover the jitter staying in range?".into();
    q["fields"][0]["options"][0]["description"] = "Inject a clock into the reconnect loop and step it by hand, so each attempt's delay is checked exactly and the test runs in milliseconds whatever the machine is doing.".into();
    q["fields"][0]["options"][2]["description"] = "Pull the delay sequence out as a pure function of the attempt number and a seeded random source, and test only that, leaving the loop itself untested.".into();
    q
}

fn morph_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    tx.after(0.5, morph_ask);
}

fn morph_ask(s: &mut App, tx: &mut Play<App>) {
    emit(s, "form.created", form(s, &[0]), tx);
    tx.after(BEAT, morph_answer);
}

fn morph_answer(s: &mut App, tx: &mut Play<App>) {
    emit(s, "form.replied", replied(s), tx);
    tx.after(BEAT, morph_ask);
}

fn under_queue_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    let id = crate::stories::sid(s);
    s.running.insert(id);
    tx.after(0.2, crate::stories::queue);
    tx.after(1.0, |s, tx| emit(s, "form.created", form(s, &[0]), tx));
}

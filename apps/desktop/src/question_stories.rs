//! The storybook's Question answers story (Timeline › Question answers): the agent asks, the dock
//! opens while the timeline shows nothing for the call, and the answer lands as the call's row
//! listing each question over its answer. Variants: one question, several, multi-select, a typed
//! answer, dismissed. A answers the pending one, R replays.

use opencode_kit::model::{Message, Part};
use rune::Tx;
use rune_knobs::{Action, Group, Knob};
use serde_json::{Value, json};

use crate::App;
use crate::knobs::ROW_GROWTH;
use crate::stories::{emit, replay};

pub const VARIANT: Knob = Knob::choice("Questions", &["One", "Several", "Multi-select", "Custom typed", "Dismissed"], 0);
pub const KNOBS: &[Group] = &[Group::new("Question answers", "stage", &[VARIANT, ROW_GROWTH])];
pub const KEYS: &[Action<App>] = &[
    Action { cap: "R", label: "Replay", run: replay },
    Action { cap: "A", label: "Answer the pending one", run: |s, tx| {
        s.play.clear();
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

/// A snapshot scene on variant `at`.
pub fn scene(s: &mut App, at: usize) {
    crate::stories::scene_variant(s, "Question answers", VARIANT, at);
}

/// The agent asks a beat in; the answer comes back a beat and a half later.
pub fn open(s: &mut App, tx: &mut Tx) {
    s.finish_stream();
    // The story's question alone: without the one the fixture's turn ends on.
    if let Some(t) = s.transcripts.get_mut(&crate::stories::sid(s)) {
        for m in &mut t.messages {
            if let Message::Assistant(a) = m {
                a.parts.retain(|p| !matches!(p, Part::Tool { name, .. } if name == "question"));
            }
        }
    }
    s.play.after(0.3, ask, tx);
    s.play.after(1.8, answer, tx);
}

fn ask(s: &mut App, tx: &mut Tx) {
    for (ty, d) in asked(s.knobs.get(&VARIANT) as usize) {
        emit(s, ty, d, tx);
    }
}

/// A pending question's answer comes back (or it's dismissed) and the step ends.
fn answer(s: &mut App, tx: &mut Tx) {
    if s.question_shown() {
        for (ty, d) in answered(s.knobs.get(&VARIANT) as usize) {
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

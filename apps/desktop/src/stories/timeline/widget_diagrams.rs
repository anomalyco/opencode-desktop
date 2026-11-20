//! The storybook's Widget diagrams story (Timeline › Widget diagrams): the agent calls `widget`
//! with each diagram built-in (`pie` … `table`) on its example data from `@opencode/widget`'s
//! fixtures, all in one turn (All), or one per variant, to check each renders.

use rune::{Play, Tx};
use serde_json::{Value, json};

use crate::App;
use crate::session::widgets::{fixtures, preload};
use crate::stories::{Trigger, emit, replay};

pub const VARIANTS: &[(&str, usize)] = &[
    ("All", 0),
    ("Pie", 1),
    ("Scatter", 2),
    ("Histogram", 3),
    ("Area", 4),
    ("Heatmap", 5),
    ("Treemap", 6),
    ("Sankey", 7),
    ("Graph", 8),
    ("Tree", 9),
    ("Timeline", 10),
    ("Funnel", 11),
    ("Radar", 12),
    ("Gauge", 13),
    ("Box plot", 14),
    ("Flowchart", 15),
    ("Sequence", 16),
    ("Table", 17),
];
pub const KEYS: &[Trigger] = &[replay(open)];

/// The assistant message holding the calls.
const ID: &str = "msg_zzzzstorydiagrams";

/// A version for a fixture template: any 16 hex digits do, as long as each template has its own.
fn version(name: &str) -> String {
    let n = name.bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)));
    format!("{n:016x}")
}

fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    for name in fixtures::DIAGRAMS {
        if let Some(program) = fixtures::program(name) {
            preload(s, name, &version(name), &program, "step", &Value::Null);
        }
    }
    tx.after(0.3, |s, tx| post(s, tx));
}

/// The agent's step: one `widget` call per diagram shown, each posted by the plugin.
fn post(s: &mut App, tx: &mut Tx) {
    let v = *s.fixture_run.variant;
    let names: Vec<&str> = match v {
        0 => fixtures::DIAGRAMS.to_vec(),
        v => fixtures::DIAGRAMS.get(v - 1).copied().into_iter().collect(),
    };
    let sid = crate::stories::sid(s);
    emit(s, "session.step.started", json!({ "assistantMessageID": ID, "agent": "build", "model": { "id": "claude-opus-5-5", "providerID": "anthropic" } }), tx);
    for (i, name) in names.into_iter().enumerate() {
        let call = format!("call_diagram_{i:02}");
        let data = fixtures::example(name).unwrap_or_else(|| json!({}));
        let title = data["title"].as_str().unwrap_or(name).to_owned();
        let input = json!({ "template": name, "data": data, "title": title });
        emit(s, "session.tool.input.started", json!({ "assistantMessageID": ID, "id": call, "name": "widget" }), tx);
        emit(s, "session.tool.called", json!({ "assistantMessageID": ID, "id": call, "name": "widget", "input": input, "executed": true }), tx);
        let state = json!({
            "sessionID": sid, "messageID": ID, "callID": call, "template": { "name": name, "version": version(name) },
            "title": title, "status": "open", "wait": false, "time": { "created": 0 },
        });
        emit(s, opencode_kit::model::WIDGET_CHANGED_EVENT, json!({ "sessionID": sid, "state": state }), tx);
        let text = format!("Widget \"{title}\" is shown to the user.");
        emit(s, "session.tool.success", json!({ "assistantMessageID": ID, "id": call, "content": [{ "type": "text", "text": text }], "metadata": { "template": { "name": name, "version": version(name) } }, "executed": true }), tx);
    }
    emit(s, "session.step.ended", json!({ "assistantMessageID": ID, "finish": "stop", "cost": 0.0, "tokens": { "input": 8, "output": 40, "reasoning": 0, "cache": { "read": 0, "write": 0 } } }), tx);
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Widget diagrams", KEYS).knobs(crate::stories::ROW_KNOBS).variants(VARIANTS),
]);

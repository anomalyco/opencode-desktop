//! The storybook's Widgets story (Timeline › Widgets): the agent calls the `widget` tool, the
//! widgets plugin's `changed` event brings its state, and it runs in the call's place in the
//! timeline on the Rust PsychoScript, with the built-in templates (`suggestions`, `form`,
//! `confirm`) and their fixture data. Variants: each built-in, one another client already
//! answered (read-only, its final view and answer), a template whose `init` throws (the error
//! card), the `chart` canvas (bars, and a line you can choose a point on), the `backdrop`
//! shader (plasma, aurora), a shader whose WGSL doesn't compile, and the live `minigame`, whose
//! program runs on a thread of its own. R replays; E answers it from another client, D dismisses it from one. Pictures
//! come from the storybook, never the web.

use std::sync::Arc;

use crate::session::widgets::fixtures;
use rune::{Play, Tx};
use serde_json::{Value, json};

use crate::App;
use crate::session::widgets::preload;
use crate::stories::{Trigger, emit, replay};

pub const VARIANTS: &[(&str, usize)] = &[
    ("Suggestions", 0),
    ("Form", 1),
    ("Confirm", 2),
    ("Answered elsewhere", 3),
    ("Broken template", 4),
    ("Chart", 5),
    ("Line chart", 6),
    ("Backdrop", 7),
    ("Backdrop aurora", 8),
    ("Broken shader", 9),
    ("Minigame (live)", 10),
    ("Live runaway", 11),
];
pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'e', label: "Answer it from another client", run: |s, tx| {
        tx.clear();
        answer_elsewhere(s, tx);
    } },
    Trigger { key: 'd', label: "Dismiss it from another client", run: |s, tx| {
        tx.clear();
        changed(s, "dismissed", Value::Null, tx);
    } },
];

/// The assistant message holding the call, and the call.
const ID: &str = "msg_zzzzstorywidget";
const CALL: &str = "call_storywidget";
const BROKEN: &str = "story-broken";
const BROKEN_SHADER: &str = "story-broken-shader";
/// A live template granted the network that, half a second in, runs without awaiting: the 50 ms
/// rule stops it, and it shows its last view with why and Restart.
const RUNAWAY: &str = "story-runaway";
/// Where the story's pictures say they are; [`serve`] answers for them.
const PICTURES: &str = "https://pictures.story.example/";

/// A version for a fixture template: any 16 hex digits do, as long as each template has its own.
fn version(name: &str) -> String {
    let n = name.bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)));
    format!("{n:016x}")
}

fn template(v: usize) -> &'static str {
    match v {
        1 => "form",
        2 => "confirm",
        4 => BROKEN,
        5 | 6 => "chart",
        7 | 8 => "backdrop",
        9 => BROKEN_SHADER,
        10 => "minigame",
        11 => RUNAWAY,
        _ => "suggestions",
    }
}

fn picture(name: &str) -> String {
    format!("{PICTURES}{name}.png")
}

/// Variant `v`'s title and data.
fn data(v: usize) -> (&'static str, Value) {
    match v {
        1 => ("Deploy settings", json!({
            "title": "Deploy the preview",
            "description": "Pick how the **preview environment** runs. You can change these later in `deploy.toml`.",
            "submitLabel": "Deploy",
            "fields": [
                { "key": "name", "type": "text", "label": "Environment name", "placeholder": "preview-…", "default": "preview-reconnect" },
                { "key": "replicas", "type": "number", "label": "Replicas", "min": 1, "max": 8, "default": 2 },
                { "key": "region", "type": "select", "label": "Region", "options": [
                    { "value": "fra", "label": "Frankfurt" }, { "value": "iad", "label": "Virginia" }, { "value": "sin", "label": "Singapore" },
                ], "default": "fra" },
                { "key": "checks", "type": "select", "label": "Checks to run first", "multiple": true, "options": [
                    { "value": "lint", "label": "Lint" }, { "value": "unit", "label": "Unit tests" }, { "value": "e2e", "label": "End to end" },
                ], "default": ["lint", "unit"] },
                { "key": "canary", "type": "boolean", "label": "Send 5% of traffic first" },
                { "key": "notes", "type": "text", "label": "Notes for reviewers", "multiline": true },
            ],
        })),
        2 => ("Delete the branch", json!({
            "title": "Delete feature/reconnect-backoff?",
            "message": "The branch is merged into `main`. Deleting it removes it **locally and on origin**; its commits stay in `main`.",
            "confirmLabel": "Delete branch",
            "cancelLabel": "Keep it",
            "danger": true,
        })),
        4 => ("Release notes", json!({ "title": "Release notes" })),
        5 => ("Build times", json!({
            "title": "Build times this week",
            "kind": "bar",
            "unit": "s",
            "labels": ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
            "series": [
                { "label": "Cold", "values": [412, 398, 455, 380, 362, 401, 377] },
                { "label": "Warm", "values": [61, 58, 72, 55, 49, 66, 52] },
            ],
        })),
        6 => ("Latency", json!({
            "title": "p95 latency after the cache change",
            "kind": "line",
            "unit": "ms",
            "labels": ["09:00", "10:00", "11:00", "12:00", "13:00", "14:00", "15:00", "16:00"],
            "series": [{ "label": "p95", "values": [182, 176, 169, 121, 98, 94, 97, 91] }],
            "choose": true,
        })),
        7 => ("Launch banner", json!({ "title": "Rune 1.0 is out", "text": "Native **GPU** rendering, now with widgets.", "preset": "plasma" })),
        8 => ("Aurora", json!({ "title": "Northern lights", "text": "An aurora drawn by a WGSL shader.", "preset": "aurora" })),
        9 => ("Broken shader", json!({})),
        10 => ("Tap the dots", json!({ "title": "Tap the dots", "seconds": 20 })),
        11 => ("Price ticker", json!({})),
        _ => ("Pick a hero image", json!({
            "title": "Which hero image should the landing page use?",
            "options": [
                { "label": "Coastline", "note": "Warm, calm; reads well under white text.", "recommended": true, "images": [picture("coast-1"), picture("coast-2"), picture("coast-3")] },
                { "label": "Forest", "note": "Darker; needs the light header.", "images": [picture("forest-1"), picture("forest-2")] },
                { "label": "City at night", "images": [picture("city-1")] },
            ],
        })),
    }
}

/// The storybook's pictures: a gradient each, its colours from its name.
fn serve(s: &App) {
    let web = Arc::new(|url: &str| -> Result<Vec<u8>, String> {
        let name = url.strip_prefix(PICTURES).ok_or("offline")?;
        let h = name.bytes().fold(7u32, |h, b| h.wrapping_mul(131).wrapping_add(u32::from(b)));
        let hue = |k: u32| ((h >> k) & 0xff) as f32 / 255.0;
        let a = [0.2 + 0.6 * hue(0), 0.25 + 0.5 * hue(8), 0.3 + 0.6 * hue(16)];
        let b = [0.1 + 0.4 * hue(24), 0.15 + 0.5 * hue(4), 0.35 + 0.5 * hue(12)];
        Ok(crate::stories::gradient((480, 360), a, b))
    });
    s.composer_ext.attach.markdown.set_fetch(Arc::new(|_: &str, _: &str| Err("not in the storybook".into())), web);
}

fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    serve(s);
    for name in fixtures::BUILTINS {
        if let Some(program) = fixtures::program(name) {
            preload(s, name, &version(name), &program, fixtures::mode(name), &Value::Null);
        }
    }
    // Throws in `init`: the error card, with Try again.
    let broken = format!("{}\nfunction view(state, data) {{\n  return ui.text(data.sections.length + \" sections\")\n}}", fixtures::PRELUDE);
    preload(s, BROKEN, &version(BROKEN), &broken, "step", &Value::Null);
    // A shader whose WGSL doesn't compile: its alt, with the compiler's message.
    let shader = format!(
        "{}\nfunction view(state, data) {{\n  return ui.column({{ gap: 2 }}, [ui.text(\"A broken shader\", {{ style: \"title\" }}), ui.shader(\"fn shade(frag: vec2f) -> vec4f {{ return vec4f(glow, 0.0, 0.0, 1.0); }}\", {{ height: 120, alt: \"A red glow\" }})])\n}}",
        fixtures::PRELUDE
    );
    preload(s, BROKEN_SHADER, &version(BROKEN_SHADER), &shader, "step", &Value::Null);
    let runaway = format!(
        "{}\nasync function main(data) {{\n  widget.render(ui.column({{ gap: 2 }}, [ui.text(\"Price ticker\", {{ style: \"title\" }}), ui.text(\"BTC 97,412.50 · ETH 3,604.12\", {{ style: \"body\" }})]))\n  await widget.sleep(500)\n  while (true) {{}}\n}}",
        fixtures::PRELUDE
    );
    preload(s, RUNAWAY, &version(RUNAWAY), &runaway, "live", &json!({ "https": { "hosts": ["api.coingecko.com"] } }));
    tx.after(0.3, |s, tx| post(s, tx));
    if *s.fixture_run.variant == 3 {
        tx.after(0.9, |s, tx| answer_elsewhere(s, tx));
    }
}

/// The agent's step: it calls `widget`, and the plugin publishes the widget it posted.
fn post(s: &mut App, tx: &mut Tx) {
    let v = *s.fixture_run.variant;
    let name = template(v);
    let (title, data) = data(v);
    let input = json!({ "template": name, "data": data, "title": title });
    emit(s, "session.step.started", json!({ "assistantMessageID": ID, "agent": "build", "model": { "id": "claude-opus-5-5", "providerID": "anthropic" } }), tx);
    emit(s, "session.tool.input.started", json!({ "assistantMessageID": ID, "id": CALL, "name": "widget" }), tx);
    emit(s, "session.tool.called", json!({ "assistantMessageID": ID, "id": CALL, "name": "widget", "input": input, "executed": true }), tx);
    changed(s, "open", Value::Null, tx);
    let text = format!("Widget \"{title}\" is shown to the user.");
    emit(s, "session.tool.success", json!({ "assistantMessageID": ID, "id": CALL, "content": [{ "type": "text", "text": text }], "metadata": { "template": { "name": name, "version": version(name) } }, "executed": true }), tx);
    emit(s, "session.step.ended", json!({ "assistantMessageID": ID, "finish": "stop", "cost": 0.0, "tokens": { "input": 8, "output": 40, "reasoning": 0, "cache": { "read": 0, "write": 0 } } }), tx);
}

/// The plugin's `changed` event for the story's widget, now `status`.
fn changed(s: &mut App, status: &str, response: Value, tx: &mut Tx) {
    let v = *s.fixture_run.variant;
    let name = template(v);
    let (title, _) = data(v);
    let sid = crate::stories::sid(s);
    let mut state = json!({
        "sessionID": sid, "messageID": ID, "callID": CALL, "template": { "name": name, "version": version(name) },
        "title": title, "status": status, "wait": false, "time": { "created": 0 },
    });
    if status == "submitted" {
        state["response"] = response;
    }
    emit(s, opencode_kit::model::WIDGET_CHANGED_EVENT, json!({ "sessionID": sid, "state": state }), tx);
}

fn answer_elsewhere(s: &mut App, tx: &mut Tx) {
    let response = match *s.fixture_run.variant {
        1 => json!({ "name": "preview-reconnect", "replicas": 3, "region": "iad", "checks": ["lint", "unit", "e2e"], "canary": true, "notes": "" }),
        2 => json!({ "confirmed": true }),
        _ => json!({ "choice": "Coastline", "index": 0, "note": "Crop it a little tighter" }),
    };
    changed(s, "submitted", response, tx);
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Widgets", KEYS).knobs(crate::stories::ROW_KNOBS).variants(VARIANTS),
]);

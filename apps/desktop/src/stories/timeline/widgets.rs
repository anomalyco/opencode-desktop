//! The storybook's Widgets story (Timeline › Widgets): the agent posts a widget and it runs in
//! the timeline on the Rust PsychoScript, with the built-in templates (`suggestions`, `form`,
//! `confirm`) and their fixture data. Variants: each built-in, one another client already
//! answered (read-only, its final view and answer), and a template whose `init` throws (the
//! error card). R replays; E answers it from another client, D dismisses it from one. Pictures
//! come from the storybook, never the web.

use std::sync::Arc;

use crate::session::widgets::fixtures;
use rune::{Play, Tx};
use serde_json::{Value, json};

use crate::App;
use crate::session::widgets::preload;
use crate::stories::{Trigger, emit, replay};

pub const VARIANTS: &[(&str, usize)] = &[("Suggestions", 0), ("Form", 1), ("Confirm", 2), ("Answered elsewhere", 3), ("Broken template", 4)];
pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'e', label: "Answer it from another client", run: |s, tx| {
        tx.clear();
        answer_elsewhere(s, tx);
    } },
    Trigger { key: 'd', label: "Dismiss it from another client", run: |s, tx| {
        tx.clear();
        emit(s, "session.widget.responded", json!({ "messageID": ID, "status": "dismissed" }), tx);
    } },
];

const ID: &str = "msg_zzzzstorywidget";
const BROKEN: &str = "story-broken";
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
            preload(s, name, &version(name), &program);
        }
    }
    // Throws in `init`: the error card, with Try again.
    let broken = format!("{}\nfunction view(state, data) {{\n  return ui.text(data.sections.length + \" sections\")\n}}", fixtures::PRELUDE);
    preload(s, BROKEN, &version(BROKEN), &broken);
    tx.after(0.3, |s, tx| post(s, tx));
    if *s.fixture_run.variant == 3 {
        tx.after(0.9, |s, tx| answer_elsewhere(s, tx));
    }
}

fn post(s: &mut App, tx: &mut Tx) {
    let v = *s.fixture_run.variant;
    let name = template(v);
    let (title, data) = data(v);
    emit(s, "session.widget.posted", json!({ "messageID": ID, "title": title, "template": { "name": name, "version": version(name) }, "data": data }), tx);
}

fn answer_elsewhere(s: &mut App, tx: &mut Tx) {
    let response = match *s.fixture_run.variant {
        1 => json!({ "name": "preview-reconnect", "replicas": 3, "region": "iad", "checks": ["lint", "unit", "e2e"], "canary": true, "notes": "" }),
        2 => json!({ "confirmed": true }),
        _ => json!({ "choice": "Coastline", "index": 0, "note": "Crop it a little tighter" }),
    };
    emit(s, "session.widget.responded", json!({ "messageID": ID, "status": "submitted", "response": response }), tx);
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Widgets", KEYS).knobs(crate::stories::ROW_KNOBS).variants(VARIANTS),
]);

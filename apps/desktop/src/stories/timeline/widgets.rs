//! The storybook's Widgets story (Timeline › Widgets): the agent calls the `widget` tool, the
//! widgets plugin's `changed` event brings its state, and it runs in the call's place in the
//! timeline on the Rust PsychoScript, with the built-in widget templates (`suggestions`, `form`,
//! `confirm`) and their fixture data. Variants: each built-in, one another client already
//! answered (read-only, its final view and answer), a widget template whose `init` throws (the error
//! card), the `chart` canvas (bars, and a line you can choose a point on), the `backdrop`
//! shader (plasma, aurora), a shader whose WGSL doesn't compile, and live programs, each on a
//! thread of its own. R replays; E answers it from another client, D dismisses it from one. Pictures
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
    ("Broken widget template", 4),
    ("Chart", 5),
    ("Line chart", 6),
    ("Backdrop", 7),
    ("Backdrop aurora", 8),
    ("Broken shader", 9),
    ("Live runaway", 11),
    ("Full screen", 12),
    ("Live shader", 13),
    ("Keyboard and pointer", 14),
    ("Live shader, rebuilt", 15),
    ("Superseded, widget template calls", 16),
    ("Live program that fails", 17),
    ("Failed on another client", 18),
    ("Failed, the agent told", 19),
    ("Drag", 20),
    ("Drag, full screen", 21),
    ("Keyboard and pointer, full screen", 22),
    ("Treemap, full screen", 23),
    ("Heatmap, full screen", 24),
];
pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'e', label: "Answer it from another client", run: |s, tx| {
        tx.clear();
        answer_elsewhere(s, tx);
    } },
    Trigger { key: 'f', label: "Full screen", run: |s, tx| {
        tx.clear();
        fullscreen(s, tx);
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
/// A live widget template granted the network that, half a second in, runs without awaiting: the 50 ms
/// rule stops it, and it shows its last view with why and Restart.
const RUNAWAY: &str = "story-runaway";
/// A live program that drives a shader game: it renders the shader again every frame with new
/// `values` (the paddle under the pointer, the time), which reach the GPU without a build.
const SHADER_GAME: &str = "story-shader-game";
const SHADER_GAME_SOURCE: &str = r##"const WGSL = `fn shade(frag: vec2f) -> vec4f {
  let uv = frag / widget.resolution;
  let x = widget.values[0].x;
  let t = widget.values[0].y;
  let bar = select(0.0, 1.0, abs(uv.x - x) < 0.07 && uv.y > 0.86 && uv.y < 0.92);
  let ball = vec2f(0.5 + 0.42 * sin(t * 1.3), 0.45 + 0.35 * sin(t * 2.1));
  let d = distance(uv * vec2f(widget.resolution.x / widget.resolution.y, 1.0), ball * vec2f(widget.resolution.x / widget.resolution.y, 1.0));
  let glow = smoothstep(0.06, 0.0, d);
  let bg = vec3f(0.04, 0.06, 0.14) + vec3f(0.12, 0.04, 0.2) * uv.y;
  return vec4f(bg + vec3f(0.95, 0.6, 0.2) * bar + vec3f(0.35, 0.85, 1.0) * glow, 1.0);
}`

async function main(data) {
  let x = 0.5
  const onPointer = (p) => {
    if (p.x !== undefined) x = p.x / p.width
  }
  while (true) {
    const frame = await widget.frame()
    widget.render(
      ui.column({ gap: 1 }, [
        ui.text("Shader paddle", { style: "title" }),
        ui.shader(WGSL, { key: "court", height: 220, values: [x, frame.t, 0, 0], alt: "A paddle and a ball", onPointer }),
        ui.text(data.clock ? "t = " + frame.t.toFixed(2) : "Drag on the court to move the paddle.", { style: "caption", tone: "muted" }),
      ]),
    )
  }
}
"##;

/// A live program that takes the keyboard and the pointer (`widgets-input.md`): arrows or WASD
/// and the captured pointer's relative moves steer a dot; it shows the last event it heard.
const INPUT: &str = "story-input";
const INPUT_SOURCE: &str = r##"async function main(data) {
  const s = { x: 160, y: 90, focus: false, capture: false, last: "Click the pad, then use the arrows or move the mouse" }
  widget.on("focus", (on) => {
    s.focus = on
    show(s)
  })
  widget.on("capture", (on) => {
    s.capture = on
    show(s)
  })
  widget.on("key", (key) => {
    if (key.kind === "down") {
      const k = key.key
      if (k === "ArrowLeft" || k === "a") s.x -= 12
      if (k === "ArrowRight" || k === "d") s.x += 12
      if (k === "ArrowUp" || k === "w") s.y -= 12
      if (k === "ArrowDown" || k === "s") s.y += 12
    }
    s.last = "key " + key.kind + " " + key.key + " (" + key.code + ")"
    show(s)
  })
  s.onPointer = (p) => {
    if (p.dx !== undefined) {
      s.x += p.dx
      s.y += p.dy
      s.last = "move dx " + p.dx.toFixed(0) + " dy " + p.dy.toFixed(0)
    } else s.last = "pointer " + p.kind + (p.button !== undefined ? " button " + p.button : "")
    show(s)
  }
  widget.capturePointer()
  show(s)
}

function show(s) {
  widget.render(
    ui.column({ gap: 1 }, [
      ui.text("Keyboard and pointer", { style: "title" }),
      ui.canvas({ key: "pad", height: 180, alt: "A dot to steer", onPointer: s.onPointer }),
      ui.text((s.focus ? "Focused" : "Not focused") + " · " + (s.capture ? "pointer captured" : "pointer free") + " · " + s.last, { style: "caption", tone: "muted" }),
    ]),
  )
  const g = ui.draw()
  g.rrect(0, 0, 2000, 180, 0, "surface")
  g.circle(s.x, s.y, 10, s.capture ? "accent" : "#f59e0b")
  widget.draw("pad", g)
}
"##;

/// A live program written for the old API: it renders a shader, then calls `widget.next`, which
/// the prelude no longer has, and fails; it stays failed, with Restart, until pressed.
const FAILING: &str = "story-failing";
const FAILING_SOURCE: &str = r##"async function main(data) {
  widget.render(
    ui.column({ gap: 1 }, [
      ui.text("Neon gate", { style: "title" }),
      ui.shader("fn shade(frag: vec2f) -> vec4f { let uv = frag / widget.resolution; return vec4f(uv.x * 0.3, 0.1, 0.4 + 0.3 * uv.y, 1.0); }", { height: 160, alt: "A gate" }),
      ui.text("Steer with the pointer.", { style: "caption", tone: "muted" }),
    ]),
  )
  while (true) {
    const next = await widget.next({ frame: true })
  }
}
"##;

/// A shader and a canvas that count the pointer events their widget template hears, for a drag
/// inline and full screen.
const DRAG: &str = "story-drag";
const DRAG_SOURCE: &str = r##"async function main(data) {
  const s = { orb: { down: 0, move: 0, up: 0, x: 0 }, pad: { down: 0, move: 0, up: 0, x: 0 } }
  const on = (k) => (p) => {
    s[k][p.kind] = (s[k][p.kind] || 0) + 1
    s[k].x = p.x
    show(s)
  }
  s.onOrb = on("orb")
  s.onPad = on("pad")
  show(s)
}

function line(name, c) {
  return name + ": down " + c.down + " · move " + c.move + " · up " + c.up + " · x " + Math.round(c.x)
}

function show(s) {
  widget.render(
    ui.column({ gap: 1 }, [
      ui.text("Drag", { style: "title" }),
      ui.shader("fn shade(frag: vec2f) -> vec4f { let uv = frag / widget.resolution; return vec4f(0.15 + 0.5 * uv.x, 0.2, 0.5 - 0.3 * uv.y, 1.0); }", { key: "orb", height: 160, fill: false, alt: "A shader to drag on", onPointer: s.onOrb }),
      ui.canvas({ key: "pad", height: 100, fill: false, alt: "A canvas to drag on", onPointer: s.onPad }),
      ui.text(line("shader", s.orb), { style: "body" }),
      ui.text(line("canvas", s.pad), { style: "body" }),
    ]),
  )
  const g = ui.draw()
  g.rrect(0, 0, 2000, 100, 0, "surface")
  g.circle(s.pad.x, 50, 10, "accent")
  widget.draw("pad", g)
}
"##;

/// A canvas that fills the screen in full screen, and a layout that changes when it goes there;
/// it outlines the close button's area its frame reports, and its title says the one its
/// `onFullscreen` heard.
const FULLSCREEN: &str = "story-fullscreen";
const FULLSCREEN_SOURCE: &str = r##"function init(data) {
  return { full: false, close: null }
}

function view(state, data) {
  return ui.column({ gap: 2 }, [
    ui.text(state.full ? "Starfield, full screen · close " + JSON.stringify(state.close) : "Starfield", { style: "title" }),
    ui.canvas({ key: "sky", height: 180, fps: 30, alt: "A starfield" }),
    ui.canvas({ key: "bar", height: 28, fill: false, alt: "A status bar that keeps its height" }),
    !state.full && ui.text("Hover the widget and press the expand button.", { style: "caption", tone: "muted" }),
  ])
}

function onFullscreen(state, on, close) {
  return { ...state, full: on, close }
}

function draw(state, data, frame) {
  const g = ui.draw()
  if (frame.key === "bar") {
    g.rrect(0, 0, frame.width, frame.height, 6, "surface")
    g.text(12, 19, (frame.fullscreen ? "Full screen" : "Inline") + " · this bar keeps its 28 pt (fill: false)", { size: 12, color: "muted" })
    return g
  }
  g.rect(0, 0, frame.width, frame.height, { fill: { linear: [0, 0, 0, frame.height], stops: [[0, "#0b1026"], [1, "#3b1d5c"]] } })
  for (let i = 0; i < data.stars; i++) {
    const x = (Math.sin(i * 12.9898) * 43758.5453) % 1 + (Math.sin(i * 12.9898) < 0 ? 1 : 0)
    const y = Math.abs(Math.sin(i * 78.233) * 12543.137) % 1
    const twinkle = 0.5 + 0.5 * Math.sin(frame.t * 2 + i)
    g.alpha(0.4 + 0.6 * twinkle).circle(x * frame.width, y * frame.height, 0.6 + (i % 3) * 0.6, "#ffffff")
  }
  g.alpha(1)
  if (frame.close) g.rrect(frame.close.x, frame.close.y, frame.close.width, frame.close.height, 4, { stroke: "#ff4d6d", width: 2 })
  const words = (frame.fullscreen ? "Full screen" : "Inline") + " · " + Math.round(frame.width) + " × " + Math.round(frame.height)
  g.text(frame.width / 2, frame.height / 2, words, { size: frame.fullscreen ? 28 : 16, weight: 600, align: "center", color: "#ffffff" })
  return g
}
"##;
/// Where the story's pictures say they are; [`serve`] answers for them.
const PICTURES: &str = "https://pictures.story.example/";

/// A version for a fixture widget template: any 16 hex digits do, as long as each widget template has its own.
fn version(name: &str) -> String {
    let n = name.bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)));
    format!("{n:016x}")
}

fn widget_template(v: usize) -> &'static str {
    match v {
        1 => "form",
        2 => "confirm",
        4 => BROKEN,
        5 | 6 => "chart",
        7 | 8 => "backdrop",
        9 => BROKEN_SHADER,
        11 => RUNAWAY,
        12 => FULLSCREEN,
        13 => SHADER_GAME,
        14 | 22 => INPUT,
        15 => SHADER_GAME,
        16 | 18 | 19 => "confirm",
        17 => FAILING,
        20 | 21 => DRAG,
        23 => "treemap",
        24 => "heatmap",
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
        2 | 18 | 19 => ("Delete the branch", json!({
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
        11 => ("Price ticker", json!({})),
        12 => ("Starfield", json!({ "stars": 140 })),
        13 => ("Shader paddle", json!({})),
        14 | 22 => ("Keyboard and pointer", json!({})),
        // The same game with a caption that changes every frame: every frame builds the widget
        // again, the cost the values-only path saves.
        15 => ("Shader paddle", json!({ "clock": true })),
        17 => ("Neon gate", json!({})),
        20 | 21 => ("Drag", json!({})),
        23 => ("Treemap", fixtures::example("treemap").unwrap_or_default()),
        24 => ("Heatmap", fixtures::example("heatmap").unwrap_or_default()),
        16 => ("Delete the branch", json!({ "title": "Delete feature/reconnect-backoff?", "message": "The branch is merged into `main`." })),
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
    preload(s, SHADER_GAME, &version(SHADER_GAME), &format!("{}\n{SHADER_GAME_SOURCE}", fixtures::PRELUDE), "live", &Value::Null);
    preload(s, INPUT, &version(INPUT), &format!("{}\n{INPUT_SOURCE}", fixtures::PRELUDE), "live", &Value::Null);
    preload(s, DRAG, &version(DRAG), &format!("{}\n{DRAG_SOURCE}", fixtures::PRELUDE), "live", &Value::Null);
    preload(s, FAILING, &version(FAILING), &format!("{}\n{FAILING_SOURCE}", fixtures::PRELUDE), "live", &Value::Null);
    preload(s, FULLSCREEN, &version(FULLSCREEN), &format!("{}\n{FULLSCREEN_SOURCE}", fixtures::PRELUDE), "step", &Value::Null);
    tx.after(0.3, |s, tx| post(s, tx));
    if *s.fixture_run.variant == 3 {
        tx.after(0.9, |s, tx| answer_elsewhere(s, tx));
    }
    // Replaced by a newer widget: one quiet line where it was.
    if *s.fixture_run.variant == 16 {
        tx.after(0.9, |s, tx| changed(s, "superseded", Value::Null, tx));
    }
    if matches!(*s.fixture_run.variant, 12 | 21 | 22 | 23 | 24) {
        tx.after(1.8, |s, tx| fullscreen(s, tx));
    }
}

/// The agent's step: it calls `widget`, and the plugin publishes the widget it posted.
fn post(s: &mut App, tx: &mut Tx) {
    let v = *s.fixture_run.variant;
    let name = widget_template(v);
    let (title, data) = data(v);
    let input = json!({ "widgetTemplate": name, "data": data, "title": title });
    emit(s, "session.step.started", json!({ "assistantMessageID": ID, "agent": "build", "model": { "id": "claude-opus-5-5", "providerID": "anthropic" } }), tx);
    // The agent worked on its widget template first: each `widget_template` call is one quiet line.
    if v == 16 {
        let calls = [
            ("call_tpl_list", json!({ "action": "list" }), json!({ "action": "list", "count": 23 })),
            ("call_tpl_get", json!({ "action": "get", "name": "neon-runner" }), json!({ "action": "get", "name": "neon-runner", "version": "0123456789abcdef", "revision": 2, "lines": 180 })),
            ("call_tpl_edit", json!({ "action": "edit", "name": "neon-runner" }), json!({ "action": "edit", "name": "neon-runner", "version": "1123456789abcdef", "revision": 3, "lines": 184 })),
        ];
        for (id, input, metadata) in calls {
            emit(s, "session.tool.input.started", json!({ "assistantMessageID": ID, "id": id, "name": "widget_template" }), tx);
            emit(s, "session.tool.called", json!({ "assistantMessageID": ID, "id": id, "name": "widget_template", "input": input, "executed": true }), tx);
            emit(s, "session.tool.success", json!({ "assistantMessageID": ID, "id": id, "content": [{ "type": "text", "text": "…" }], "metadata": metadata, "executed": true }), tx);
        }
    }
    emit(s, "session.tool.input.started", json!({ "assistantMessageID": ID, "id": CALL, "name": "widget" }), tx);
    emit(s, "session.tool.called", json!({ "assistantMessageID": ID, "id": CALL, "name": "widget", "input": input, "executed": true }), tx);
    changed(s, "open", Value::Null, tx);
    let text = format!("Widget \"{title}\" is shown to the user.");
    emit(s, "session.tool.success", json!({ "assistantMessageID": ID, "id": CALL, "content": [{ "type": "text", "text": text }], "metadata": { "widgetTemplate": { "name": name, "version": version(name) } }, "executed": true }), tx);
    emit(s, "session.step.ended", json!({ "assistantMessageID": ID, "finish": "stop", "cost": 0.0, "tokens": { "input": 8, "output": 40, "reasoning": 0, "cache": { "read": 0, "write": 0 } } }), tx);
}

/// The plugin's `changed` event for the story's widget, now `status`.
fn changed(s: &mut App, status: &str, response: Value, tx: &mut Tx) {
    let v = *s.fixture_run.variant;
    let name = widget_template(v);
    let (title, _) = data(v);
    let sid = crate::stories::sid(s);
    let mut state = json!({
        "sessionID": sid, "messageID": ID, "callID": CALL, "widgetTemplate": { "name": name, "version": version(name) },
        "title": title, "status": status, "wait": false, "time": { "created": 0 },
    });
    if status == "submitted" {
        state["response"] = response;
    }
    // A program error another client recorded: not told yet (18), or told (19).
    if status == "open" && matches!(v, 18 | 19) {
        state["failure"] = json!({ "phase": "event", "message": "TypeError: undefined is not an object (evaluating 'state.branch.name')", "line": 42, "column": 17, "client": "ios", "sent": v == 19 });
    }
    emit(s, opencode_kit::model::WIDGET_CHANGED_EVENT, json!({ "sessionID": sid, "state": state }), tx);
}

/// The story's widget goes full screen, as its expand button takes it.
fn fullscreen(s: &mut App, tx: &mut Tx) {
    let now = tx.clock();
    let key = opencode_kit::model::widget_key(ID, CALL);
    crate::session::widgets::fullscreen::send(s, crate::session::widgets::fullscreen::ScreenEvent::Enter(key), now, tx);
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

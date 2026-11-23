//! Checks that a view or draw read straight from the VM's value (`Node::from_host`,
//! `DrawOutput::from_host`, what a live `widget.render` / `widget.draw` post now uses) reads
//! exactly as its JSON did (`Node::from_value`, `DrawOutput::from_value` on `HostValue::to_json`):
//! the same node or commands, or the same error. Values come from the real VM through a host
//! function, as `__host.post` gets them: every built-in's view and draw on its example data, and
//! values made to break the rules. The repo has no automated tests (AGENTS.md), so this is a
//! program to run by hand: `cargo run -p opencode-widget --example host_value_check` (exits
//! non-zero on a difference).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::print_stdout)]

use std::sync::{Arc, Mutex};

use opencode_widget::draw::DrawOutput;
use opencode_widget::{Node, fixtures};
use psychoscript::{HostFunction, HostValue, Runtime};

/// Values made to break the rules: each `view(...)` is read as a view, each `draw(...)` as a
/// draw's commands.
const EDGES: &str = r##"
view(ui.text("x", { style: "fancy", tone: undefined }))
view({ type: "text" })
view({ type: "text", text: 5 })
view({ type: "button", id: "0", label: 5 })
view({ type: "button", label: "no id" })
view({ type: "column", children: [ui.text("a"), , ui.text("b")] })
view({ type: "column", children: [ui.text("a"), undefined, null] })
view({ type: "canvas", height: NaN })
view({ type: "canvas", height: Infinity })
view({ type: "canvas", height: 1e16 })
view({ type: "canvas", height: -0 })
view({ type: "canvas", height: 100, values: [1, 2] })
view({ type: "canvas", height: 100, wgsl: "x".repeat(17000) })
view({ type: "canvas", height: 100, wgsl: "fn shade(f: vec2f) -> vec4f { return vec4f(1.0); }", values: [1, NaN, 3] })
view({ type: "slider", id: "s", value: Infinity, min: 0, max: 1 })
view({ type: "slider", id: "s", value: 0.5, min: 0, max: 1, step: 1e20 })
view({ type: "image", src: new Date(0) })
view({ type: "image", src: "a.png", aspect: new Map() })
view({ type: "card", children: new Map() })
view({ type: "row", gap: 2.5, children: [] })
view({ type: "row", gap: 2, align: "stretch", wrap: "yes", children: [] })
view({ type: "column", gap: 300, children: [] })
view({ type: "column", fullscreen: null, capture: "pointer", children: [] })
view({ type: "select", id: "s", options: [{ value: "a" }, "b"], value: ["a"], multiple: true })
view({ type: "select", id: "s", options: [{ value: "a", label: "A" }], value: 3 })
view({ type: "input", id: "i", value: undefined })
view({ type: "carousel", index: 1.5, children: [] })
view({ type: "carousel", index: -1, children: [] })
view({ type: "stack", children: [ui.text("a", { place: "fill", inset: 4 }), ui.text("b", { place: "nowhere" })] })
view({ type: "stack", children: [ui.text("a", { inset: -1 })] })
view({ type: "stack", children: [ui.text("a", { inset: { top: "4" } })] })
view({ type: "stack", children: [ui.text("a", { inset: { top: 4, left: NaN } })] })
view({ type: "somethingnew", children: [] })
view({ type: 7 })
view({ children: [] })
view(42)
view("text")
view(null)
view(undefined)
view([ui.text("a")])
view({ type: "text", text: "a", extra: new Set([1, 2]), more: [1, , 3] })
view({ type: "text", text: "\u00e9\u{1F600}" })
draw([["rect", 0, 0, 10, 10, { fill: "#fff" }]])
draw([["rect", 0, 0, NaN, 10, { fill: "#fff" }]])
draw([["rect", 0, 0, 10, 10, { fill: "nope" }]])
draw([["rect", 0, 0, 10, 10, "#fff"]])
draw([["rect", 0, 0, 10, 10, { fill: null, stroke: "#000", width: undefined, cap: "round" }]])
draw([["rect", 0, 0, 10, 10, { fill: new Date(0) }]])
draw([["text", 1, 2, { a: 1 }]])
draw([["text", 1, 2, undefined]])
draw([["text", 1, 2]])
draw([["text", 0, 0, "x", { size: NaN, weight: 450.5, align: "center", color: "#f00", mono: true }]])
draw([["text", 0, 0, "x", { size: 20, weight: 1e16, align: 3 }]])
draw([["text", 0, 0, "x", { weight: -100 }]])
draw([["path", [["M", 0, 0], ["L", 1, 1], ["Q", 1, 2, 3, 4], ["C", 1, 2, 3, 4, 5, 6], ["Z"]], { stroke: "#000" }]])
draw([["path", [["M", 0], ["L", 1, 1]], { stroke: "#000" }]])
draw([["path", [["X", 0, 0]], { stroke: "#000" }]])
draw([["path", "M0 0", { stroke: "#000" }]])
draw([["circle", 0, 0, 5, { fill: { linear: [0, 0, 1, 1], stops: [[0, "#fff"], [2, "#000"]] } }]])
draw([["circle", 0, 0, 5, { fill: { radial: [0, 0, 1], stops: [[0, "#fff"]] } }]])
draw([["circle", 0, 0, 5, { fill: { linear: [0, 0, 1], stops: [] } }]])
draw([["circle", 0, 0, 5, { fill: { stops: [[0, "#fff"]] } }]])
draw([["circle", 0, 0, 5, { fill: { linear: [0, 0, 1, 1] } }]])
draw([["circle", 0, 0, 5, { fill: { linear: [0, 0, 1, 1], stops: [[0, "#fff", 3]] } }]])
draw([["image", "a.png", 0, 0, 10, 10], ["image", 5, 0, 0, 1, 1]])
draw([["save"], ["translate", 1, 2], ["rotate", 0.5], ["scale", 2, 2], ["alpha", 3], ["restore"]])
draw([["line", 0, 0, 1, 1, { stroke: "#000" }], ["rrect", 0, 0, 1, 1, 2, { fill: "accent" }]])
draw([["bogus"]])
draw([["rect"]])
draw([[]])
draw([[1, 2, 3]])
draw([, ["save"]])
draw([undefined])
draw(new Map())
draw({ not: "an array" })
draw(undefined)
draw(Array.from({ length: 2001 }, () => ["save"]))
"##;

/// A widget template's view and draw on its example data, each read as above.
const BUILTIN: &str = r##"
const state = typeof init === "function" ? init(DATA) ?? {} : {}
built(view(state, DATA))
if (typeof draw === "function") {
  for (const width of [320, 640]) {
    const frame = { key: null, t: 5, dt: 0.016, width, height: 240, scale: 2, pointer: { x: 10, y: 10, down: false }, dark: true, fullscreen: false, close: null }
    // A draw that needs a canvas key (a table's sparklines) throws here: not what's checked.
    let out
    try { out = draw(state, DATA, frame) } catch (e) { continue }
    draw$(out && out.commands !== undefined ? out.commands : out)
  }
}
"##;

#[derive(Clone)]
enum Seen {
    View(HostValue),
    /// A view as the program built it, which the host makes into its tree first.
    Built(HostValue),
    Draw(Option<HostValue>),
}

/// Runs `code` (with `view(v)` and `draw$(v)` as the host functions it hands values to), and the
/// values it handed over.
fn run(code: &str) -> Vec<Seen> {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let (views, draws, builts) = (seen.clone(), seen.clone(), seen.clone());
    let translate = psychoscript::Translate { functions: psychoscript::TranslateFunctions::Mark, ..Default::default() };
    let runtime = Runtime::make().typed_arrays(true).translate(translate).globals(move |_run| {
        let (views, draws, builts) = (views.clone(), draws.clone(), builts.clone());
        [
            ("__built", HostFunction::new(move |args: Vec<HostValue>, _call| {
                builts.lock().unwrap().push(Seen::Built(args.into_iter().next().unwrap_or(HostValue::Undefined)));
                Ok(HostValue::Undefined)
            })),
            ("__view", HostFunction::new(move |args: Vec<HostValue>, _call| {
                views.lock().unwrap().push(Seen::View(args.into_iter().next().unwrap_or(HostValue::Undefined)));
                Ok(HostValue::Undefined)
            })),
            ("__draw", HostFunction::new(move |args: Vec<HostValue>, _call| {
                draws.lock().unwrap().push(Seen::Draw(args.into_iter().next()));
                Ok(HostValue::Undefined)
            })),
        ]
    });
    let outcome = runtime.execute(code);
    if let Err(d) = outcome.json() {
        panic!("the check's program failed: {}", d.message_lossy());
    }
    seen.lock().unwrap().clone()
}

/// The values read both ways; the cases that differ.
fn compare(name: &str, seen: &[Seen]) -> (usize, Vec<String>) {
    let mut bad = Vec::new();
    for (i, s) in seen.iter().enumerate() {
        let built;
        let s = match s {
            Seen::Built(v) => {
                built = Seen::View(opencode_widget::tree::build(v.clone(), "Argument 1 to __built").expect("a built-in's view builds").view);
                &built
            }
            s => s,
        };
        let (direct, json) = match s {
            Seen::View(v) => (format!("{:?}", Node::from_host(v)), format!("{:?}", Node::from_value(&v.to_json().unwrap_or_default()))),
            Seen::Built(_) => unreachable!("made into a view above"),
            Seen::Draw(v) => {
                let json = v.as_ref().and_then(HostValue::to_json);
                (format!("{:?}", DrawOutput::from_host(v.as_ref())), format!("{:?}", DrawOutput::from_value(&serde_json::json!({ "commands": json }))))
            }
        };
        if direct != json {
            bad.push(format!("{name} #{i}:\n  direct {direct}\n  json   {json}"));
        }
    }
    (seen.len(), bad)
}

fn main() {
    let shim = "const view = __view; const draw$ = __draw; const draw = __draw;\n";
    let mut checked = 0;
    let mut bad = Vec::new();
    let edges = run(&format!("{}\n{shim}{EDGES}", fixtures::PRELUDE));
    let (n, b) = compare("edges", &edges);
    println!("edges: {n} values");
    checked += n;
    bad.extend(b);
    let mut skipped = Vec::new();
    for name in fixtures::BUILTINS {
        let Some(data) = fixtures::example(name) else {
            skipped.push(name);
            continue;
        };
        let code = format!(
            "{}\n{}\nconst DATA = {data};\nconst __v = __view; const draw$ = __draw;\n{}",
            fixtures::PRELUDE,
            fixtures::source(name).expect("a built-in"),
            BUILTIN.replace("built(view(", "__built(view(")
        );
        let seen = run(&code);
        let (n, b) = compare(name, &seen);
        checked += n;
        bad.extend(b);
    }
    println!("{checked} values read both ways (built-ins without example data, not run: {})", skipped.join(", "));
    if !bad.is_empty() {
        for b in &bad {
            println!("{b}");
        }
        std::process::exit(1);
    }
    println!("ok   every view and draw reads the same from the VM's value as from its JSON");
}

//! Checks a live run as a native host (`widgets.md`, Native hosts): `widget.draw` by key reaches
//! the canvas the last render gave that key, an unknown key or a GPU canvas's throws the spec's
//! errors (which the program can catch, and which fail it when it doesn't), and `render` tells the
//! prelude whether the view has handlers (a run without any ends when `main` returns). A render
//! breaking Keys and depth ends the run at once in phase `view`; a function anywhere but a view's
//! handler props throws the runtime's `TypeError` from that call; a program built for another
//! protocol (`widgets.md`, Protocol) isn't run, step or live. The repo
//! has no automated tests (AGENTS.md), so this is a program to run by hand:
//! `cargo run -p opencode-widget --example live_native_check` (exits non-zero on a failure).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::print_stdout)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use opencode_widget::live::{Ended, Grants, Live, LiveProgram, Post, Setup};
use opencode_widget::{Node, fixtures};
use serde_json::json;

/// Draws by key, then reports the errors the two bad draws throw, as text.
const DRAWS: &str = r#"
async function main(data) {
  widget.render(ui.column([ui.canvas({ key: "pad", height: 10 }), ui.canvas({ key: "gpu", height: 10, wgsl: "fn shade(f: vec2f) -> vec4f { return vec4f(1.0); }" }), ui.button("go", { onPress: () => {} })]))
  widget.draw("pad", ui.draw().rect(0, 0, 4, 4, "accent"))
  const seen = []
  for (const key of ["nope", "gpu"]) {
    try { widget.draw(key, ui.draw()) } catch (e) { seen.push(e.message) }
  }
  widget.render(ui.column([ui.text(seen.join(" | ")), ui.button("go", { onPress: () => {} })]))
}
"#;

const UNCAUGHT: &str = r#"
async function main(data) {
  widget.render(ui.text("no canvas"))
  widget.draw("nope", ui.draw())
}
"#;

/// A view without handlers: `main` returning ends the run.
const QUIET: &str = r#"
async function main(data) {
  widget.render(ui.column({ key: "root" }, [ui.text("done"), ui.canvas({ key: "c", height: 4 })]))
}
"#;

/// A view with a handler: the run stays after `main` returns.
const INTERACTIVE: &str = r#"
async function main(data) {
  widget.render(ui.column([ui.button("press", { key: "b", onPress: () => widget.render(ui.text("pressed")) })]))
}
"#;

/// Renders a view with a key of another type, after catching nothing.
const BAD_KEY: &str = r#"
async function main(data) {
  try { widget.render(ui.column([ui.text("x", { key: { a: 1 } })])) } catch (e) { widget.render(ui.text("caught " + e.message)) }
  await widget.sleep(1000)
}
"#;

/// Renders a view of 120 levels: too deep to cross, so the prelude posts its failure.
const DEEP: &str = r#"
async function main(data) {
  let node = ui.text("leaf")
  for (let i = 0; i < 120; i++) node = ui.column([node])
  widget.render(node)
  await widget.sleep(1000)
}
"#;

/// Puts functions where none may go, catching each error, then shows what each threw.
const FUNCTIONS: &str = r#"
async function main(data) {
  const seen = []
  const tries = [
    () => widget.render(ui.column([ui.text("x", { extra: () => 1 })])),
    () => widget.send({ f: () => 1 }),
    () => __host.post({ type: "frame" }, () => 1),
    () => widget.sleep(() => 1),
    () => widget.fetch("https://a.test", { headers: { h: () => 1 } }),
  ]
  for (const t of tries) {
    // The prelude's fetch hands its errors on as `{ kind, message }`.
    try { await t() } catch (e) { seen.push((e.name ?? e.kind) + ": " + e.message) }
  }
  widget.render(ui.column([ui.text(seen.join(" | ")), ui.button("go", { onPress: () => {} })]))
}
"#;

fn start(source: &str) -> Live {
    start_program(&format!("{}\n{source}", fixtures::PRELUDE))
}

fn start_program(program: &str) -> Live {
    let program = Arc::new(LiveProgram::compile(program).expect("compiles"));
    Live::start(program, json!({}), Setup { grants: Grants::default(), transport: None, allow_local_network: false, notify: Arc::new(|| {}) })
}

/// The posts `live` makes within `within`, and how it ended by then.
fn posts(live: &Live, within: Duration) -> (Vec<Post>, Option<Ended>) {
    let end = Instant::now() + within;
    let mut out = Vec::new();
    while Instant::now() < end {
        out.extend(live.take());
        if let Some(e) = live.ended() {
            out.extend(live.take());
            return (out, Some(e));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    (out, None)
}

fn main() {
    let (seen, _) = posts(&start(DRAWS), Duration::from_millis(800));
    let draws: Vec<_> = seen.iter().filter_map(|p| match p { Post::Draw { id, commands } => Some((id.clone(), commands.len())), _ => None }).collect();
    assert_eq!(draws, vec![("0/:pad".to_owned(), 1)], "a draw by key reaches its canvas's id");
    let text = seen.iter().rev().find_map(|p| match p {
        Post::Render(Node::Column { children, .. }) => children.iter().find_map(|c| match c { Node::Text { text, .. } => Some(text.clone()), _ => None }),
        _ => None,
    });
    let want = r#"No canvas with key "nope" in the rendered view | Canvas "gpu" is drawn by its wgsl; it takes no draw commands"#;
    assert_eq!(text.as_deref(), Some(want), "the bad draws throw the spec's errors, which the program catches");
    println!("ok   a draw by key reaches its canvas; bad keys throw the spec's errors");

    let (_, end) = posts(&start(UNCAUGHT), Duration::from_secs(2));
    match end {
        Some(Ended::Failed(m)) if m.contains("No canvas with key \"nope\" in the rendered view") => println!("ok   an uncaught bad draw fails the run: {m}"),
        other => panic!("an uncaught bad draw should fail the run, got {other:?}"),
    }

    let (_, end) = posts(&start(QUIET), Duration::from_secs(2));
    assert_eq!(end, Some(Ended::Returned), "a view without handlers ends with main");
    println!("ok   a view without handlers ends with main");

    let live = start(INTERACTIVE);
    let (seen, end) = posts(&live, Duration::from_millis(600));
    assert_eq!(end, None, "a view with a handler keeps the run");
    let id = seen.iter().find_map(|p| match p {
        Post::Render(Node::Column { children, .. }) => children.iter().find_map(|c| match c { Node::Button { id, .. } => Some(id.clone()), _ => None }),
        _ => None,
    });
    assert_eq!(id.as_deref(), Some("0/:b"), "the button has its keyed id");
    live.event(&opencode_widget::WidgetEvent::press("0/:b"));
    let (seen, _) = posts(&live, Duration::from_millis(600));
    assert!(seen.iter().any(|p| matches!(p, Post::Render(Node::Text { text, .. }) if text == "pressed")), "its handler runs on a press: {seen:?}");
    println!("ok   a view with a handler keeps the run, and its handler runs by its id");

    for (name, source, want) in [("a key of another type", BAD_KEY, opencode_widget::tree::KEY), ("a view of 120 levels", DEEP, opencode_widget::tree::DEEP)] {
        let (seen, end) = posts(&start(source), Duration::from_secs(2));
        assert_eq!(end, Some(Ended::View(want.into())), "{name} ends the run in phase view");
        assert!(!seen.iter().any(|p| matches!(p, Post::Render(_))), "{name}: nothing rendered, nothing caught: {seen:?}");
        println!("ok   {name} ends the run at once, in phase view: {want}");
    }

    let (seen, _) = posts(&start(FUNCTIONS), Duration::from_millis(800));
    let text = seen.iter().rev().find_map(|p| match p {
        Post::Render(Node::Column { children, .. }) => children.iter().find_map(|c| match c { Node::Text { text, .. } => Some(text.clone()), _ => None }),
        _ => None,
    });
    let want = ["post", "post", "post", "sleep", "fetch"]
        .iter()
        .zip([1, 1, 2, 1, 1])
        .map(|(name, n)| format!("{}: Argument {n} to __host.{name} contains a function, which cannot be passed to the host.", if *name == "fetch" { "Network" } else { "TypeError" }))
        .collect::<Vec<_>>()
        .join(" | ");
    assert_eq!(text.as_deref(), Some(want.as_str()), "a function where none may go throws the runtime's TypeError");
    println!("ok   a function where none may go throws the runtime's TypeError, naming the argument");

    let body = "\nasync function main(data) { widget.render(ui.text(\"never\")) }";
    let older = fixtures::PRELUDE.replacen("const __widget_protocol = 2\n", "", 1);
    let newer = fixtures::PRELUDE.replacen("const __widget_protocol = 2", "const __widget_protocol = 3", 1);
    let (seen, end) = posts(&start_program(&format!("{older}{body}")), Duration::from_millis(500));
    assert_eq!((seen.len(), end), (0, Some(Ended::Failed(opencode_widget::runner::OLDER.into()))), "an older protocol is a program error");
    let (seen, end) = posts(&start_program(&format!("{newer}{body}")), Duration::from_millis(500));
    assert_eq!((seen.len(), end), (0, Some(Ended::Broke(opencode_widget::runner::NEWER.into()))), "a newer protocol is this client's");
    let step = |prelude: &str| opencode_widget::Program::compile(&format!("{prelude}\nfunction view() {{ return ui.text(\"x\") }}")).unwrap().step(&opencode_widget::StepInput::Init { data: json!(null) });
    assert_eq!(step(&older), Err(opencode_widget::Error::Older));
    assert_eq!(step(&newer), Err(opencode_widget::Error::Newer));
    println!("ok   programs of another protocol aren't run, step or live: older a program error, newer this client's");
}

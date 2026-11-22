//! Checks a live run as a native host (`widgets.md`, Native hosts): `widget.draw` by key reaches
//! the canvas the last render gave that key, an unknown key or a GPU canvas's throws the spec's
//! errors (which the program can catch, and which fail it when it doesn't), and `render` tells the
//! prelude whether the view has handlers (a run without any ends when `main` returns). The repo
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

fn start(source: &str) -> Live {
    let program = Arc::new(LiveProgram::compile(&format!("{}\n{source}", fixtures::PRELUDE)).expect("compiles"));
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
}

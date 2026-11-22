//! Checks the 32 MB heap cap on widget programs (`widgets-live.md`, Budgets): a step or a live run
//! that holds more stops with "It used more than 32 MB of memory", a program error, and one that
//! stays under it runs on. The repo has no automated tests (AGENTS.md), so this is a program to
//! run by hand: `cargo run -p opencode-widget --example heap_check` (exits non-zero on a failure).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::print_stdout)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use opencode_widget::live::{Ended, Grants, Live, LiveProgram, Setup};
use opencode_widget::runner::{Error, heap_exceeded};
use opencode_widget::{Program, StepInput, fixtures};
use serde_json::json;

/// Keeps two distinct 10-million-character strings in its state: about 40 MB in the size model
/// (garbage never counts, so they must stay reachable).
const STEP_OVER: &str = r#"
function init(data) {
  return { a: "a".repeat(10000000), b: "b".repeat(10000000) }
}
function view(state) { return ui.text(String(state.a.length + state.b.length)) }
"#;

const STEP_UNDER: &str = r#"
function init(data) {
  const a = "a".repeat(1000000)
  return { n: a.length }
}
function view(state) { return ui.text(String(state.n)) }
"#;

/// Keeps a new 2 MB string every millisecond: over the cap within a second.
const LIVE_GROWS: &str = r#"
async function main(data) {
  const kept = []
  widget.render(ui.text("growing"))
  for (let i = 0; ; i++) {
    kept.push("z".repeat(1000000) + i)
    await widget.sleep(1)
  }
}
"#;

/// Keeps the same small state forever.
const LIVE_STEADY: &str = r#"
async function main(data) {
  let n = 0
  while (true) {
    n = (n + 1) % 1000
    widget.render(ui.text("tick " + n))
    await widget.sleep(1)
  }
}
"#;

fn program(source: &str) -> Program {
    Program::compile(&format!("{}\n{source}", fixtures::PRELUDE)).expect("compiles")
}

fn start(source: &str) -> Live {
    let program = Arc::new(LiveProgram::compile(&format!("{}\n{source}", fixtures::PRELUDE)).expect("compiles"));
    Live::start(program, json!({}), Setup { grants: Grants::default(), transport: None, allow_local_network: false, notify: Arc::new(|| {}) })
}

/// How `live` ended within `within`, taking its posts meanwhile; `None` while it still runs.
fn ended(live: &Live, within: Duration) -> Option<Ended> {
    let end = Instant::now() + within;
    while Instant::now() < end {
        live.take();
        if let Some(e) = live.ended() {
            return Some(e);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

fn main() {
    let over = program(STEP_OVER).step(&StepInput::Init { data: json!({}) });
    match &over {
        Err(Error::Script { message, location: None }) if *message == heap_exceeded() => println!("ok   a step over the cap fails: {message}"),
        other => panic!("a step over the cap should fail with the heap error, got {other:?}"),
    }
    let under = program(STEP_UNDER).step(&StepInput::Init { data: json!({}) });
    assert!(under.is_ok(), "a step under the cap runs: {under:?}");
    println!("ok   a step under the cap runs");

    let grows = start(LIVE_GROWS);
    match ended(&grows, Duration::from_secs(10)) {
        Some(Ended::Failed(m)) if m == heap_exceeded() => println!("ok   a live run over the cap fails: {m}"),
        other => panic!("a live run over the cap should fail with the heap error, got {other:?}"),
    }

    let steady = start(LIVE_STEADY);
    assert_eq!(ended(&steady, Duration::from_secs(2)), None, "a live run under the cap runs on");
    println!("ok   a live run under the cap runs on");
}

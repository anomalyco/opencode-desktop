//! Checks the 32 MB heap cap on widget programs (`widgets-live.md`, Budgets): a step or a live run
//! that holds more stops with "It used more than 32 MB of memory", a program error, and one that
//! stays under it runs on, including 100,000 frames of `await widget.frame()` (which grew the heap
//! until PsychoScript freed an async function's coroutine when it returns, rust e4cb862). The repo has no automated tests (AGENTS.md), so this is a program to
//! run by hand: `cargo run -p opencode-widget --example heap_check` (exits non-zero on a failure).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::print_stdout)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use opencode_widget::live::{Ended, Grants, Live, LiveProgram, Post, Setup};
use opencode_widget::runner::{Error, heap_exceeded};
use opencode_widget::{Frame, Program, StepInput, fixtures};
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

/// A game's loop: a frame, a little work on it, the next frame; a render every 1,000 frames.
const LIVE_FRAMES: &str = r#"
async function main(data) {
  let n = 0
  let x = 0
  while (true) {
    const frame = await widget.frame()
    x = (x + frame.dt * 60) % 400
    n++
    if (n % 1000 === 0) widget.render(ui.text("frame " + n + " at " + Math.round(x)))
  }
}
"#;

const FRAMES: usize = 100_000;

fn frame() -> Frame {
    Frame { id: None, t: 0.0, dt: 1.0 / 60.0, width: 400.0, height: 300.0, scale: 2.0, pointer: None, dark: true, fullscreen: false, close: None }
}

/// A frame every 4.5 ms: about 222 a second (a vsync gives at most 120).
const FRAME_GAP: Duration = Duration::from_micros(4500);

/// Answers `live`'s frame requests, no faster than [`FRAME_GAP`], until it had `n` frames or it
/// ended: the frames it had, and how it ended.
fn frames(live: &Live, n: usize, within: Duration) -> (usize, Option<Ended>) {
    let end = Instant::now() + within;
    let (mut given, mut wanted) = (0, false);
    let mut next = Instant::now();
    while given < n && Instant::now() < end {
        wanted |= live.take().contains(&Post::Frame);
        if wanted && Instant::now() >= next {
            live.frame(&frame());
            (wanted, given) = (false, given + 1);
            next += FRAME_GAP;
            if given % 10_000 == 0 {
                println!("     {given} frames");
            }
        }
        if let Some(e) = live.ended() {
            return (given, Some(e));
        }
        std::thread::sleep(Duration::from_micros(200));
    }
    (given, live.ended())
}

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

    let looping = start(LIVE_FRAMES);
    let t = Instant::now();
    let (given, end) = frames(&looping, FRAMES, Duration::from_secs(900));
    assert_eq!(end, None, "a frame loop stays under the cap: it ended after {given} frames");
    assert_eq!(given, FRAMES, "a frame loop gets all its frames");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(looping.ended(), None, "a frame loop stays under the cap after its last frame");
    println!("ok   {FRAMES} frames of await widget.frame() stay under the cap ({:.1} s)", t.elapsed().as_secs_f64());
}

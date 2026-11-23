//! Checks a paused live run against the 50 ms rule: a run at rest behind a pause isn't stopped,
//! its budget starts again when it resumes, and a run that really is busy while paused still is.
//! The repo has no automated tests (AGENTS.md), so this is a program to run by hand:
//! `cargo run -p opencode-widget --example live_pause_check` (exits non-zero on a failure).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::print_stdout)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use opencode_widget::live::{Ended, Grants, Live, LiveProgram, Post, Setup};
use opencode_widget::{Frame, fixtures};
use serde_json::json;

/// Asks for a frame, says it has it, then works on it for about 15 ms before asking again: a
/// pause made when it says so lands before its next request is first polled.
const FRAMES: &str = r#"
async function main(data) {
  let n = 0
  while (true) {
    await widget.frame()
    n++
    widget.render(ui.text("frame " + n))
    let x = 0
    for (let i = 0; i < 500000; i++) x += i % 7
  }
}
"#;

/// Spins without awaiting once it has its first frame (saying so first).
const SPIN: &str = r#"
async function main(data) {
  await widget.frame()
  widget.render(ui.text("spinning"))
  while (true) {}
}
"#;

fn start(source: &str) -> Live {
    let program = Arc::new(LiveProgram::compile(&fixtures::served("live", source)).expect("compiles"));
    Live::start(program, json!({}), Setup { grants: Grants::default(), transport: None, allow_local_network: false, notify: Arc::new(|| {}) })
}

fn frame() -> Frame {
    Frame { id: None, t: 0.0, dt: 1.0 / 60.0, width: 400.0, height: 300.0, scale: 2.0, pointer: None, dark: true, fullscreen: false, close: None }
}

/// Takes posts until `want` counts `n` of them, or `within` passes. How many it saw.
fn wait(live: &Live, within: Duration, n: usize, want: impl Fn(&Post) -> bool) -> usize {
    let end = Instant::now() + within;
    let mut seen = 0;
    while Instant::now() < end && seen < n {
        seen += live.take().iter().filter(|p| want(p)).count();
        if live.ended().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    seen
}

fn frame_asked(p: &Post) -> bool {
    matches!(p, Post::Frame)
}

fn rendered(p: &Post) -> bool {
    matches!(p, Post::Render(_))
}

fn a_run_paused_at_rest_is_not_stopped_and_its_budget_restarts_on_resume() {
    let live = start(FRAMES);
    assert_eq!(wait(&live, Duration::from_secs(5), 1, frame_asked), 1, "asks for its first frame");
    // A frame, and the pause while it works on it: its next frame request is first polled while
    // paused, so the pause alone holds it.
    live.frame(&frame());
    assert_eq!(wait(&live, Duration::from_secs(5), 1, rendered), 1, "has its frame");
    live.pause();
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(live.ended(), None, "a run at rest behind a pause is not stopped");

    live.resume();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(live.ended(), None, "its 50 ms start again on resume, not from the pause");
    // It runs on: it takes frames again.
    for _ in 0..3 {
        live.frame(&frame());
        assert_eq!(wait(&live, Duration::from_secs(5), 1, rendered), 1, "renders after the resume");
    }
    assert_eq!(live.ended(), None);
    live.stop();
}

fn a_run_busy_while_paused_is_still_stopped() {
    let live = start(SPIN);
    assert_eq!(wait(&live, Duration::from_secs(5), 1, frame_asked), 1);
    live.frame(&frame());
    // Paused once it spins: the pause holds nothing it waits for.
    assert_eq!(wait(&live, Duration::from_secs(5), 1, rendered), 1);
    live.pause();
    let end = Instant::now() + Duration::from_secs(5);
    while live.ended().is_none() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(live.ended(), Some(Ended::Failed(_))), "the 50 ms rule holds while paused: {:?}", live.ended());
}

fn main() {
    a_run_paused_at_rest_is_not_stopped_and_its_budget_restarts_on_resume();
    println!("ok: a run paused at rest isn't stopped, and its budget restarts on resume");
    a_run_busy_while_paused_is_still_stopped();
    println!("ok: a run busy while paused is still stopped");
}

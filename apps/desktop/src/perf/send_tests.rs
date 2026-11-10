//! The frame Enter lands on, in a long session: what a send and a steer cost the UI thread.
use super::{fixture, report, settle};
use crate::{App, view};
use rune::{Env, Key, KeyEvent, Mods, Runtime};

const FRAMES: usize = 6;

/// CPU and phases of the Enter frame and the frames after it, for a send (idle session) or a
/// steer (busy session).
fn send_frames(steer: bool) -> Vec<[f64; 7]> {
    let mut app = fixture();
    if steer { app.running.insert("bench".into()); }
    let mut rt = Runtime::new(app, view::app, (1280.0, 820.0), Env::new(true, 2.0));
    rt.poll = Some(crate::poll);
    rt.frame(0.0);
    settle(&mut rt);
    rt.update(|s, _| { s.composer = rune::TextEdit::new("Can you also check the reconnect jitter?"); s.composer.focused = true; });
    settle(&mut rt);
    let mut out = Vec::new();
    for k in 0..FRAMES {
        out.push(super::measured(&mut rt, |rt| {
            if k == 0 { rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() }); }
            rt.frame(1.0 / 120.0);
        }));
    }
    assert!(rt.state.composer.text.is_empty(), "Enter sent the prompt");
    out
}

#[test]
#[ignore = "CPU benchmark; run --profile perf --features frame-profile --ignored --nocapture --test-threads=1"]
fn send_frame_benchmark() {
    assert!(!cfg!(debug_assertions), "use --profile perf");
    for (steer, name) in [(false, "send"), (true, "steer")] {
        let mut runs: Vec<Vec<[f64; 7]>> = Vec::new();
        for sample in 0..8 {
            let frames = send_frames(steer);
            if sample > 1 { runs.push(frames); }
        }
        for k in 0..FRAMES {
            for (i, metric) in super::METRICS.iter().enumerate() {
                report(&format!("{name}_frame{k}_{metric}"), &mut runs.iter().map(|r| r[k][i]).collect::<Vec<_>>());
            }
        }
    }
}

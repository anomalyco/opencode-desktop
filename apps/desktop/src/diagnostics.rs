//! Always-on frame diagnostics for live windowed runs, through Rune's `rune-perf`: one writer
//! thread keeps `perf.json` (a two-second snapshot), capped logs and incident files in
//! `<data dir>/rune-opencode/perf/`. The UI thread only sends a fixed-size record per frame
//! report; it never formats, writes or asks for a frame, so an idle window still draws nothing.
//!
//! Timing is Rune's own (`FrameEncoded::update_ms`, `encode_ms`), so no compositor is attached
//! and the window keeps skipping unchanged timer frames. Input latency runs from the newest input
//! the window dispatched since the last drawn frame (`rune::last_input`) to this frame's encode:
//! a lower bound when several inputs land between frames.
use std::{cell::RefCell, rc::Rc};

use rune::time::Instant;
use rune::{FrameEncoded, FrameOutcome, Runtime};
use rune_perf::{Config, Frame, Outcome, Recorder};

use crate::App;

/// Shared by the window's encoded and drawn hooks, and by `main` to shut the writer down.
#[derive(Clone, Default)]
pub struct Diagnostics(Rc<RefCell<State>>);

#[derive(Default)]
struct State {
    started: bool,
    recorder: Option<Recorder>,
    last_drawn: Option<Instant>,
}

impl Diagnostics {
    /// `App::on_frame_drawn`: starts recording on the first drawn frame of a live session, never
    /// for a fixture, a test or a headless run.
    pub fn drawn(&self, rt: &Runtime<App>) {
        let mut s = self.0.borrow_mut();
        if s.started || !rt.state.live() || crate::headless() {
            return;
        }
        s.started = true;
        s.recorder = start();
    }

    /// `App::on_frame_encoded`.
    pub fn encoded(&self, f: &FrameEncoded) {
        self.record(f.seq(), f.outcome(), f.at(), f.update_ms(), f.encode_ms());
    }

    fn record(&self, seq: u64, outcome: FrameOutcome, at: Instant, update_ms: Option<f64>, encode_ms: Option<f64>) {
        let mut s = self.0.borrow_mut();
        if s.recorder.is_none() {
            return;
        }
        let frame = s.frame(seq, outcome, at, rune::last_input(), update_ms, encode_ms);
        if let Some(r) = &s.recorder {
            r.record(frame);
        }
    }

    /// After `run`: drains what's queued. The window is gone, so nothing waits on it.
    pub fn shutdown(&self) {
        if let Some(r) = self.0.borrow_mut().recorder.take() {
            let _ = r.shutdown();
        }
    }
}

impl State {
    fn frame(&mut self, seq: u64, outcome: FrameOutcome, at: Instant, input: Option<Instant>, update_ms: Option<f64>, encode_ms: Option<f64>) -> Frame {
        let outcome = match outcome {
            FrameOutcome::Drawn => Outcome::Drawn,
            FrameOutcome::Unchanged => Outcome::Unchanged,
            _ => Outcome::Skipped,
        };
        let mut input_ms = None;
        if outcome == Outcome::Drawn {
            input_ms = input
                .filter(|t| self.last_drawn.is_none_or(|d| *t > d))
                .and_then(|t| at.checked_duration_since(t))
                .map(|d| d.as_secs_f64() * 1000.0);
            self.last_drawn = Some(at);
        }
        Frame::new(0, seq, outcome)
            .update_ms(update_ms)
            .render_ms(encode_ms)
            .input_to_frame_ms(input_ms)
            .allocated_bytes(rune_perf::take_allocated_bytes())
    }
}

fn start() -> Option<Recorder> {
    let dir = crate::dirs::logs()?.join("perf");
    let config = Config::new(dir, "OpenCode (Rune)").build(env!("CARGO_PKG_VERSION"));
    match Recorder::start(config) {
        Ok(r) => {
            // The first call only sets this thread's baseline.
            rune_perf::take_allocated_bytes();
            Some(r)
        }
        Err(e) => {
            eprintln!("opencode-rune: frame diagnostics unavailable: {e}");
            None
        }
    }
}


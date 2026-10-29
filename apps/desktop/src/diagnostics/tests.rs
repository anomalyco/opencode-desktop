use std::time::{Duration, Instant};

use rune::FrameOutcome;
use rune_perf::Outcome;

use super::State;

fn json(f: rune_perf::Frame) -> String { format!("{f:?}") }

#[test]
fn input_counts_once_from_the_newest_since_the_last_drawn_frame() {
    let mut s = State::default();
    let t0 = Instant::now();
    let key = t0 + Duration::from_millis(1);
    let ms = |d: u64| t0 + Duration::from_millis(d);
    let first = s.frame(0, FrameOutcome::Drawn, ms(5), Some(key), Some(1.0), Some(0.5));
    assert!(json(first).contains("input_to_frame_ms: Some(4.0"), "{first:?}");
    // Unchanged and skipped frames don't take it, and a drawn frame with no newer input has none.
    let unchanged = s.frame(1, FrameOutcome::Unchanged, ms(9), Some(key), None, None);
    assert!(json(unchanged).contains("input_to_frame_ms: None"), "{unchanged:?}");
    let later = s.frame(2, FrameOutcome::Drawn, ms(13), Some(key), Some(1.0), Some(0.5));
    assert!(json(later).contains("input_to_frame_ms: None"), "the key already counted: {later:?}");
    let next = s.frame(3, FrameOutcome::Drawn, ms(20), Some(ms(18)), Some(1.0), Some(0.5));
    assert!(json(next).contains("input_to_frame_ms: Some(2.0"), "{next:?}");
}

#[test]
fn outcomes_and_rune_timings_pass_through() {
    let mut s = State::default();
    let at = Instant::now();
    let f = s.frame(7, FrameOutcome::Skipped, at, None, None, None);
    let text = json(f);
    assert!(text.contains(&format!("outcome: {:?}", Outcome::Skipped)) && text.contains("seq: 7"), "{text}");
    assert!(text.contains("update_ms: None") && text.contains("render_ms: None"), "{text}");
    let f = s.frame(8, FrameOutcome::Drawn, at, None, Some(1.5), Some(0.25));
    let text = json(f);
    assert!(text.contains("update_ms: Some(1.5)") && text.contains("render_ms: Some(0.25)"), "{text}");
}

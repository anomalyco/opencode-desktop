//! Streaming text grows its row a line at a time, fast (p252): Working sits its lane's gap under
//! the last row at all times, and steps down by exactly one line each time the answer wraps. With
//! Clip, Grow then show and Soft edge, no word ever draws below its row's moving edge or over
//! Working.

use crate::App;
use crate::line_grow_story::{FAST, SLOW, WORDS_AT, play, script, words_end};
use crate::line_reveal::{NEW_LINE_REVEAL, Reveal};
use crate::working::LANE_PAD;

/// One frame at the end of the timeline: Working's top and the bottom of the row over it, in
/// the timeline's own coordinates (scrolling taken out).
struct Look {
    t: f64,
    above: String,
    bottom: f32,
    working: f32,
    /// The lowest any of its words draws, after its clips: its bottom if none draw.
    words: f32,
}

/// Plays the story's run a word every `word` seconds, looking at every 1/120 s frame.
fn run(word: f64, reveal: Reveal) -> Vec<Look> {
    let mut rt = App::fixture_session().into_runtime();
    rt.update(|s, _| s.knobs.set(&NEW_LINE_REVEAL, reveal.knob()));
    rt.frame(0.0);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    rt.update(|s, _| play(s, word));
    let end = script(word).last().unwrap().0 + 1.0;
    let (dt, mut t) = (1.0 / 120.0, 0.0);
    let mut looks = Vec::new();
    while t < end {
        rt.frame(dt);
        t += dt;
        let crate::Route::Session { id: sid, .. } = rt.state.route.clone() else { panic!("no open session") };
        let keys = crate::view::row_keys(&rt.state, &rt.state.transcripts[&sid]);
        let Some(i) = keys.iter().position(|k| k == "turn-actions").filter(|&i| i > 0) else { continue };
        let els = rt.elements();
        let live = |k: &str| els.iter().find(|e| !e.exiting && e.key.as_deref() == Some(k));
        let (Some(above), Some(working)) = (live(&keys[i - 1]), live("working")) else { continue };
        let row = els.iter().position(|e| std::ptr::eq(e, above)).unwrap();
        let inside = |mut k: usize| loop {
            match els[k].parent {
                Some(p) if p == row => break true,
                Some(p) => k = p,
                None => break false,
            }
        };
        let bottom = above.rect[1] + above.rect[3] + above.scrolled[1];
        let words = (0..els.len())
            .filter(|&k| els[k].role == "text" && !els[k].exiting && els[k].opacity > 0.01 && els[k].visible[3] > 0.01 && inside(k))
            .map(|k| els[k].visible[1] + els[k].visible[3] + els[k].scrolled[1])
            .fold(f32::NEG_INFINITY, f32::max);
        looks.push(Look { t, above: keys[i - 1].clone(), bottom, working: working.rect[1] + working.scrolled[1], words });
    }
    looks
}

/// Working's moves while the answer's words come in: (when it set off, how far it went, how
/// long it took), each a run of frames it moved in.
fn moves(looks: &[Look], from: f64, to: f64) -> Vec<(f64, f32, f64)> {
    let mut out: Vec<(f64, f32, f64)> = Vec::new();
    let mut open: Option<(f64, f32)> = None;
    for w in looks.windows(2).filter(|w| w[1].t > from && w[1].t < to) {
        let moving = (w[1].working - w[0].working).abs() > 1.0 / 64.0;
        match (moving, open) {
            (true, None) => open = Some((w[0].t, w[0].working)),
            (false, Some((t0, y0))) => {
                out.push((t0, w[0].working - y0, w[0].t - t0));
                open = None;
            }
            _ => {}
        }
    }
    out
}

fn check(word: f64, reveal: Reveal) {
    let looks = run(word, reveal);
    // No word of the reply draws below its row's moving edge, and nothing lands on Working.
    for l in &looks {
        // A text's box is its exact line box (Rune 1571ad57) while rows round to the pixel grid,
        // so the last line's leading may sit a point past the row's edge; its ink doesn't.
        assert!(!l.above.starts_with("t-") || l.words <= l.bottom + 1.5, "{reveal:?}, {word}s a word, {:.3}s: words draw to {}, under {}'s edge at {}", l.t, l.words, l.above, l.bottom);
        assert!(l.words <= l.working, "{reveal:?}, {word}s a word, {:.3}s: words draw to {}, over Working at {}", l.t, l.words, l.working);
    }
    // Thinking, a tool, the answer, a tool, the end: Working is its lane's gap under the last
    // row at every frame, with no room left under a reply for it to glide down into.
    for l in &looks {
        let gap = l.working - l.bottom;
        assert!((gap - LANE_PAD).abs() < 0.5, "{word}s a word, {:.3}s: Working {gap} pt under {}, not {LANE_PAD}", l.t, l.above);
    }
    // Each step down while the words stream is a whole line (22 or 23 pt as lines round to the
    // pixel grid), landed fast: one at a time when
    // the words come slowly, and no frame moves more than a line.
    let steps = moves(&looks, WORDS_AT + 0.3, words_end(word));
    assert!(!steps.is_empty(), "{word}s a word: the answer wraps onto more lines");
    let line = steps.iter().map(|s| s.1).fold(f32::INFINITY, f32::min);
    assert!((18.0..=32.0).contains(&line), "{word}s a word: a line is {line} pt: {steps:?}");
    for &(t, d, secs) in &steps {
        let n = (d / line).round();
        assert!(n >= 1.0 && (d - n * line).abs() < 1.25, "{word}s a word, {t:.3}s: Working moved {d} pt, not whole {line} pt lines: {steps:?}");
        if word >= SLOW {
            assert!(n == 1.0 && secs < 0.2, "{word}s a word, {t:.3}s: {d} pt took {secs:.3}s to open: {steps:?}");
        }
    }
    if word >= SLOW {
        assert!(steps.len() >= 2, "{word}s a word: the answer wraps a line at a time: {steps:?}");
    }
    for w in looks.windows(2) {
        let step = (w[1].working - w[0].working).abs();
        assert!(step <= line + 1.25, "{word}s a word, {:.3}s: Working moved {step} pt in a frame", w[1].t);
    }
    // The answer ending moves nothing: Working only rides the rows that come after it.
    let end = words_end(word);
    let at = |t: f64| looks.iter().find(|l| l.t >= t).unwrap().working;
    assert!((at(end + 0.08) - at(end - 0.02)).abs() < 0.5, "{word}s a word: Working moved as the words stopped");
}

#[test]
fn clip_opens_each_line_fast_under_its_edge() {
    check(SLOW, Reveal::Clip);
    check(FAST, Reveal::Clip);
}

#[test]
fn grow_then_show_opens_each_line_fast_under_its_edge() {
    check(SLOW, Reveal::GrowThenShow);
    check(FAST, Reveal::GrowThenShow);
}

#[test]
fn soft_edge_opens_each_line_fast_under_its_edge() {
    check(SLOW, Reveal::SoftEdge);
    check(FAST, Reveal::SoftEdge);
}

/// The check above sees words past a row's edge: Held room's do, into the room over Working.
#[test]
fn held_room_draws_past_its_edge() {
    let looks = run(SLOW, Reveal::HeldRoom);
    assert!(looks.iter().any(|l| l.above.starts_with("t-") && l.words > l.bottom + 0.5), "Held room's words stayed inside their row");
}

//! The room over Working (`view::CUSHION`): only while a reply's words stream in, and it opens
//! and closes without a jump (p252).

use crate::App;
use crate::view::CUSHION;
use crate::working::LANE_PAD;

/// One frame's look at the end of the timeline: the room's top (the bottom of the row above it),
/// Working's top, and whether a reply's words are streaming.
struct Look {
    t: f64,
    top: f32,
    working: f32,
    words: bool,
}

#[test]
fn the_room_over_working_opens_only_while_words_stream_in() {
    let mut rt = App::fixture_session().into_runtime();
    rt.frame(0.0);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    rt.update(|s, _| crate::cushion_story::play(s));
    let end = crate::cushion_story::script().last().unwrap().0 + 1.0;
    let (dt, mut t) = (1.0 / 120.0, 0.0);
    let mut looks = Vec::new();
    while t < end {
        rt.frame(dt);
        t += dt;
        let els = rt.elements();
        let live = |k: &str| els.iter().find(|e| !e.exiting && e.key.as_deref() == Some(k));
        let (Some(room), Some(working)) = (live("turn-cushion"), live("working")) else { continue };
        let crate::Route::Session(sid) = rt.state.route.clone() else { panic!("no open session") };
        let tr = &rt.state.transcripts[&sid];
        let words = tr.busy() && matches!(tr.messages.last(), Some(opencode_kit::model::Message::Assistant(a))
            if matches!(a.parts.last(), Some(opencode_kit::model::Part::Text { text }) if !text.trim().is_empty()));
        looks.push(Look { t, top: room.rect[1], working: working.rect[1], words });
    }
    let gap = |l: &Look| l.working - l.top;
    let rest = LANE_PAD;
    // Thinking, then a tool, with nothing streaming: Working its lane's gap under the last row.
    for at in [0.55, 1.2] {
        let l = looks.iter().find(|l| l.t >= at).unwrap();
        assert!(!l.words && (gap(l) - rest).abs() < 0.5, "{at}s: Working {} pt under the last row, not {rest}", gap(l));
    }
    // While the answer streams, a line of room opens over Working.
    let streamed = looks.iter().filter(|l| l.words).map(gap).fold(0.0, f32::max);
    assert!((streamed - rest - CUSHION).abs() < 0.5, "the room opens to {CUSHION} pt while words stream: {streamed}");
    // After the turn ends, the room is gone again.
    let last = looks.last().unwrap();
    assert!((gap(last) - rest).abs() < 0.5, "after the run, Working's lane sits {} pt under the reply", gap(last));
    // It opens and closes on a glide: never more than a few points in a 1/120 s frame.
    for w in looks.windows(2) {
        let step = (gap(&w[1]) - gap(&w[0])).abs();
        assert!(step < 3.0, "{:.3}s: the room jumps {step} pt in a frame", w[1].t);
    }
}

//! Kit's capture (p162): at the timeline's end, the Compacted divider folds open a summary
//! several screens long on the tool group's spring. It holds under the pointer while the end
//! moves down, shuts there in place, springs back from a press mid-glide, sticks to the top once
//! scrolled into the summary, and a press there shuts it without the divider leaving the screen.

use rune::ElementInfo;

use super::{Run, SID};
use crate::compaction::Piece;
use crate::compaction_stories::LONG;

fn bar(r: &Run) -> ElementInfo {
    let row = Piece::Quiet.key(LONG);
    r.rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("bar") && e.path.contains(&row)).expect("the Compacted divider")
}

/// The Compacted row's height as drawn.
fn row_h(r: &Run) -> f32 {
    let row = Piece::Quiet.key(LONG);
    r.rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(row.as_str())).map(|e| e.rect[3]).expect("the Compacted row")
}

fn viewport(r: &Run) -> [f32; 4] {
    let key = format!("scroll-{SID}");
    r.rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key.as_str())).map(|e| e.rect).expect("the timeline")
}

fn tap(r: &mut Run) {
    let b = bar(r);
    let (x, y) = (b.rect[0] + b.rect[2] / 2.0, b.visible[1] + b.visible[3] / 2.0);
    r.act("tap Compacted", |rt| {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
    });
}

pub fn open_at_bottom(r: &mut Run) {
    let open = |r: &Run| r.rt.state.expanded.contains(&Piece::Quiet.key(LONG));
    let (at, shut) = (bar(r).rect[1], row_h(r));
    let held = |r: &Run, what: &str| {
        let y = bar(r).rect[1];
        assert!((y - at).abs() < 1.0, "{what} holds the divider where it was pressed: {at} -> {y}");
    };
    tap(r);
    r.play(0.5);
    assert!(open(r), "the press opens the summary");
    held(r, "opening");
    tap(r);
    r.play(0.5);
    held(r, "shutting at the end");
    assert!((row_h(r) - shut).abs() < 0.5, "and it folds shut: {} vs {shut}", row_h(r));
    tap(r);
    r.play(0.06);
    tap(r);
    r.play(0.5);
    held(r, "a press mid-glide");
    assert!(!open(r) && (row_h(r) - shut).abs() < 0.5, "reversed mid-glide, it springs back shut: {} vs {shut}", row_h(r));
    tap(r);
    r.play(0.5);
    r.swipe_down(800.0, 0.6);
    let (b, v) = (bar(r), viewport(r));
    assert!(b.stuck > 0.0 && (b.rect[1] - v[1]).abs() < 1.0, "scrolled into the summary, the divider sticks to the top: {:?} in {v:?}", b.rect);
    tap(r);
    r.play(0.8);
    let (b, v) = (bar(r), viewport(r));
    assert!(!open(r), "a press on the stuck divider shuts the summary");
    assert!(b.rect[1] >= v[1] - 1.0 && b.rect[1] + b.rect[3] <= v[1] + v[3] + 1.0, "the divider stays on screen: {:?} in {v:?}", b.rect);
}

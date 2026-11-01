//! Kit's capture (p162): at the timeline's end, the Compacted divider opens a summary several
//! screens long. It holds under the pointer while the end moves down, sticks to the top once
//! scrolled into the summary, and a press there shuts it without the divider leaving the screen.

use rune::ElementInfo;

use super::{Run, SID};
use crate::compaction::Piece;
use crate::compaction_stories::LONG;

fn bar(r: &Run) -> ElementInfo {
    let row = Piece::Quiet.key(LONG);
    r.rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("bar") && e.path.contains(&row)).expect("the Compacted divider")
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
    let open = |r: &Run| r.rt.state.expanded.contains(&format!("compaction-{LONG}"));
    let at = bar(r).rect[1];
    tap(r);
    r.play(0.5);
    assert!(open(r), "the press opens the summary");
    let held = bar(r).rect[1];
    assert!((held - at).abs() < 1.0, "opening holds the divider where it was pressed: {at} -> {held}");
    r.swipe_down(800.0, 0.6);
    let (b, v) = (bar(r), viewport(r));
    assert!(b.stuck > 0.0 && (b.rect[1] - v[1]).abs() < 1.0, "scrolled into the summary, the divider sticks to the top: {:?} in {v:?}", b.rect);
    tap(r);
    r.play(0.8);
    let (b, v) = (bar(r), viewport(r));
    assert!(!open(r), "a press on the stuck divider shuts the summary");
    assert!(b.rect[1] >= v[1] - 1.0 && b.rect[1] + b.rect[3] <= v[1] + v[3] + 1.0, "the divider stays on screen: {:?} in {v:?}", b.rect);
}

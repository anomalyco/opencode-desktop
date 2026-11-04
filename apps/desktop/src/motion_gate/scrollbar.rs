//! Motion-gate scenarios for the timeline's scroll bar (Rune's `rune_kit::ScrollBar`): it shows
//! on a scroll and fades at rest, and its thumb drags the timeline.

use rune::{ElementInfo, Runtime};

use super::{Run, SID};
use crate::App;

fn part(rt: &Runtime<App>, key: &str) -> Option<ElementInfo> {
    let area = format!("scroll-{SID}-scroll-area");
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key) && e.path.contains(&area))
}

/// A scroll up shows the bar, which fades once the timeline rests; a scroll back down does it
/// again.
pub fn scroll_fade(r: &mut Run) {
    r.swipe_up(600.0, 2.0);
    r.swipe_down(300.0, 2.0);
}

/// The thumb dragged up the track, then back down: the timeline follows the pointer and the
/// thumb stays under it.
pub fn drag(r: &mut Run) {
    r.swipe_up(400.0, 0.3);
    let t = part(&r.rt, "thumb").expect("a thumb").rect;
    let (x, y) = (t[0] + t[2] / 2.0, t[1] + t[3] / 2.0);
    r.act("press the thumb", |rt| {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
    });
    for dy in (1..=12).map(|i| -12.0 * i as f32).chain((1..=12).map(|i| -144.0 + 8.0 * i as f32)) {
        r.act("drag", |rt| rt.pointer_move(x, y + dy));
    }
    r.act("release", |rt| rt.pointer_up(x, y - 48.0));
    r.play(0.4);
    r.act("pointer leaves", |rt| rt.pointer_move(x - 200.0, y));
    r.play(2.0);
}

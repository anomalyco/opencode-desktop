//! Motion-gate scenarios for the timeline's scroll bar (`scrollbar.rs`): it shows on a scroll
//! and fades at rest, widens under the pointer at the edge, and its thumb drags the timeline.

use rune::Runtime;

use super::{Run, SID};
use crate::App;

fn thumb(rt: &Runtime<App>) -> Option<[f32; 4]> {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("bar-thumb")).map(|e| e.rect)
}

fn edge(rt: &Runtime<App>) -> (f32, f32) {
    let key = format!("scroll-{SID}-bar");
    let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key.as_str())).expect("the scroll bar");
    (e.rect[0] + e.rect[2] - 4.0, e.rect[1] + e.rect[3] / 2.0)
}

/// A scroll up shows the bar, which fades once the timeline rests; a scroll back down does it
/// again.
pub fn scroll_fade(r: &mut Run) {
    r.swipe_up(600.0, 2.0);
    r.swipe_down(300.0, 2.0);
}

/// The pointer comes to the edge: the bar shows and its thumb widens; it leaves and the bar
/// narrows and fades.
pub fn hover(r: &mut Run) {
    let (x, y) = edge(&r.rt);
    r.act("pointer to the edge", |rt| rt.pointer_move(x, y));
    r.play(0.8);
    r.act("pointer leaves", |rt| rt.pointer_move(x - 200.0, y));
    r.play(2.0);
}

/// The thumb dragged up the track, then back down: the timeline follows the pointer and the
/// thumb stays under it.
pub fn drag(r: &mut Run) {
    r.swipe_up(400.0, 0.3);
    let t = thumb(&r.rt).expect("a thumb");
    let (x, y) = (edge(&r.rt).0, t[1] + t[3] / 2.0);
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

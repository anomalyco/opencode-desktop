//! A reply's diagram, too wide for the column, opens full size (`diagram`): it grows out of its
//! card to the middle of the window, a pinch and two fingers move it on the fingers, ⌘0 springs
//! it back to its fit, and Esc flies it back into its card before it goes.

use rune::{Key, KeyEvent, MagnifyEvent, Mods, WheelEvent};

use super::Run;

pub fn open_zoom_close(r: &mut Run) {
    r.rt.update(|s, _| crate::diagram::stories::add(s));
    r.idle(1.0);
    // A wide window draws the diagram taller (top tabs leave the column the whole width): bring
    // its top, where the open button sits, into view.
    for _ in 0..8 {
        let top = r.timeline_rect()[1];
        let open = r.rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("diagram-open")).map(|e| e.rect);
        if open.is_none_or(|b| b[1] >= top + 8.0) {
            break;
        }
        r.scroll_up(120.0);
    }
    let card = r.rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("card") && e.path.contains("t-msg_zzwide1")).map(|e| e.rect).expect("the diagram's card");
    r.act("hover the diagram", |rt| rt.pointer_move(card[0] + card[2] / 2.0, card[1] + card[3] / 2.0));
    r.play(0.2);
    r.restart();
    r.act("open it full size", |rt| Run::tap(rt, "diagram-open"));
    r.play(0.6);
    let open = r.rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("diagram-lightbox")).map(|e| e.rect).expect("the open diagram");
    let (x, y) = (open[0] + open[2] * 0.4, open[1] + open[3] * 0.4);
    for _ in 0..30 {
        r.act("pinch in", |rt| rt.magnify_event(MagnifyEvent::new(0.03).at(x, y)));
    }
    r.play(0.3);
    for _ in 0..30 {
        r.act("pan", |rt| rt.wheel_event(WheelEvent::new(-9.0, -3.0).at(x, y).precise(true)));
    }
    r.play(0.3);
    r.act("⌘0", |rt| rt.key(KeyEvent { key: Key::Command('0'), mods: Mods { cmd: true, ..Default::default() } }));
    r.play(0.6);
    r.act("Esc", |rt| rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() }));
    r.play(0.8);
}

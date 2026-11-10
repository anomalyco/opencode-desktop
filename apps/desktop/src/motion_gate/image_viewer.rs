//! A reply's inline picture opens in the image viewer (`session_view::images::viewer`): it flies
//! out of its place to the window's middle and ⌘ + zooms it; Escape fits it, as Desktop's does,
//! and a second Escape while that fit still glides flies it home from where it shows. A
//! double-click on the fitted picture zooms it about the pointer, and another fits it again.

use rune::{Key, KeyEvent, Mods};

use super::Run;

fn picture(r: &Run) -> Option<[f32; 4]> {
    r.rt.elements().into_iter().find(|e| !e.exiting && e.role == "button" && e.label.as_deref() == Some("The sidebar after the fix") && e.rect[3] > 40.0).map(|e| e.rect)
}

/// The reply with its picture, landed and settled at the timeline's end. Returns the middle of
/// the part the timeline shows (zoomed, the picture is taller than the timeline).
fn add(r: &mut Run) -> (f32, f32) {
    r.rt.state.attach.set_waker(std::sync::Arc::new(|| {}));
    crate::stories::serve_pictures(&r.rt.state);
    r.rt.update(|s, _| crate::stories::add_picture_reply(s));
    for _ in 0..400 {
        r.rt.update(|_, _| {});
        r.idle(1.0 / 60.0);
        if picture(r).is_some() { break; }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    r.idle(1.0);
    let home = picture(r).expect("the reply's picture");
    let shown = r.timeline_rect();
    let (top, bottom) = (home[1].max(shown[1]), (home[1] + home[3]).min(shown[1] + shown[3]));
    let at = (home[0] + home[2] / 2.0, (top + bottom) / 2.0);
    r.act("hover the picture", |rt| rt.pointer_move(at.0, at.1));
    r.play(0.2);
    r.restart();
    at
}

fn open(r: &mut Run, (x, y): (f32, f32)) -> [f32; 4] {
    r.act("open it", |rt| {
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
    });
    r.play(0.6);
    r.rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("image-viewer")).map(|e| e.rect).expect("the viewer opens")
}

pub fn open_zoom_close(r: &mut Run) {
    let at = add(r);
    open(r, at);
    r.act("⌘ +", |rt| rt.key(KeyEvent { key: Key::Command('='), mods: Mods { cmd: true, ..Default::default() } }));
    r.play(0.6);
    r.act("Esc: fit", |rt| rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() }));
    r.play(0.1);
    r.act("Esc: close", |rt| rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() }));
    r.play(0.8);
}

pub fn double_click(r: &mut Run) {
    let at = add(r);
    let open = open(r, at);
    let (x, y) = (open[0] + open[2] * 0.3, open[1] + open[3] * 0.4);
    r.act("press", |rt| {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
    });
    r.act("release", |rt| rt.pointer_up(x, y));
    r.act("press again", |rt| rt.pointer_down(x, y));
    r.act("release: a double-click", |rt| rt.pointer_up(x, y));
    r.play(0.6);
    r.act("double-click back to fit", |rt| {
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
    });
    r.play(0.6);
    r.act("Esc", |rt| rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() }));
    r.play(0.8);
}

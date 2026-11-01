//! The empty composer holds one line; typing a second grows it by a line and the toolbar rides
//! down with it, and deleting back shrinks it the same way (`animate_edit_layout`).

use rune::{Key, KeyEvent, Mods};

use super::Run;

pub fn grow(r: &mut Run) {
    r.act("tap the composer", |rt| Run::tap(rt, "prompt"));
    r.play(0.4);
    let key = |k: Key| KeyEvent { key: k, mods: Mods::default() };
    r.act("type a line", |rt| rt.key(key(Key::Text("First line".into()))));
    r.play(0.4);
    r.act("⏎ then a second line", |rt| {
        rt.key(key(Key::Text("\n".into())));
        rt.key(key(Key::Text("Second line".into())));
    });
    r.play(0.8);
    r.act("delete back to one line", |rt| {
        for _ in 0..12 {
            rt.key(key(Key::Backspace));
        }
    });
    r.play(0.8);
}

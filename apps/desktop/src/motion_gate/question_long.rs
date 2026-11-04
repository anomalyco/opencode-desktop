//! A long question (p273): the dock opens at its share of the pane, ↓ walks every answer and
//! the list scrolls just far enough to show each, End and Home reach the ends, the next
//! question comes and goes, and the answer closes the dock.

use rune::{Key, KeyEvent, Mods};
use serde_json::json;

use super::{Run, SID};

pub fn walk(r: &mut Run) {
    std::mem::forget(r.rt.state.fake_writes());
    let q = crate::question::long::form(&r.rt.state);
    let id = q["id"].clone();
    r.event("form.created", q);
    r.play(0.8);
    let key = |r: &mut Run, what: &str, key: Key| r.act(what, |rt| rt.key(KeyEvent { key, mods: Mods::default() }));
    for _ in 0..9 {
        key(r, "↓", Key::Down);
        r.play(0.2);
    }
    key(r, "home", Key::Home);
    r.play(0.3);
    key(r, "end", Key::End);
    r.play(0.3);
    key(r, "→", Key::Right);
    r.play(0.6);
    key(r, "←", Key::Left);
    r.play(0.6);
    r.event("form.replied", json!({ "id": id, "sessionID": SID }));
    // The tall dock closing settles in about a second.
    r.play_out(1.5);
}

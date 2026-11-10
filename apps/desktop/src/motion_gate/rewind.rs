//! Motion-gate scenario for Rewind (the message menu's Undo, the `Rewound` toast's `Undo`).

use rune::{Key, KeyEvent, Mods, Runtime};

use super::{Run, SID};
use crate::App;

fn in_band(rt: &Runtime<App>, key: &str) -> Option<(f32, f32)> {
    rt.find(key).filter(|&(_, y)| (120.0..540.0).contains(&y))
}

pub fn run(r: &mut Run) {
    if !r.circ.scrolled {
        let cur = r.rt.state.visible_user_ids(SID).pop();
        if cur.as_ref().is_none_or(|id| in_band(&r.rt, &format!("user-menu-{id}")).is_none()) {
            // Send a short prompt so a user message sits on screen at the pinned bottom.
            r.type_prompt();
            r.enter(false);
            r.play_out(1.0);
        }
    }
    let Some(to) = r.rt.state.visible_user_ids(SID).pop() else { return };
    let more_key = format!("user-menu-{to}");
    if r.circ.scrolled {
        for _ in 0..6 {
            if in_band(&r.rt, &more_key).is_some() {
                break;
            }
            r.swipe_up(450.0, 0.8);
        }
    }
    if in_band(&r.rt, &more_key).is_none() {
        return;
    }
    r.act("hover prompt", |rt| {
        let (x, y) = rt.find(&more_key).expect("menu button");
        rt.pointer_move(x, y);
    });
    r.play(0.1);
    r.act("open message menu", |rt| Run::tap(rt, &more_key));
    r.play(0.2);
    // On macOS the menu is the system's (`popup::NATIVE`, 31056cee): nothing of it moves in the
    // window, so it's dismissed and picked as the system would.
    if r.rt.popup_menu().is_some() {
        r.act("esc message menu", |rt| { let _ = rt.resolve_popup(None); });
        r.play(0.2);
        r.act("reopen message menu", |rt| Run::tap(rt, &more_key));
        r.play(0.2);
        r.act("pick Undo", |rt| rt.resolve_popup(Some("revert")).expect("Undo"));
    } else {
        r.act("esc message menu", |rt| rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() }));
        r.play(0.2);
        r.act("reopen message menu", |rt| Run::tap(rt, &more_key));
        r.play(0.2);
        r.act("hover Undo", |rt| {
            let (x, y) = rt.find("popup-item-revert").expect("Undo");
            rt.pointer_move(x, y);
        });
        r.play(0.1);
        r.act("tap Undo", |rt| Run::tap(rt, "popup-item-revert"));
    }
    r.play(0.8);
    r.act("tap Undo on Rewound toast", |rt| Run::tap(rt, "toast-undo"));
    r.play(0.8);
}

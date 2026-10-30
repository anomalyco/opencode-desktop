//! Motion-gate scenario for Rewind (`Rewind to here`, `Rewound` toast `Undo`, and `⋯` menu).

use rune::{Key, KeyEvent, Mods, Runtime};

use super::{Run, SID};
use crate::App;

fn in_band(rt: &Runtime<App>, key: &str) -> Option<(f32, f32)> {
    rt.find(key).filter(|&(_, y)| (120.0..540.0).contains(&y))
}

pub fn run(r: &mut Run) {
    if !r.circ.scrolled {
        let cur = r.rt.state.visible_user_ids(SID).pop();
        if cur.as_ref().is_none_or(|id| in_band(&r.rt, &format!("more-{id}")).is_none()) {
            // Send a short prompt so a user message sits on screen at the pinned bottom.
            r.type_prompt();
            r.enter(false);
            r.play_out(1.0);
        }
    }
    let Some(to) = r.rt.state.visible_user_ids(SID).pop() else { return };
    let more_key = format!("more-{to}");
    let rewind_key = format!("rewind-{to}");
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
        let (x, y) = rt.find(&more_key).expect("more button");
        rt.pointer_move(x, y);
    });
    r.play(0.1);
    r.act("open message menu", |rt| Run::tap(rt, &more_key));
    r.play(0.2);
    r.act("esc message menu", |rt| rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() }));
    r.play(0.2);
    r.act("hover Rewind to here", |rt| {
        let (x, y) = rt.find(&rewind_key).expect("rewind button");
        rt.pointer_move(x, y);
    });
    r.play(0.1);
    r.act("tap Rewind to here", |rt| Run::tap(rt, &rewind_key));
    r.play(0.8);
    r.act("tap Undo on Rewound toast", |rt| Run::tap(rt, "toast-undo"));
    r.play(0.8);
}

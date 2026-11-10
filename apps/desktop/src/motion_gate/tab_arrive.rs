//! A tab dragged in from another window over the titlebar's strip: the tabs past the slot it
//! would take step aside on the strip's reorder spring, follow it as it moves along, close up
//! when it leaves, open again when it's back, and the tab lands in the gap when it's let go.

use super::Run;
use crate::windows::tear_stories::{along, incoming, roots};

pub fn run(r: &mut Run) {
    r.rt.update(|s, tx| {
        tx.instant();
        s.tabs = roots(s).into_iter().take(3).collect();
        if let Some(first) = s.tabs.first().map(|t| t.id.clone()) {
            s.select_tab(&first, tx);
        }
    });
    r.idle(1.0);
    r.restart();
    for share in [0.9, 0.6, 0.35, 0.1] {
        r.act("a tab from another window moves along the strip", |rt| rt.update(move |s, tx| {
            let at = along(s, share);
            s.tab_incoming(at, tx);
        }));
        r.play(0.3);
    }
    r.act("it leaves the strip", |rt| rt.update(|s, tx| {
        s.tab_incoming(None, tx);
    }));
    r.play(0.4);
    r.act("it's back", |rt| rt.update(|s, tx| {
        let at = along(s, 0.35);
        s.tab_incoming(at, tx);
    }));
    r.play(0.4);
    r.act("let go", |rt| rt.update(|s, tx| {
        if let (Some(tab), at) = (incoming(s), along(s, 0.35)) {
            s.receive_tab(tab, None, at, tx);
        }
    }));
    r.play(0.8);
}

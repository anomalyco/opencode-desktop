//! Opening a tool group shows its steps right under its row, wherever the row sits (p277).

use rune::{ElementInfo, Runtime};

use crate::App;
use crate::motion_gate::SID;
use crate::shell_fold::{GROUP, add_shell_fold};

const DT: f64 = 1.0 / 60.0;
const STEP: &str = "tool-call_zzshellfold";

fn find<'a>(els: &'a [ElementInfo], key: &str) -> Option<&'a ElementInfo> {
    els.iter().find(|e| !e.exiting && e.key.as_deref() == Some(key) && e.path.contains(GROUP))
}

fn viewport(els: &[ElementInfo]) -> [f32; 4] {
    els.iter().find(|e| e.key.as_deref() == Some(&*format!("scroll-{SID}"))).expect("the timeline").rect
}

/// Scrolls the group's row so its top sits `at` points below the timeline's top, presses it,
/// and lets it settle. Returns the row's drawn top before and after (stuck shift included), how
/// much of the step shows below it on screen, and the timeline's rect.
fn open_at(at: f32) -> (f32, f32, f32, [f32; 4]) {
    let mut app = App::fixture_session();
    add_shell_fold(&mut app, SID);
    let mut rt: Runtime<App> = app.into_runtime();
    rt.frame(0.0);
    for _ in 0..60 {
        rt.frame(DT);
    }
    let trigger = format!("{GROUP}/trigger");
    rt.update(move |_, tx| tx.scroll_key_to(trigger.clone(), at));
    for _ in 0..30 {
        rt.frame(DT);
    }
    let els = rt.elements();
    let t = find(&els, "trigger").expect("the group's row");
    let before = t.rect[1];
    let (x, y) = (t.rect[0] + 12.0, (t.visible[1] + t.visible[3] / 2.0).max(t.rect[1] + 20.0));
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..60 {
        rt.frame(DT);
    }
    assert!(rt.state.expanded.contains(GROUP), "the press opened the group");
    let els = rt.elements();
    let t = find(&els, "trigger").expect("the group's row").rect;
    let view = viewport(&els);
    let shows = find(&els, STEP).map_or(0.0, |e| {
        let v = e.visible;
        let top = v[1].max(t[1] + t[3]).max(view[1]);
        (v[1] + v[3]).min(view[1] + view[3]) - top
    });
    (before, t[1], shows, view)
}

#[test]
fn a_group_opens_below_its_row_wherever_it_sits() {
    let mut bad = Vec::new();
    for at in [-10.0, 250.0, 450.0] {
        let (before, after, shows, view) = open_at(at);
        // The whole step, clear of the row over it.
        if shows < 24.0 {
            bad.push(format!("row at {at}: only {shows} pt of its step shows below it"));
        }
        // A row half under the top edge comes down to it; otherwise it holds under the pointer.
        let want = before.max(view[1]);
        if (after - want).abs() > 1.0 {
            bad.push(format!("row at {at}: moved from {before} to {after}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

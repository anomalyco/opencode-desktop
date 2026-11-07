//! A short tool group shutting: its steps stay under the row's clip until they unmount (t1107).

use rune::{ElementInfo, Runtime};

use crate::App;
use crate::motion_gate::SID;

use crate::short_group::{GROUP, add_short_group};

const DT: f64 = 1.0 / 60.0;

/// Steps of group `key` that show below its row, as `key@bottom` against the pixel its row ends
/// on (at scale 1). Paint rounds a scroll's translation as a whole and each edge under it on
/// its own, so at a fractional scroll (a zoomed window) the row's clip ends up to a pixel past
/// its exact bottom, on the pixel where the row below starts.
pub fn overhang(els: &[ElementInfo], key: &str) -> Vec<String> {
    let Some(row) = els.iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)) else { return vec![] };
    let pixel = |v: f32| (v + 0.5).floor();
    let scrolled = row.scrolled[1];
    let bottom = pixel(row.rect[1] + row.rect[3] + scrolled) + pixel(-scrolled);
    els.iter()
        .filter(|e| e.path.starts_with(&row.path) && e.path.contains("/steps-box") && e.opacity > 0.01)
        .filter(|e| e.visible[2] > 0.5 && e.visible[3] > 0.5 && e.visible[1] + e.visible[3] > bottom + 1.0 / 64.0)
        .filter_map(|e| Some(format!("{}@{} below {bottom}", e.key.as_deref()?, e.visible[1] + e.visible[3])))
        .collect()
}

#[test]
fn a_short_group_shut_keeps_its_steps_under_its_row() {
    let mut app = App::fixture_session();
    add_short_group(&mut app, SID);
    let mut rt: Runtime<App> = app.into_runtime();
    rt.frame(0.0);
    for _ in 0..90 {
        rt.frame(DT);
    }
    // Kit's four presses, open, shut, open, shut, then long enough for the steps to unmount.
    let presses = [0, 41, 67, 96];
    let mut seen = Vec::new();
    for f in 0..160usize {
        if presses.contains(&f) {
            let els = rt.elements();
            let t = els.iter().find(|e| e.key.as_deref() == Some("trigger") && e.path.ends_with(&format!("{GROUP}/#0/trigger"))).expect("the group's header");
            let (x, y) = (t.rect[0] + 12.0, t.visible[1] + t.visible[3] / 2.0);
            rt.pointer_move(x, y);
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
        }
        rt.frame(DT);
        if !rt.state.expanded.contains(GROUP) {
            let over = overhang(&rt.elements(), GROUP);
            if !over.is_empty() {
                seen.push(format!("frame {f}: {over:?}"));
            }
        }
    }
    assert!(seen.is_empty(), "a shut group's steps showed below its row:\n{}", seen.join("\n"));
}

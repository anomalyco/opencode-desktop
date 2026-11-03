//! Kit's capture (t1129): with thirty subagents and a plan running, opening and shutting a tool
//! group in the last turn pushed Thinking and Working down under the pills. The pills have their
//! own lane at the end of the timeline, so while it shows its end no text shows under a pill on
//! any frame. Off the end (p248) the timeline runs on under the pills and fades into the composer.

use rune::ElementInfo;

use super::Run;
use crate::short_group::GROUP;

/// Text in the timeline whose visible part overlaps a pill that shows.
fn under_pills(els: &[ElementInfo]) -> Vec<String> {
    let pills: Vec<[f32; 4]> = els.iter().filter(|e| !e.exiting && e.opacity > 0.01 && matches!(e.key.as_deref(), Some("activity-chip" | "todo-pill"))).map(|e| e.rect).collect();
    let hit = |v: [f32; 4], p: [f32; 4]| v[3] > 0.5 && v[2] > 0.5 && v[0] < p[0] + p[2] && p[0] < v[0] + v[2] && v[1] < p[1] + p[3] && p[1] < v[1] + v[3];
    els.iter()
        .filter(|e| e.role == "text" && e.opacity > 0.01 && e.path.contains("/timeline/"))
        .filter(|e| pills.iter().any(|&p| hit(e.visible, p)))
        .map(|e| format!("{:?} at {:?}", e.text.as_deref().unwrap_or(&e.path), e.visible))
        .collect()
}

fn play_clear(r: &mut Run, secs: f64, what: &str) {
    for _ in 0..(secs / super::DT).round() as usize {
        r.play(super::DT);
        let at_end = match &r.rt.state.route { crate::Route::Session(id) => r.rt.state.ends.at_end(id), _ => true };
        let over = under_pills(&r.rt.elements());
        assert!(!at_end || over.is_empty(), "{what}: text under a pill at the end: {over:?}");
    }
}

pub fn working_clear(r: &mut Run) {
    r.act("thirty agents start", |rt| rt.update(|s, _| crate::capsule_stories::run_thirty(s)));
    play_clear(r, 0.6, "subagents starting");
    r.act("the run plans", |rt| rt.update(|s, _| crate::todo_stories::set_step(s, 1)));
    play_clear(r, 0.6, "the plan arriving");
    let path = r.rt.elements().into_iter()
        .find(|e| e.key.as_deref() == Some("trigger") && e.path.ends_with(&format!("{GROUP}/#0/trigger")))
        .map(|e| e.path)
        .expect("the short group's header");
    for what in ["opening the group", "shutting it"] {
        let e = r.rt.elements().into_iter().find(|e| e.path == path).expect("the header");
        let (x, y) = (e.rect[0] + 12.0, e.visible[1] + e.visible[3] / 2.0);
        r.act(what, |rt| {
            rt.pointer_move(x, y);
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            // Off the window, so no row slides under a resting pointer and lights up.
            rt.pointer_leave();
        });
        play_clear(r, 0.8, what);
    }
}

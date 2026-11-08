//! Kit's capture (t1458): Home's rows reshuffled under the pointer as Flow's sessions updated.
//! Now, while the pointer is on the list a session that updates keeps its row; when the pointer
//! leaves, the rows move to their new order on the layout spring, never at once.

use serde_json::json;

use super::Run;
use crate::view::is_automation;

/// Home's session rows top to bottom: key and top.
fn rows(r: &Run) -> Vec<(String, f32)> {
    let mut v: Vec<(String, f32)> = r
        .rt
        .elements()
        .into_iter()
        .filter(|e| !e.exiting && e.key.as_deref().is_some_and(|k| k.starts_with("r-ses_")))
        .map(|e| (e.key.unwrap(), e.rect[1]))
        .collect();
    v.sort_by(|a, b| a.1.total_cmp(&b.1));
    v
}

pub fn holds_under_the_pointer(r: &mut Run) {
    r.rt.update(|s, tx| {
        tx.instant();
        crate::view::add_flow(s, 60, 4);
        s.go_home(tx);
    });
    r.idle(1.0);
    r.restart();
    let before = rows(r);
    assert!(before.len() >= 3, "Home lists the fixture's sessions: {before:?}");
    let (first, _) = before[0].clone();
    let last = before.last().unwrap().0.trim_start_matches("r-").to_owned();
    assert!(r.rt.state.session(&last).is_some_and(|s| !is_automation(&s.title)));
    let (x, y) = r.rt.find(&first).expect("the top row");
    r.act("pointer onto the top row", |rt| rt.pointer_move(x, y));
    r.play(0.3);
    r.event("session.step.ended", json!({ "sessionID": last }));
    r.play(1.0);
    assert_eq!(rows(r), before, "the rows hold still while the pointer is on the list");
    r.act("pointer leaves the list", |rt| rt.pointer_leave());
    r.play(1.2);
    let after = rows(r);
    assert_eq!(after[0].0, format!("r-{last}"), "then the session that updated leads");
}

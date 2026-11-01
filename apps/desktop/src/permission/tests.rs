//! The permission dock in the composer's place.
use std::sync::mpsc::{Receiver, Sender, channel};

use opencode_kit::client::{Incoming, Write};
use rune::{Runtime, TextEdit};
use serde_json::json;

use crate::fixture_ids::{CHILD, MAIN};
use crate::{App, Route};

const DT: f64 = 1.0 / 120.0;

struct Live {
    rt: Runtime<App>,
    writes: Receiver<Write>,
    server: Sender<Incoming>,
}

fn ask(app: &mut App, id: &str, session: &str, action: &str, resources: &[&str]) {
    app.apply_event("permission.asked", &json!({ "id": id, "sessionID": session, "action": action, "resources": resources }));
}

/// The fixture session with a draft in the composer and `asks` asked, on a live source whose
/// writes land in a channel and whose server lines come from one.
fn live(asks: &[(&str, &str, &str, &[&str])]) -> Live {
    let mut app = App::fixture_session();
    app.composer = TextEdit::new("half a thought");
    for (id, session, action, resources) in asks {
        ask(&mut app, id, session, action, resources);
    }
    let writes = app.fake_writes();
    let (server, inbox) = channel();
    app.inbox = Some(inbox);
    let mut rt = app.into_runtime();
    settle(&mut rt);
    Live { rt, writes, server }
}

fn bash() -> Live {
    live(&[("per_1", MAIN, "bash", &["bun test test/net/backoff.test.ts"])])
}

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..90 {
        rt.frame(DT);
    }
}

fn rect(rt: &Runtime<App>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("no {key}")).rect
}

fn shown(rt: &Runtime<App>, key: &str) -> bool {
    rt.elements().into_iter().any(|e| !e.exiting && e.key.as_deref() == Some(key))
}

fn tap(rt: &mut Runtime<App>, key: &str) {
    let r = rect(rt, key);
    let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(DT);
}

fn decisions(w: &Receiver<Write>) -> Vec<(String, String)> {
    w.try_iter()
        .filter_map(|w| match w {
            Write::Permission { request, decision, .. } => Some((request, decision)),
            _ => None,
        })
        .collect()
}

fn shows_composer(rt: &Runtime<App>) -> bool {
    shown(rt, "prompt") && !shown(rt, "permission")
}

#[test]
fn the_composer_becomes_the_permission() {
    let l = bash();
    assert!(shown(&l.rt, "permission") && !shown(&l.rt, "prompt") && !shown(&l.rt, "toolbar"), "one surface, not a card above the composer");
    let label = |key: &str| l.rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).and_then(|e| e.text);
    assert_eq!(label("permission-title").as_deref(), Some("Permission required"));
    assert_eq!(label("permission-hint").as_deref(), Some("Run shell commands"));
    assert_eq!(label("pattern-0").as_deref(), Some("bun test test/net/backoff.test.ts"));
}

#[test]
fn deny_allow_always_and_allow_once_sit_in_that_order_at_the_right() {
    let l = bash();
    let [deny, always, once] = ["perm-deny", "perm-always", "perm-once"].map(|k| rect(&l.rt, k));
    assert!(deny[0] + deny[2] <= always[0] && always[0] + always[2] <= once[0], "{deny:?} {always:?} {once:?}");
    assert!(deny[1] == always[1] && always[1] == once[1], "one row");
    let surface = rect(&l.rt, "composer");
    let right = surface[0] + surface[2] - (once[0] + once[2]);
    assert!((right - 8.0).abs() < 1.0, "Allow once ends the footer's 8 pt in: {right}");
    let left = deny[0] - surface[0];
    assert!(left > surface[2] / 2.0, "the answers hug the right: {left}");
    // The title and what's under it start past the icon.
    let (icon, title, hint) = (rect(&l.rt, "permission-icon"), rect(&l.rt, "permission-title"), rect(&l.rt, "permission-hint"));
    assert!((title[0] - hint[0]).abs() < 0.5 && title[0] >= icon[0] + icon[2] + 8.0 - 0.5);
}

#[test]
fn the_description_row_shows_only_when_desktop_names_the_tool() {
    let l = live(&[("per_1", MAIN, "github_create_issue", &["anomalyco/seer"])]);
    assert!(!shown(&l.rt, "permission-hint"));
    assert!(shown(&l.rt, "pattern-0"));
    let l = live(&[("per_1", MAIN, "bash", &[])]);
    assert!(shown(&l.rt, "permission-hint"));
    assert!(!shown(&l.rt, "permission-patterns"));
}

#[test]
fn every_pattern_shows_and_a_long_one_wraps() {
    let long = "git commit -am 'Backoff: cap at 30 s, reset after a success, keep jitter in range' && bun run typecheck && bun test test/net/backoff.test.ts --timeout 20000";
    let many: Vec<String> = (0..8).map(|i| format!("src/net/file-{i}.ts")).collect();
    let mut asks: Vec<&str> = many.iter().map(String::as_str).collect();
    asks.push(long);
    let l = live(&[("per_1", MAIN, "edit", &asks)]);
    for i in 0..9 {
        assert!(shown(&l.rt, &format!("pattern-{i}")), "pattern {i}");
    }
    assert!(rect(&l.rt, "pattern-8")[3] > 30.0, "the long one wraps onto more lines");
}

#[test]
fn allow_once_answers_at_once_and_the_composer_comes_back_with_its_draft() {
    let mut l = bash();
    tap(&mut l.rt, "perm-once");
    assert_eq!(decisions(&l.writes), [("per_1".to_owned(), "once".to_owned())]);
    assert!(l.rt.state.permission_asked().is_none(), "gone on the press");
    settle(&mut l.rt);
    assert!(shows_composer(&l.rt));
    assert_eq!(l.rt.state.composer.text, "half a thought");
}

#[test]
fn each_button_sends_its_decision() {
    for (key, decision) in [("perm-deny", "reject"), ("perm-always", "always"), ("perm-once", "once")] {
        let mut l = bash();
        tap(&mut l.rt, key);
        assert_eq!(decisions(&l.writes), [("per_1".to_owned(), decision.to_owned())], "{key}");
    }
}

#[test]
fn a_refused_reply_brings_the_permission_back_with_a_toast() {
    let mut l = bash();
    tap(&mut l.rt, "perm-always");
    settle(&mut l.rt);
    assert!(shows_composer(&l.rt));
    l.server.send(Incoming::Status(format!("POST /api/session/{MAIN}/permission/per_1/reply: http status: 500"))).unwrap();
    settle(&mut l.rt);
    assert!(shown(&l.rt, "permission"), "back in the composer's place");
    assert_eq!(l.rt.state.permission_asked().map(|p| p.id().to_owned()).as_deref(), Some("per_1"));
    let toast = l.rt.state.toast.current.as_ref().expect("a toast");
    assert_eq!(toast.title, "Couldn't answer the permission");
    assert_eq!(l.rt.state.composer.text, "half a thought");
}

#[test]
fn a_re_read_before_the_server_has_the_reply_doesnt_bring_it_back() {
    let mut l = bash();
    tap(&mut l.rt, "perm-once");
    let again = opencode_kit::model::parse_permission(&json!({ "id": "per_1", "sessionID": MAIN, "action": "bash" })).unwrap();
    l.server.send(Incoming::Pending { session: MAIN.into(), items: vec![again] }).unwrap();
    settle(&mut l.rt);
    assert!(shows_composer(&l.rt));
    l.rt.update(|s, _| {
        s.apply_event("permission.replied", &json!({ "requestID": "per_1", "sessionID": MAIN }));
    });
    assert!(l.rt.state.permission_sent.0.is_empty(), "the server has it");
}

#[test]
fn several_wait_one_at_a_time_oldest_first() {
    let mut l = live(&[("per_1", MAIN, "bash", &["one"]), ("per_2", MAIN, "edit", &["two"])]);
    assert_eq!(l.rt.state.permission_asked().map(|p| p.id().to_owned()).as_deref(), Some("per_1"));
    tap(&mut l.rt, "perm-once");
    settle(&mut l.rt);
    assert_eq!(l.rt.state.permission_asked().map(|p| p.id().to_owned()).as_deref(), Some("per_2"));
    assert!(shown(&l.rt, "permission"));
    tap(&mut l.rt, "perm-once");
    settle(&mut l.rt);
    assert!(shows_composer(&l.rt));
}

#[test]
fn the_pages_own_request_goes_before_its_subagents() {
    let l = live(&[("per_child", CHILD, "bash", &["child"]), ("per_main", MAIN, "bash", &["main"])]);
    assert_eq!(l.rt.state.permission_asked().map(|p| p.id().to_owned()).as_deref(), Some("per_main"));
}

#[test]
fn a_subagents_permission_shows_on_its_own_page_and_not_its_parents_on_it() {
    let mut l = live(&[("per_main", MAIN, "bash", &["main"])]);
    l.rt.update(|s, _| s.route = Route::Session(CHILD.into()));
    settle(&mut l.rt);
    assert!(l.rt.state.permission_asked().is_none(), "the parent's stays on the parent's page");
    let data = json!({ "id": "per_child", "sessionID": CHILD, "action": "bash", "resources": ["child"] });
    l.server.send(Incoming::Event { ty: "permission.asked".into(), data }).unwrap();
    settle(&mut l.rt);
    assert!(shown(&l.rt, "permission"));
    tap(&mut l.rt, "perm-once");
    assert_eq!(decisions(&l.writes), [("per_child".to_owned(), "once".to_owned())]);
}

#[test]
fn auto_accept_keeps_it_hidden() {
    let mut l = bash();
    l.rt.update(|s, _| s.settings.auto_accept = true);
    settle(&mut l.rt);
    assert!(shows_composer(&l.rt));
}

/// The surface's height each frame from a change to the next 120 frames.
fn heights(rt: &mut Runtime<App>, change: impl FnOnce(&mut Runtime<App>)) -> Vec<f32> {
    let h = |rt: &Runtime<App>| rect(rt, "composer")[3];
    let mut hs = vec![h(rt)];
    change(rt);
    for _ in 0..120 {
        rt.frame(DT);
        hs.push(h(rt));
    }
    hs
}

/// The swap springs between the two faces both ways, as the question dock's morph does: no
/// frame covers more than a fifth of the way, and it settles.
#[test]
fn the_face_swap_springs_both_ways() {
    let mut l = live(&[]);
    let data = json!({ "id": "per_1", "sessionID": MAIN, "action": "edit", "resources": ["src/a.ts", "src/b.ts", "src/c.ts", "src/d.ts"] });
    let server = l.server.clone();
    let hs = heights(&mut l.rt, move |_| server.send(Incoming::Event { ty: "permission.asked".into(), data }).unwrap());
    springs(&hs, "opening");
    let hs = heights(&mut l.rt, |rt| tap(rt, "perm-once"));
    springs(&hs, "closing");
    assert!(shows_composer(&l.rt));
}

fn springs(hs: &[f32], what: &str) {
    let (a, b) = (hs[0], *hs.last().unwrap());
    assert!((a - b).abs() > 30.0, "{what}: {a} → {b}");
    for w in hs.windows(2) {
        assert!((w[1] - w[0]).abs() < (b - a).abs() / 5.0, "{what}: jumps {w:?}");
    }
    assert!((hs[hs.len() - 2] - b).abs() < 0.5, "{what}: settles");
}

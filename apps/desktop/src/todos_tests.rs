use rune::{Key, KeyEvent, Mods, Runtime};
use serde_json::json;

use opencode_kit::model::{Transcript, parse_message};

use super::*;
use crate::Route;
use crate::fixture_ids::{CHILD, SIXTH};
use crate::fixture_runtime as rt;

const DT: f64 = 1.0 / 120.0;

fn call(id: &str, status: &str, todos: serde_json::Value) -> serde_json::Value {
    json!({ "type": "tool", "id": id, "name": "todowrite", "state": { "status": status, "input": { "todos": todos } } })
}

fn transcript() -> Transcript {
    let msgs = json!([
        { "id": "msg_a", "type": "user", "text": "plan it" },
        { "id": "msg_b", "type": "assistant", "content": [call("c1", "completed", json!([
            { "content": "First", "status": "in_progress" },
            { "content": "Second", "status": "pending" },
        ]))] },
        { "id": "msg_c", "type": "assistant", "content": [
            call("c2", "completed", json!([
                { "content": "First", "status": "completed" },
                { "content": "Second", "status": "in_progress" },
                { "content": "", "status": "pending" },
                { "content": "Third", "status": "cancelled" },
                { "content": "Fourth", "status": "whatever" },
            ])),
            call("c3", "running", json!([{ "content": "Not yet", "status": "pending" }])),
        ] },
    ]);
    Transcript { session: "ses_t".into(), messages: msgs.as_array().unwrap().iter().filter_map(parse_message).collect() }
}

fn has(rt: &Runtime<App>, key: &str) -> bool {
    rt.elements().into_iter().any(|e| !e.exiting && e.key.as_deref() == Some(key))
}

fn tap(rt: &mut Runtime<App>, key: &str) {
    rt.frame(DT);
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no {key}"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(DT);
}

/// The fixture's plan session, open and running.
fn plan() -> Runtime<App> {
    let mut rt = rt();
    rt.update(|s, tx| s.open(SIXTH, tx));
    rt.frame(DT);
    rt
}

#[test]
fn the_plan_is_the_latest_finished_call_before_a_revert() {
    let t = transcript();
    let todos = session_todos(&t, None);
    let names: Vec<_> = todos.iter().map(|t| (t.content.as_str(), t.status)).collect();
    assert_eq!(
        names,
        [("First", Status::Completed), ("Second", Status::InProgress), ("Third", Status::Cancelled), ("Fourth", Status::Pending)],
        "a running call doesn't count yet; empty todos drop and unknown statuses are pending"
    );
    let before = session_todos(&t, Some("msg_c"));
    assert_eq!(before.len(), 2, "a staged revert at msg_c leaves the first plan");
    assert_eq!(before[0].status, Status::InProgress);
    assert!(session_todos(&t, Some("msg_b")).is_empty());
}

#[test]
fn the_current_todo_is_in_progress_then_pending_then_last_done() {
    let todo = |c: &str, status| Todo { content: c.into(), status };
    let list = [todo("a", Status::Completed), todo("b", Status::Pending), todo("c", Status::InProgress)];
    assert_eq!(current(&list).unwrap().content, "c");
    let list = [todo("a", Status::Completed), todo("b", Status::Pending), todo("c", Status::Pending)];
    assert_eq!(current(&list).unwrap().content, "b");
    let list = [todo("a", Status::Completed), todo("b", Status::Completed), todo("c", Status::Cancelled)];
    assert_eq!(current(&list).unwrap().content, "b");
    assert!(current(&[]).is_none());
}

#[test]
fn the_pill_names_the_todo_in_hand_and_opens_the_list() {
    let mut rt = plan();
    assert!(has(&rt, "todo-pill"));
    let now = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("todo-now")).and_then(|e| e.text).unwrap();
    assert_eq!(now, "Map old keys to new ones in a migration step");
    let pill = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("todo-pill")).unwrap();
    assert_eq!(pill.label.as_deref(), Some("2 of 6 todos done"));
    assert!(!has(&rt, "todo-more"));

    tap(&mut rt, "todo-pill");
    assert!(rt.state.todo_open);
    assert!(has(&rt, "todo-panel"), "at once, no fade in");
    let panel = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("todo-panel")).unwrap();
    assert!(panel.opacity > 0.99);
    assert!((0..6).all(|k| has(&rt, &format!("todo-{k}"))));
    // Above the pill, start edges aligned.
    assert!(panel.rect[1] + panel.rect[3] <= pill.rect[1]);
    assert!((panel.rect[0] - pill.rect[0]).abs() < 1.0);

    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(DT);
    assert!(!rt.state.todo_open && !has(&rt, "todo-panel"));

    tap(&mut rt, "todo-pill");
    tap(&mut rt, "todo-dismiss");
    assert!(!rt.state.todo_open, "a click outside closes it");
}

#[test]
fn the_pill_shows_only_while_the_run_has_work_left() {
    let mut rt = plan();
    assert!(has(&rt, "todo-pill"));
    rt.update(|s, _| {
        s.running.remove(SIXTH);
    });
    rt.frame(DT);
    assert!(!has(&rt, "todo-pill"), "the run ended");

    rt.update(|s, _| {
        s.running.insert(SIXTH.into());
        crate::todo_stories::set_step(s, crate::todo_stories::PLAN.len());
    });
    rt.frame(DT);
    assert!(!has(&rt, "todo-pill"), "every todo is finished");

    rt.update(|s, _| crate::todo_stories::set_step(s, 0));
    rt.frame(DT);
    assert!(has(&rt, "todo-pill"));
}

#[test]
fn more_than_one_in_progress_shows_a_count() {
    let mut rt = plan();
    rt.update(|s, _| {
        crate::todo_stories::set_plan(s, json!({ "todos": [
            { "content": "One", "status": "in_progress" },
            { "content": "Two", "status": "in_progress" },
            { "content": "Three", "status": "in_progress" },
        ] }));
    });
    rt.frame(DT);
    let more = rt.elements().into_iter().find(|e| e.path.contains("todo-more") && e.text.is_some()).and_then(|e| e.text).unwrap();
    assert_eq!(more, "+2");
}

#[test]
fn one_list_opens_at_a_time() {
    let mut rt = plan();
    rt.update(|s, _| {
        s.running.insert(CHILD.into());
    });
    rt.update(|s, tx| s.open(crate::fixture_ids::MAIN, tx));
    rt.update(|s, _| {
        crate::todo_stories::set_step(s, 1);
        let main = crate::fixture_ids::MAIN.to_owned();
        s.running.insert(main);
    });
    rt.frame(DT);
    assert_eq!(rt.state.route, Route::Session(crate::fixture_ids::MAIN.into()));
    assert!(has(&rt, "activity-chip") && has(&rt, "todo-pill"));
    let chip = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("activity-chip")).unwrap().rect;
    let pill = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("todo-pill")).unwrap().rect;
    assert!((pill[0] - (chip[0] + chip[2] + 8.0)).abs() < 0.5, "beside the capsule, 8 apart: {chip:?} {pill:?}");

    tap(&mut rt, "activity-chip");
    assert!(rt.state.activity_open && !has(&rt, "todo-panel"));
    tap(&mut rt, "activity-dismiss");
    tap(&mut rt, "todo-pill");
    assert!(rt.state.todo_open && !rt.state.activity_open);
    assert!(has(&rt, "todo-panel") && !has(&rt, "activity-panel"));
}

#[test]
fn opening_another_session_closes_the_list() {
    let mut rt = plan();
    tap(&mut rt, "todo-pill");
    rt.update(|s, tx| s.open(crate::fixture_ids::MAIN, tx));
    assert!(!rt.state.todo_open);
}

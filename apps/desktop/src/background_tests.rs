use std::sync::mpsc::Receiver;

use opencode_kit::client::Write;
use opencode_kit::model::{Message, Part, ToolStatus};
use rune::{Key, KeyEvent, Mods, Runtime, TextEdit};
use serde_json::json;

use crate::{App, Route};

const REPLY: &str = "msg_zzzzbgreply";

/// The fixture app on its open session, running, with a channel standing in for the server's
/// writes when `live`.
fn app(live: bool) -> (Runtime<App>, Option<Receiver<Write>>, String) {
    let mut app = App::fixture_session();
    let Route::Session(id) = app.route.clone() else { panic!("no open session") };
    let writes = live.then(|| app.fake_writes());
    app.apply_event("session.execution.started", &json!({ "sessionID": id }));
    app.apply_event("session.step.started", &json!({ "sessionID": id, "assistantMessageID": REPLY, "agent": "build", "model": { "id": "m", "providerID": "p" } }));
    app.running.insert(id.clone());
    let mut rt = app.into_runtime();
    frames(&mut rt, 30);
    (rt, writes, id)
}

fn shown(rt: &Runtime<App>, key: &str) -> Option<rune::ElementInfo> {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key))
}

fn frames(rt: &mut Runtime<App>, n: usize) {
    for _ in 0..n {
        rt.frame(1.0 / 60.0);
    }
}

fn call(rt: &mut Runtime<App>, id: &str, call: &str, name: &str) {
    let input = if name == "shell" { json!({ "command": "cargo test" }) } else { json!({ "agent": "general", "description": "Look around" }) };
    rt.update(|s, _| {
        s.apply_event("session.tool.called", &json!({ "sessionID": id, "assistantMessageID": REPLY, "id": call, "name": name, "input": input }));
    });
    frames(rt, 2);
}

fn steer(rt: &mut Runtime<App>, text: &str) -> String {
    rt.update(|s, tx| {
        s.composer = TextEdit::new(text);
        s.submit(false, tx);
    });
    frames(rt, 2);
    rt.state.inbox_for(&session(rt)).and_then(|i| i.steering().last()).map(|i| i.id.clone()).expect("the steer waits")
}

fn session(rt: &Runtime<App>) -> String {
    let Route::Session(id) = rt.state.route.clone() else { panic!("no open session") };
    id
}

fn enqueued(rt: &mut Runtime<App>, sid: &str, inbox: &str) {
    rt.update(|s, _| {
        let item = json!({ "type": "user", "delivery": "steer", "payload": { "text": "x", "metadata": { "displayText": "x" } } });
        s.apply_event("session.inbox.enqueued", &json!({ "sessionID": sid, "inboxID": inbox, "item": item }));
    });
    frames(rt, 2);
}

fn moves(writes: &Receiver<Write>) -> Vec<(String, bool)> {
    writes.try_iter().filter_map(|w| match w {
        Write::Background { session, quiet } => Some((session, quiet)),
        _ => None,
    }).collect()
}

fn ctrl_b() -> KeyEvent {
    KeyEvent { key: Key::Text("b".into()), mods: Mods { ctrl: true, ..Mods::default() } }
}

#[test]
fn a_steer_moves_a_blocking_shell_once_admitted() {
    let (mut rt, writes, id) = app(true);
    let writes = writes.unwrap();
    call(&mut rt, &id, "call_bg_shell", "shell");
    let inbox = steer(&mut rt, "Stop and use the other test");
    assert!(moves(&writes).is_empty(), "nothing moves before the server admits the steer");
    enqueued(&mut rt, &id, &inbox);
    assert_eq!(moves(&writes), vec![(id.clone(), true)], "one quiet move after admission");
    enqueued(&mut rt, &id, &inbox);
    assert!(moves(&writes).is_empty(), "an echo again moves nothing");
}

#[test]
fn work_that_starts_during_admission_moves_too() {
    let (mut rt, writes, id) = app(true);
    let writes = writes.unwrap();
    let inbox = steer(&mut rt, "Also run the subagent");
    call(&mut rt, &id, "call_bg_task", "subagent");
    enqueued(&mut rt, &id, &inbox);
    assert_eq!(moves(&writes), vec![(id, true)]);
}

#[test]
fn nothing_blocking_makes_no_call() {
    let (mut rt, writes, id) = app(true);
    let writes = writes.unwrap();
    let inbox = steer(&mut rt, "Keep going");
    enqueued(&mut rt, &id, &inbox);
    assert!(moves(&writes).is_empty());
    assert!(!crate::background::key(&mut rt.state, &ctrl_b(), &mut rune::Tx::default()), "⌃B passes on to the field");
}

#[test]
fn a_normal_send_or_another_sessions_steer_makes_no_call() {
    let (mut rt, writes, id) = app(true);
    let writes = writes.unwrap();
    call(&mut rt, &id, "call_bg_shell", "shell");
    // A steer some other client sent.
    enqueued(&mut rt, &id, "msg_zzzzothersteer");
    // This app's steer, admitted under another session.
    let inbox = steer(&mut rt, "Not yet");
    enqueued(&mut rt, "ses_someoneelse", &inbox);
    assert!(moves(&writes).is_empty());
    // A send to an idle session.
    rt.update(|s, tx| {
        s.running.clear();
        s.apply_event("session.execution.succeeded", &json!({ "sessionID": id }));
        s.composer = TextEdit::new("Thanks");
        s.submit(false, tx);
    });
    frames(&mut rt, 2);
    assert!(moves(&writes).is_empty());
}

#[test]
fn ctrl_b_and_the_hint_move_and_report() {
    let (mut rt, writes, id) = app(true);
    let writes = writes.unwrap();
    assert!(shown(&rt, "background-hint").is_none(), "no hint while nothing blocks");
    call(&mut rt, &id, "call_bg_shell", "shell");
    frames(&mut rt, 4);
    let hint = shown(&rt, "background-hint").expect("the hint shows beside Working");
    assert_eq!(hint.label.as_deref(), Some(crate::background::hint_label().as_str()));
    rt.key(ctrl_b());
    frames(&mut rt, 2);
    assert_eq!(moves(&writes), vec![(id.clone(), false)], "⌃B reports its failure");
    let r = shown(&rt, "background-hint").unwrap().rect;
    rt.pointer_move(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.pointer_down(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.pointer_up(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    frames(&mut rt, 2);
    assert_eq!(moves(&writes), vec![(id, false)]);
}

#[test]
fn the_fixture_server_returns_the_blocking_call() {
    let (mut rt, _, id) = app(false);
    call(&mut rt, &id, "call_bg_shell", "shell");
    steer(&mut rt, "Stop and use the other test");
    assert_eq!(rt.state.background.fixture_moves, vec![id.clone()], "the fixture admits at once and moves once");
    frames(&mut rt, 30);
    let status = rt.state.transcripts[&id].messages.iter().find_map(|m| match m {
        Message::Assistant(a) => a.parts.iter().find_map(|p| match p {
            Part::Tool { id, status, .. } if id == "call_bg_shell" => Some(status.clone()),
            _ => None,
        }),
        _ => None,
    });
    assert_eq!(status, Some(ToolStatus::Completed));
    assert!(shown(&rt, "background-hint").is_none(), "the hint goes with the blocking work");
}

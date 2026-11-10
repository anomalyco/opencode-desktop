//! A prompt sent while earlier steers still wait takes its place behind them, where the server
//! will deliver it, so nothing reorders when the server confirms.

use rune::{Runtime, TextEdit};
use serde_json::json;

use crate::{App, Route};

fn busy_app() -> (Runtime<App>, String) {
    let mut app = App::fixture_session();
    let Route::Session { id, .. } = app.route.clone() else { panic!("no open session") };
    app.running.insert(id.clone());
    let mut rt = app.into_runtime();
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    (rt, id)
}

fn enter(rt: &mut Runtime<App>, body: &str) -> String {
    rt.update(|s, tx| {
        s.composer = TextEdit::new(body);
        s.send(tx);
    });
    rt.frame(1.0 / 120.0);
    let sid = match &rt.state.route { Route::Session { id: s, .. } => s.clone(), _ => unreachable!() };
    let t = &rt.state.transcripts[&sid];
    let in_inbox = rt.state.inboxes.get(&sid).and_then(|i| i.items.iter().find(|i| i.text == body)).map(|i| i.id.clone());
    in_inbox.or_else(|| t.messages.iter().rev().find(|m| matches!(m, crate::Message::User { text, .. } if text == body)).map(|m| m.id().to_owned())).expect("sent")
}

/// The keys of the open timeline's rows, top to bottom, as laid out.
fn order(rt: &Runtime<App>, sid: &str) -> Vec<String> {
    session_view::timeline::row_keys(&rt.state, &rt.state.transcripts[sid])
}

fn positions(rows: &[String], ids: &[String]) -> Vec<usize> {
    ids.iter().map(|id| rows.iter().position(|k| *k == format!("u-{id}")).unwrap_or_else(|| panic!("u-{id} missing from {rows:?}"))).collect()
}

#[test]
fn a_send_after_a_stop_waits_behind_the_earlier_steers() {
    let (mut rt, sid) = busy_app();
    let a = enter(&mut rt, "or did he give me stuff?");
    let b = enter(&mut rt, "can you check");
    // The run stops with both steers still waiting, as when Kit's reply was interrupted.
    rt.update(|s, _| {
        s.apply_event("session.execution.interrupted", &json!({ "sessionID": sid, "reason": "user" }));
    });
    rt.frame(1.0 / 120.0);
    assert!(!rt.state.busy(&sid), "the run ended");
    let c = enter(&mut rt, "this is really weird");
    let d = enter(&mut rt, "what");
    let ids = [a.clone(), b.clone(), c.clone(), d.clone()];

    let rows = order(&rt, &sid);
    let at = positions(&rows, &ids);
    assert!(at.windows(2).all(|w| w[0] < w[1]), "in send order: {rows:?}");
    if let Some(working) = rows.iter().position(|k| k == "turn-actions") {
        assert!(at[0] > working, "all wait under Working: {rows:?}");
    }
    let waiting: Vec<String> = rt.state.inboxes[&sid].steering().map(|i| i.id.clone()).collect();
    assert_eq!(waiting, ids, "each waits in the inbox, behind the ones before it");

    // The server delivers them in the order it queued them; nothing moves relative to the rest.
    for id in &ids {
        rt.update(|s, _| {
            s.apply_event("session.inbox.delivered", &json!({ "sessionID": sid, "inboxID": id }));
        });
        rt.frame(1.0 / 120.0);
        let rows = order(&rt, &sid);
        let now = positions(&rows, &ids);
        assert!(now.windows(2).all(|w| w[0] < w[1]), "after delivering {id}, still in send order: {rows:?}");
    }
    let users: Vec<&str> = rt.state.transcripts[&sid].messages.iter().filter_map(|m| match m {
        crate::Message::User { id, .. } => Some(id.as_str()),
        _ => None,
    }).collect();
    assert_eq!(&users[users.len() - 4..], ids.iter().map(String::as_str).collect::<Vec<_>>(), "the transcript ends in the server's order");
}

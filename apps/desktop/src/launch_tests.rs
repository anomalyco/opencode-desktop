use std::sync::mpsc::channel;

use opencode_kit::client::{Incoming, Server};
use rune::Tx;
use serde_json::{Value, json};

use super::{First, poll};
use crate::{App, FIXTURE, Source};

#[test]
fn first_reads_land_in_one_poll() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let mut app = App::empty();
    app.source = Source::Live { server: Server::new("http://127.0.0.1:9"), requests: None, writes: None, allow_send: false };
    app.launch.waiting = true;
    app.launch.recent_tabs = true;
    let (out, rx) = channel();
    app.launch.first = Some(rx);
    assert!(!poll(&mut app, &mut Tx::default()), "nothing has landed");
    let first = First { projects: fixture["projects"].clone(), sessions: Some(fixture["sessions"].clone()), models: None, active: Some(json!({ "data": { "ses_fx0006aaaaaaaaaaaaaaaaaa": {} } })) };
    out.send(Ok(first)).unwrap();
    assert!(poll(&mut app, &mut Tx::default()));
    assert!(!app.launch.waiting && app.launch.first.is_none());
    assert!(!app.projects.is_empty());
    assert_eq!(app.tabs.first().map(|t| t.id.as_str()), Some("ses_fx0001aaaaaaaaaaaaaaaaaa"), "no saved tabs: the recent sessions");
    assert!(app.running.contains("ses_fx0006aaaaaaaaaaaaaaaaaa"));
    assert!(!poll(&mut app, &mut Tx::default()), "and then rests");
}

#[test]
fn another_sessions_stream_does_not_redraw() {
    let mut app = App::empty();
    let (send, inbox) = channel();
    app.inbox = Some(inbox);
    send.send(Incoming::Event { ty: "session.text.delta".into(), data: json!({ "sessionID": "ses_elsewhere", "delta": "hi" }) }).unwrap();
    assert!(!crate::poll(&mut app, 0.0, &mut Tx::default()));
    send.send(Incoming::Event { ty: "session.execution.started".into(), data: json!({ "sessionID": "ses_elsewhere" }) }).unwrap();
    assert!(crate::poll(&mut app, 0.1, &mut Tx::default()), "its spinner starts");
}

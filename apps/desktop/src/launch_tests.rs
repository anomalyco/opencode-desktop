use std::sync::mpsc::channel;

use opencode_kit::client::{Incoming, Server};
use rune::Tx;
use serde_json::{Value, json};

use super::{First, poll};
use crate::{App, FIXTURE, Route, Source};

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
    let first = First { projects: fixture["projects"].clone(), sessions: Some(fixture["sessions"].clone()), models: None, active: Some(json!({ "data": { crate::fixture_ids::SIXTH: {} } })) };
    out.send(Ok(first)).unwrap();
    assert!(poll(&mut app, &mut Tx::default()));
    assert!(!app.launch.waiting && app.launch.first.is_none());
    assert!(!app.projects.is_empty());
    assert_eq!(app.tabs.first().map(|t| t.id.as_str()), Some(crate::fixture_ids::MAIN), "no saved tabs: the recent sessions");
    assert!(app.running.contains(crate::fixture_ids::SIXTH));
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

fn spinner(rt: &rune::Runtime<App>) -> bool {
    rt.elements().iter().any(|e| !e.exiting && e.path.contains("working") && e.key.as_deref() == Some("d0"))
}

#[test]
fn a_run_that_ended_during_a_stream_gap_stops_after_reconnect() {
    use opencode_kit::client::Request;
    use rune::{Env, Runtime};
    let mut app = App::empty();
    app.load_fixture(FIXTURE);
    crate::open_scene(&mut app);
    app.finish_stream();
    let Route::Session(id) = app.route.clone() else { panic!("no open session") };
    app.running.insert(id.clone());
    let (requests, asked) = channel();
    let (send, inbox) = channel();
    app.source = Source::Live { server: Server::new("http://127.0.0.1:9"), requests: Some(requests), writes: None, allow_send: false };
    app.inbox = Some(inbox);
    let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
    rt.poll = Some(crate::poll);
    for _ in 0..30 {
        rt.frame(1.0 / 120.0);
    }
    assert!(rt.state.busy(&id));
    assert!(spinner(&rt), "the spinner shows");

    // The stream drops, the run ends in the gap, the stream comes back.
    send.send(Incoming::Reconnected).unwrap();
    for _ in 0..4 {
        rt.frame(1.0 / 120.0);
    }
    let reads: Vec<Request> = asked.try_iter().collect();
    assert!(reads.iter().any(|r| matches!(r, Request::Active)), "it re-reads what's running");
    assert!(reads.iter().any(|r| matches!(r, Request::Pending(s) if *s == id)), "and the open session's requests");
    assert!(reads.iter().any(|r| matches!(r, Request::Inbox(s) if *s == id)));
    assert!(rt.state.busy(&id), "nothing changes until the read lands");
    send.send(Incoming::Active(vec![])).unwrap();
    for _ in 0..240 {
        rt.frame(1.0 / 120.0);
    }
    assert!(rt.state.running.is_empty() && !rt.state.busy(&id));
    assert!(!spinner(&rt), "the spinner is gone");
    let working = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("working")).expect("the reserved row");
    assert_eq!(working.opacity, 0.0, "the Working row is hidden");
    assert!(!rt.frame(1.0 / 120.0), "and the window rests");
    assert!(!rt.frame(1.0 / 120.0));
}

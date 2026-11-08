use std::sync::mpsc::channel;

use opencode_kit::client::{Incoming, Server};
use rune::Tx;
use serde_json::{Value, json};

use super::{First, Ready, poll};
use crate::{App, FIXTURE, Route, Source};

#[test]
fn first_reads_land_in_one_poll() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let mut app = App::empty();
    app.source = Source::Live { server: Server::new("http://127.0.0.1:9"), requests: None, writes: None, allow_send: false };
    app.launch.waiting = true;
    let (out, rx) = channel();
    app.launch.first = Some(rx);
    assert!(!poll(&mut app, &mut Tx::default()), "nothing has landed");
    let first = First { projects: fixture["projects"].clone(), sessions: Some(fixture["sessions"].clone()), models: None, providers: None, active: Some(json!({ "data": { crate::fixture_ids::SIXTH: {} } })), config: None };
    out.send(Ready { server: None, beta_tabs: None, first: Some(Ok(first)) }).unwrap();
    assert!(poll(&mut app, &mut Tx::default()));
    assert!(!app.launch.waiting && app.launch.first.is_none());
    assert!(!app.projects.is_empty());
    assert!(app.tabs.is_empty(), "no saved tabs: none, as Desktop's first launch");
    assert!(app.running.contains(crate::fixture_ids::SIXTH));
    assert!(!poll(&mut app, &mut Tx::default()), "and then rests");
}

#[test]
fn deferred_beta_tabs_win_and_unreachable_falls_back_to_fixture() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let mut app = App::empty();
    app.source = Source::Live { server: Server::new("http://127.0.0.1:9"), requests: None, writes: None, allow_send: false };
    app.launch.waiting = true;
    app.route = Route::Session("ses_gone".into());
    let (out, rx) = channel();
    app.launch.first = Some(rx);
    let beta = vec![crate::tabs::Tab::session(crate::fixture_ids::THIRD, "Pinned", "/w")];
    let first = First { projects: fixture["projects"].clone(), sessions: Some(fixture["sessions"].clone()), models: None, providers: None, active: None, config: None };
    out.send(Ready { server: None, beta_tabs: Some(beta.clone()), first: Some(Ok(first)) }).unwrap();
    assert!(poll(&mut app, &mut Tx::default()));
    assert_eq!(app.tabs, beta, "Beta's tabs win over recent sessions");
    assert_eq!(app.route, Route::Home, "unknown restored session falls back to Home");

    // Unreachable background probe falls back to the built-in fixture when explicitly requested.
    let mut offline = App::empty();
    offline.source = Source::Live { server: Server::new("http://127.0.0.1:9"), requests: None, writes: None, allow_send: false };
    offline.launch.waiting = true;
    offline.launch.fallback_fixture = true;
    let (out, rx) = channel();
    offline.launch.first = Some(rx);
    out.send(Ready { server: None, beta_tabs: None, first: None }).unwrap();
    assert!(poll(&mut offline, &mut Tx::default()));
    assert!(matches!(offline.source, Source::Fixture));
    assert!(!offline.sessions.is_empty());
}

#[test]
fn unreachable_live_launch_shows_no_server_instead_of_fixture() {
    use rune::{Env, Runtime};
    let mut app = App::empty();
    app.source = Source::Live { server: Server::new("http://127.0.0.1:4096"), requests: None, writes: None, allow_send: false };
    app.launch.waiting = true;
    app.route = Route::Session("ses_restored".into());
    app.tabs.push(crate::tabs::Tab::session("ses_restored", "Old", "/w"));
    let (out, rx) = channel();
    app.launch.first = Some(rx);
    out.send(Ready {
        server: Some(Server::new("http://127.0.0.1:4096")),
        beta_tabs: None,
        first: Some(Err("No OpenCode server responded at http://127.0.0.1:4096".into())),
    })
    .unwrap();
    assert!(poll(&mut app, &mut Tx::default()));
    assert!(app.live(), "never silently falls back to FIXTURE");
    assert!(app.launch.offline);
    assert!(app.sessions.is_empty());
    assert!(app.tabs.is_empty());
    assert_eq!(app.route, Route::Home);

    let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
    rt.frame(1.0 / 60.0);
    assert!(rt.find("no-server").is_some(), "renders the no-server setup card on Home");
    assert!(rt.find("no-server-retry").is_some(), "renders the Retry button");
}

#[test]
fn synchronous_live_load_enters_offline_instead_of_fixture() {
    let mut fallback = App::empty();
    assert!(!fallback.load_live(Server::new("http://127.0.0.1:9"), true));

    let mut live = App::empty();
    assert!(live.load_live(Server::new("http://127.0.0.1:9"), false));
    assert!(live.live());
    assert!(live.launch.offline);
    assert_eq!(live.route, Route::Home);
    assert!(live.tabs.is_empty());
    super::setup(&mut live);
    assert!(live.launch.first.is_none(), "offline setup does not spin up a second background probe");
}

#[test]
fn idle_poll_allocates_nothing_with_open_transcript() {
    let mut app = App::fixture_session();
    let _ = crate::poll(&mut app, 0.0, &mut Tx::default());
    let before = procwatch::alloc::thread_allocations();
    assert!(!crate::poll(&mut app, 1.0 / 120.0, &mut Tx::default()));
    let after = procwatch::alloc::thread_allocations();
    assert_eq!(after - before, 0, "an idle poll allocates nothing");
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

/// Working's shimmer, which sweeps only while the run is live.
fn shimmer(rt: &rune::Runtime<App>) -> bool {
    rt.elements().iter().any(|e| !e.exiting && e.path.contains("/working/") && e.key.as_deref() == Some("sheen"))
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
    assert!(shimmer(&rt), "Working shimmers");

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
    assert!(!shimmer(&rt), "the shimmer is gone");
    let working = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("working")).expect("the reserved row");
    assert_eq!(working.opacity, 0.0, "the Working row is hidden");
    assert!(!rt.frame(1.0 / 120.0), "and the window rests");
    assert!(!rt.frame(1.0 / 120.0));
}

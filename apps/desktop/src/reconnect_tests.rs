use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use opencode_kit::client::{Incoming, Request, Server};
use rune::{Env, Runtime, Tx};

use super::RECONNECTING;
use crate::{App, FIXTURE, Route, Source};

const DT: f64 = 1.0 / 60.0;

fn toast(rt: &Runtime<App>) -> Option<String> {
    rt.state.toast.current.as_ref().map(|e| e.title.clone())
}

fn frames(rt: &mut Runtime<App>, seconds: f64) {
    for _ in 0..(seconds / DT).ceil() as usize {
        rt.frame(DT);
    }
}

/// A connected session page with a typed draft, its requests and its inbox.
fn connected() -> (Runtime<App>, Sender<Incoming>, std::sync::mpsc::Receiver<Request>, String) {
    let mut app = App::empty();
    app.load_fixture(FIXTURE);
    crate::open_scene(&mut app);
    app.finish_stream();
    let Route::Session(id) = app.route.clone() else { panic!("no open session") };
    let (requests, asked) = channel();
    let (send, inbox) = channel();
    app.source = Source::Live { server: Server::new("http://127.0.0.1:9"), requests: Some(requests), writes: None, allow_send: false };
    app.inbox = Some(inbox);
    app.composer.replace_text("half a thought");
    let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
    rt.poll = Some(crate::poll);
    frames(&mut rt, 0.5);
    (rt, send, asked, id)
}

#[test]
fn a_dropped_stream_keeps_the_page_and_resyncs_when_back() {
    let (mut rt, send, asked, id) = connected();
    let sidebar = rt.state.tabs.len();
    let messages = rt.state.transcripts[&id].messages.len();
    assert!(messages > 0 && sidebar > 0);

    send.send(Incoming::Status("events: Connection refused".into())).unwrap();
    frames(&mut rt, 0.5);
    assert!(super::down(&rt.state));
    assert_eq!(toast(&rt), None, "a short drop says nothing");

    frames(&mut rt, super::QUIET);
    assert_eq!(toast(&rt).as_deref(), Some(RECONNECTING));
    // More failed tries while it stays down change nothing.
    send.send(Incoming::Status("events: Connection refused".into())).unwrap();
    frames(&mut rt, 0.5);
    assert_eq!(toast(&rt).as_deref(), Some(RECONNECTING));
    assert_eq!(rt.state.route, Route::Session(id.clone()), "the page stays");
    assert_eq!(rt.state.transcripts[&id].messages.len(), messages);
    assert_eq!(rt.state.tabs.len(), sidebar);
    assert_eq!(rt.state.composer.text, "half a thought");
    assert!(!rt.state.launch.offline && rt.find("no-server").is_none(), "no setup card");
    let _: Vec<Request> = asked.try_iter().collect();

    send.send(Incoming::Reconnected).unwrap();
    frames(&mut rt, 0.1);
    assert!(!super::down(&rt.state));
    assert_eq!(toast(&rt), None, "the toast leaves");
    let reads: Vec<Request> = asked.try_iter().collect();
    assert!(reads.iter().any(|r| matches!(r, Request::Active)), "it re-reads what's running");
    assert!(reads.iter().any(|r| matches!(r, Request::Sessions)));
    assert!(reads.iter().any(|r| matches!(r, Request::Inbox(s) if *s == id)), "and the open session");
    assert_eq!(rt.state.composer.text, "half a thought");
}

#[test]
fn a_drop_that_comes_back_quickly_says_nothing() {
    let (mut rt, send, _asked, _) = connected();
    send.send(Incoming::Status("events: stream ended".into())).unwrap();
    frames(&mut rt, 1.0);
    send.send(Incoming::Reconnected).unwrap();
    frames(&mut rt, super::QUIET + 0.5);
    assert_eq!(toast(&rt), None);
    assert!(!super::down(&rt.state));
}

#[test]
fn a_never_reached_server_shows_the_card_only_after_the_grace() {
    let mut app = App::empty();
    app.source = Source::Live { server: Server::new("http://127.0.0.1:9"), requests: None, writes: None, allow_send: false };
    app.launch.waiting = true;
    app.launch.grace = Some(Duration::from_millis(800));
    app.tabs.push(crate::tabs::Tab::session("ses_restored", "Old", "/w"));
    let start = Instant::now();
    crate::launch::start(&mut app, &mut Tx::default());
    let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
    rt.poll = Some(crate::poll);
    frames(&mut rt, 0.1);
    assert!(rt.find("no-server").is_none(), "no card on the first failed probe");
    while !rt.state.launch.offline && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
        rt.frame(DT);
    }
    assert!(rt.state.launch.offline, "it gives up after the grace");
    assert!(rt.state.status.starts_with("No OpenCode server responded"), "{}", rt.state.status);
    assert!(start.elapsed() >= Duration::from_millis(500), "and not before: {:?}", start.elapsed());
    rt.frame(DT);
    assert!(rt.find("no-server").is_some());
    assert!(rt.find("no-server-cmd").is_none(), "a non-default address gets no start command");
    assert!(rt.state.tabs.is_empty(), "the tabs wait behind the card");
}

#[test]
fn the_setup_card_names_the_command_only_for_the_local_default() {
    let mut app = App::empty();
    app.launch.offline = true;
    app.route = Route::Home;
    let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
    rt.frame(DT);
    assert!(rt.find("no-server-cmd").is_some(), "the default address: start a server");
    rt.update(|s, _| s.launch.story_addr = Some("http://127.0.0.1:14096".into()));
    rt.frame(DT);
    assert!(rt.find("no-server-cmd").is_none(), "another address: nothing to start");
}

#[test]
fn a_read_that_times_out_says_reconnecting_until_the_server_answers() {
    let (mut rt, send, _asked, id) = connected();
    send.send(Incoming::Status(format!("GET /api/session/{id}/message: timeout: global"))).unwrap();
    frames(&mut rt, super::QUIET + 0.2);
    assert_eq!(toast(&rt).as_deref(), Some(RECONNECTING), "a timeout is a drop");
    assert_eq!(rt.state.route, Route::Session(id.clone()));
    assert_eq!(rt.state.composer.text, "half a thought");
    send.send(Incoming::Active(vec![])).unwrap();
    frames(&mut rt, 0.1);
    assert_eq!(toast(&rt), None, "an answer ends it");

    // A 5xx is a busy server, not a lost link.
    send.send(Incoming::Status(format!("GET /api/session/{id}/message: http status: 503"))).unwrap();
    frames(&mut rt, super::QUIET + 0.2);
    assert_eq!(toast(&rt), None);
}

#[test]
fn a_suspect_link_whose_stream_stays_up_clears_on_its_own() {
    let (mut rt, send, _asked, _) = connected();
    send.send(Incoming::Status("GET /api/session: Connection refused".into())).unwrap();
    frames(&mut rt, super::QUIET + 0.2);
    assert_eq!(toast(&rt).as_deref(), Some(RECONNECTING));
    // The stream would have dropped by now if the link were dead.
    rt.state.launch.link.suspect_at = Some(rt.state.clock - super::STREAM_IDLE.as_secs_f64() - super::QUIET);
    frames(&mut rt, 0.2);
    assert_eq!(toast(&rt), None);
    assert!(!super::down(&rt.state));
}

/// Kit's lost HOME tab: a launch that can't reach the server, then a Retry that can, must
/// leave the saved tabs as they were, and nothing is saved behind the card.
#[test]
fn a_failed_launch_never_saves_away_a_tab() {
    use crate::launch::{First, Ready};
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    crate::tabs::SAVED.with(|s| *s.borrow_mut() = None);
    let mut app = App::empty();
    app.source = Source::Live { server: Server::new("http://127.0.0.1:9"), requests: None, writes: None, allow_send: false };
    app.launch.waiting = true;
    let home = crate::tabs::Tab::session("ses_home_not_in_list", "HOME", "/w");
    app.tabs = vec![home.clone()];
    let (out, rx) = channel();
    app.launch.set_first(rx);
    out.send(Ready { server: None, beta_tabs: None, first: Some(Err("No OpenCode server responded at http://127.0.0.1:9".into())) }).unwrap();
    assert!(crate::launch::poll(&mut app, &mut Tx::default()));
    assert!(app.launch.offline && app.tabs.is_empty());
    app.save_tabs();
    assert_eq!(crate::tabs::SAVED.with(|s| s.borrow().clone()), None, "nothing saved behind the card");

    // The server answers; its session list doesn't name the tab (a page past it, or a drop).
    let (out, rx) = channel();
    app.launch.set_first(rx);
    let first = First::from_parts(fixture["projects"].clone(), Some(fixture["sessions"].clone()));
    out.send(Ready { server: None, beta_tabs: None, first: Some(Ok(first)) }).unwrap();
    assert!(crate::launch::poll(&mut app, &mut Tx::default()));
    assert!(!app.launch.offline);
    assert_eq!(app.tabs.first(), Some(&home), "the saved tab is back, not swapped for recent sessions");
    app.save_tabs();
    let saved = crate::tabs::SAVED.with(|s| s.borrow().clone()).unwrap();
    assert!(saved.contains(&home.id), "{saved:?}");

    // Without the session list, the tabs aren't replaced by an empty "recent" list.
    let mut cold = App::empty();
    cold.launch.recent_tabs = true;
    cold.tabs = vec![home.clone()];
    cold.apply_first(First::from_parts(fixture["projects"].clone(), None));
    assert_eq!(cold.tabs, vec![home]);
}

/// Kit's stale "Starting" row: a subagent that worked on while the link was down shows its
/// latest step as soon as the link is back, from a fresh read of its page, not the next event.
#[test]
fn a_reconnect_brings_a_running_subagents_row_up_to_date() {
    use opencode_kit::model::{Message, Part, ToolStatus};
    const CHILD: &str = "ses_fx0002aaaaaaaaaaaaaaaaaa";
    const CALL: &str = "call_subagent_60500";
    let (mut rt, send, asked, id) = connected();
    // The subagent had just started when the link went: its call runs, its page isn't loaded.
    for m in &mut rt.state.transcripts.get_mut(&id).unwrap().messages {
        if let Message::Assistant(a) = m {
            for p in &mut a.parts {
                if let Part::Tool { id, status, .. } = p
                    && id == CALL
                {
                    *status = ToolStatus::Running;
                }
            }
        }
    }
    rt.state.transcripts.remove(CHILD);
    rt.state.running.insert(CHILD.into());
    let line = |s: &App| s.agent_run(&id, CALL, Some(CHILD), &ToolStatus::Running).line();
    assert_eq!(line(&rt.state), "Starting");

    send.send(Incoming::Status("events: timeout: per-read".into())).unwrap();
    frames(&mut rt, super::QUIET + 0.5);
    let _: Vec<Request> = asked.try_iter().collect();

    send.send(Incoming::Reconnected).unwrap();
    let mut asked_after = 0;
    while !asked.try_iter().any(|r| matches!(r, Request::Session(s) if s == CHILD)) {
        rt.frame(DT);
        asked_after += 1;
        assert!(asked_after < 60, "its page is read again");
    }
    // The server's answer: it has moved on to reading files.
    let page = serde_json::json!({ "data": [{
        "id": "msg_fx_child_late", "type": "assistant", "agent": "general",
        "model": { "id": "claude-opus-5-5", "providerID": "anthropic" },
        "content": [{ "type": "tool", "id": "call_read_late", "name": "read", "executed": true,
            "state": { "status": "running", "input": { "filePath": "src/net/backoff.ts" } } }],
        "time": { "created": 1792658365000i64 }
    }]});
    send.send(Incoming::Messages { session: CHILD.into(), messages: opencode_kit::model::parse_messages(&page) }).unwrap();
    let mut current_after = 0;
    while line(&rt.state) == "Starting" {
        rt.frame(DT);
        current_after += 1;
        assert!(current_after < 60, "the row catches up");
    }
    assert_eq!(line(&rt.state), "Read backoff.ts");
    eprintln!("reconnect catch-up: page asked {asked_after} frame(s) after the link came back, row current {current_after} frame(s) after it landed");
}

/// A subagent that started while the link was down: the next list of running sessions names
/// it, and the app looks up its family and reads its page.
#[test]
fn a_subagent_started_during_a_drop_is_read_after_the_reconnect() {
    let (mut rt, send, asked, _) = connected();
    send.send(Incoming::Status("events: stream ended".into())).unwrap();
    frames(&mut rt, 0.2);
    send.send(Incoming::Reconnected).unwrap();
    frames(&mut rt, 0.1);
    let _: Vec<Request> = asked.try_iter().collect();
    send.send(Incoming::Active(vec!["ses_fx_new_child".into()])).unwrap();
    frames(&mut rt, 0.1);
    let reads: Vec<Request> = asked.try_iter().collect();
    assert!(reads.iter().any(|r| matches!(r, Request::Info(s) if s == "ses_fx_new_child")), "its family");
    assert!(reads.iter().any(|r| matches!(r, Request::Session(s) if s == "ses_fx_new_child")), "its page");
    // Later lists while connected don't re-read pages.
    send.send(Incoming::Active(vec!["ses_fx_new_child".into()])).unwrap();
    frames(&mut rt, 0.1);
    assert!(!asked.try_iter().any(|r| matches!(r, Request::Session(_))));
}

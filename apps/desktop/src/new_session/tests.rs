use std::sync::mpsc::{Receiver, Sender, channel};

use opencode_kit::client::{Incoming, Write};
use opencode_kit::model::{Message, SessionInfo};
use rune::Runtime;

use crate::{App, Route};

const DT: f64 = 1.0 / 60.0;
const DIR: &str = "/Users/alex/code/harbor";

struct Live {
    rt: Runtime<App>,
    writes: Receiver<Write>,
    /// What the write thread sends back.
    server: Sender<Incoming>,
}

/// The new-session page on a live source, "start" typed in its composer.
fn draft() -> Live {
    let mut app = App::fixture_session();
    let writes = app.fake_writes();
    let (server, inbox) = channel();
    app.inbox = Some(inbox);
    let mut rt = app.into_runtime();
    rt.update(|s, tx| {
        s.new_session(tx);
        s.set_draft_dir(DIR.into());
    });
    rt.frame(DT);
    rt.update(|s, _| s.composer = rune::TextEdit::new("start"));
    rt.frame(DT);
    Live { rt, writes, server }
}

/// Sends, and returns the new session's id and the prompt's.
fn send(l: &mut Live) -> (String, String) {
    l.rt.update(|s, tx| s.send(tx));
    let Ok(Write::StartAs { session, directory, prompt }) = l.writes.try_recv() else { panic!("no start") };
    assert_eq!(directory, DIR);
    assert!(l.writes.try_recv().is_err(), "one write starts it");
    (session, prompt.id.unwrap())
}

fn working(rt: &Runtime<App>) -> bool {
    rt.elements().iter().any(|e| e.key.as_deref() == Some("working") && e.opacity > 0.99)
}

#[test]
fn send_shows_the_new_session_with_its_prompt_and_working_at_once() {
    let mut l = draft();
    let (id, prompt) = send(&mut l);
    assert!(id.starts_with("ses_") && id.len() == 30, "{id}");
    let s = &l.rt.state;
    assert_eq!(s.route, Route::here(id.clone()), "the page is the new session's on Send");
    assert!(s.starts.has(&id));
    assert_eq!(s.session(&id).map(|i| i.directory.as_str()), Some(DIR));
    assert!(s.tabs.iter().any(|t| t.id == id), "it takes a tab");
    assert!(matches!(s.transcripts[&id].messages.last(), Some(Message::User { id: m, text }) if *m == prompt && text == "start"));
    assert!(s.composer.text.is_empty());
    l.rt.frame(DT);
    assert!(l.rt.find(&format!("u-{prompt}")).is_some(), "the prompt is on the first frame");
    assert!(working(&l.rt), "Working shows under it");
}

#[test]
fn the_servers_session_lands_without_a_reload() {
    let mut l = draft();
    let (id, prompt) = send(&mut l);
    l.rt.frame(DT);
    let info = SessionInfo { id: id.clone(), directory: DIR.into(), title: "Fix the sidebar".into(), ..Default::default() };
    l.server.send(Incoming::Created(info)).unwrap();
    l.rt.frame(DT);
    let s = &l.rt.state;
    assert!(!s.starts.has(&id));
    assert_eq!(s.route, Route::here(id.clone()), "still on it");
    assert_eq!(s.sessions.iter().filter(|i| i.id == id).count(), 1);
    assert_eq!(s.session(&id).unwrap().title, "Fix the sidebar");
    assert!(s.opening.is_none(), "nothing holds the page");
    assert!(l.rt.find(&format!("u-{prompt}")).is_some());
    assert!(working(&l.rt));
}

#[test]
fn a_disconnected_start_keeps_its_tab_and_prompt_for_reconnect() {
    let mut l = draft();
    let (id, prompt) = send(&mut l);
    l.rt.frame(DT);
    l.server.send(Incoming::SendFailed { session: id.clone(), id: prompt.clone(), error: "POST /api/session: Connection refused".into() }).unwrap();
    l.rt.frame(DT);
    let s = &l.rt.state;
    assert_eq!(s.route, Route::here(id.clone()));
    assert_eq!(s.unsent.state(&prompt),Some(crate::send_failed::State::Waiting));
    assert!(s.transcripts[&id].messages.iter().any(|m| m.id()==prompt));
    assert!(s.toast.current().is_none());
}

#[test]
fn a_fixture_start_opens_and_replies() {
    let mut rt = App::fixture_session().into_runtime();
    rt.update(|s, tx| s.new_session(tx));
    rt.frame(DT);
    rt.update(|s, tx| {
        s.composer = rune::TextEdit::new("start");
        s.send(tx);
    });
    let Route::Session { id, .. } = rt.state.route.clone() else { panic!("no session") };
    assert!(!rt.state.starts.has(&id), "a fixture has nothing to wait for");
    assert!(rt.state.playback.iter().any(|e| e.2["sessionID"] == id.as_str()), "its reply is on the way");
    rt.frame(DT);
    assert!(working(&rt));
}

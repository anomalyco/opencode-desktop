//! A prompt the server never took rolls back; one that timed out waits as "Sending…" until the
//! server answers, then is kept or rolled back by what the session holds.
use std::sync::Arc;
use std::time::{Duration, Instant};

use opencode_kit::model::{Message, Transcript};
use rune::{Tx, TextEdit};
use serde_json::json;

use super::*;
use crate::session_pages::Pages;
use crate::{App, Route};

const SES: &str = "ses_a";
const ID: &str = "msg_mine";

fn app_with_echo(inbox: bool) -> App {
    let mut app = App::empty();
    let _writes = app.fake_writes();
    std::mem::forget(_writes);
    app.route = Route::Session(SES.into());
    app.transcripts.insert(SES.into(), Transcript { session: SES.into(), messages: vec![] });
    let draft = Entry::of(&TextEdit::new("hey"));
    if !inbox {
        app.transcripts.get_mut(SES).unwrap().messages.push(Message::User { id: ID.into(), text: "hey".into() });
        app.attach.echoes.push((SES.into(), ID.into(), "hey".into()));
    }
    app.unsent.note(Sent::new(SES, ID, draft, vec![], vec![], inbox));
    app
}

fn has_echo(app: &App) -> bool {
    app.transcripts[SES].messages.iter().any(|m| m.id() == ID)
}

fn serve(app: &mut App, messages: serde_json::Value) {
    let page = json!({"data": messages, "cursor": {"next": null}});
    app.pages.set(Pages::new(Arc::new(move |_| Ok(page.clone())), None, Arc::new(|| {})));
}

fn settle(app: &mut App, until: impl Fn(&App) -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        app.poll_pages(&mut Tx::default());
        if until(app) { return true; }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
fn a_refused_send_rolls_back_at_once_and_returns_to_the_composer() {
    let mut app = app_with_echo(false);
    assert!(app.send_failed(ID, "POST /api/session/ses_a/prompt: http status: 422"));
    assert!(!has_echo(&app));
    assert!(app.attach.echoes.is_empty());
    assert_eq!(app.composer.text, "hey");
    let toast = app.toast.current.clone().unwrap();
    assert_eq!((toast.title.as_str(), toast.description.as_deref()), ("Failed to send prompt", Some("http status: 422")));
}

#[test]
fn a_new_draft_is_never_overwritten() {
    let mut app = app_with_echo(false);
    app.composer = TextEdit::new("something newer");
    app.send_failed(ID, "POST /api/session/ses_a/prompt: io: Connection refused (os error 61)");
    assert_eq!(app.composer.text, "something newer");
    assert_eq!(app.toast.current.unwrap().description.as_deref(), Some("OpenCode isn't running"));
}

#[test]
fn a_timed_out_send_waits_as_sending_and_says_nothing() {
    let mut app = app_with_echo(false);
    app.send_failed(ID, "POST /api/session/ses_a/prompt: timeout: global");
    assert!(has_echo(&app) && app.unsent.sending(ID));
    assert!(app.toast.current.is_none());
}

#[test]
fn it_was_sent_after_all_when_the_session_has_it() {
    let mut app = app_with_echo(false);
    app.send_failed(ID, "POST /api/session/ses_a/prompt: timeout: global");
    serve(&mut app, json!([{"id": ID, "type": "user", "text": "hey", "time": {"created": 1}}]));
    app.check_sends();
    assert!(settle(&mut app, |a| !a.unsent.sending(ID) && !a.unsent.unsure()));
    assert!(has_echo(&app));
    assert!(app.toast.current.is_none());
    assert_eq!(app.composer.text, "");
}

#[test]
fn it_rolls_back_when_the_session_never_got_it() {
    let mut app = app_with_echo(false);
    app.send_failed(ID, "POST /api/session/ses_a/prompt: timeout: global");
    serve(&mut app, json!([{"id": "msg_other", "type": "user", "text": "earlier", "time": {"created": 1}}]));
    app.check_sends();
    assert!(settle(&mut app, |a| a.toast.current.is_some()));
    assert!(!has_echo(&app) && !app.unsent.unsure());
    assert_eq!(app.composer.text, "hey");
    let toast = app.toast.current.unwrap();
    assert_eq!((toast.title.as_str(), toast.description.as_deref()), ("Failed to send prompt", Some("OpenCode didn't answer in time")));
}

#[test]
fn a_waiting_prompt_is_kept_when_the_inbox_lists_it_and_dropped_when_not() {
    let listed = |ids: &[&str]| json!({"sessionID": SES, "items": ids.iter().map(|i| json!({"id": i, "type": "user", "delivery": "steer", "payload": {"text": "hey"}})).collect::<Vec<_>>()});
    let mut kept = app_with_echo(true);
    kept.send_failed(ID, "POST /x: timeout: global");
    kept.unsent.sent[0].asked = Some(0.0);
    kept.apply_inbox("session.inbox.listed", &listed(&[ID]), SES);
    assert!(!kept.unsent.unsure() && kept.toast.current.is_none());

    let mut lost = app_with_echo(true);
    lost.send_failed(ID, "POST /x: timeout: global");
    lost.unsent.sent[0].asked = Some(0.0);
    serve(&mut lost, json!([]));
    lost.judge_sends_by_page(SES, &[]);
    assert!(lost.unsent.unsure(), "the inbox has the last word on a waiting prompt");
    lost.apply_inbox("session.inbox.listed", &listed(&[]), SES);
    assert!(!lost.unsent.unsure());
    assert_eq!(lost.composer.text, "hey");
    assert!(lost.toast.current.is_some());
}

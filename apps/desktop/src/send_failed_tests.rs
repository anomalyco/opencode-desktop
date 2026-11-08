use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use opencode_kit::model::{Delivery, InboxItem, Message, Transcript};
use rune::TextEdit;
use serde_json::json;
use super::*;
use crate::{App, Route};

const SES: &str = "ses_a";
const ID: &str = "msg_mine";

#[test]
fn outbox_holds_a_send_before_the_writer_connects() {
    let mut app = App::fixture_session();
    app.fake_writes();
    if let Source::Live { writes, .. } = &mut app.source { *writes = None; }
    let Route::Session(session) = app.route.clone() else { unreachable!() };
    app.composer = TextEdit::new("send when ready");
    app.send(&mut rune::Tx::default());
    assert!(app.composer.text.is_empty());
    assert!(app.transcripts[&session].messages.iter().any(|m| matches!(m, Message::User {text, ..} if text == "send when ready")));
}

fn app_with_echo(inbox: bool) -> App {
    let mut app = App::empty();
    app.fake_writes();
    app.unsent.fetch = Some(Arc::new(|_| Err("offline".into())));
    app.route = Route::Session(SES.into());
    app.transcripts.insert(SES.into(), Transcript { session: SES.into(), messages: vec![] });
    let prompt = Prompt { id: Some(ID.into()), text: "hey".into(), delivery: inbox.then_some(Delivery::Steer), ..Default::default() };
    if inbox {
        app.inboxes.entry(SES.into()).or_default().upsert(InboxItem { id: ID.into(), text: "hey".into(), delivery: Delivery::Steer, files: vec![] });
    } else {
        app.transcripts.get_mut(SES).unwrap().messages.push(message());
        app.attach.echoes.push((SES.into(), ID.into(), "hey".into()));
    }
    app.unsent.note(Sent::new(SES, ID, Entry::of(&TextEdit::new("hey")), vec![], vec![], inbox).with_prompt(&prompt));
    app
}

fn message() -> Message { Message::User { id: ID.into(), text: "hey".into() } }
fn has_echo(app: &App) -> bool { app.transcripts[SES].messages.iter().any(|m| m.id() == ID) }
fn timeout(app: &mut App) { assert!(app.send_failed(ID, "POST /prompt: timeout: global")); }

fn settle(app: &mut App) {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if app.poll_sends() { return; }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("receipt did not finish");
}

#[test]
fn refusal_keeps_a_failed_bubble_with_recoverable_text() {
    let mut app = app_with_echo(false);
    app.send_failed(ID, "POST /prompt: http status: 422");
    assert!(has_echo(&app));
    assert_eq!(app.unsent.state(ID), Some(State::Failed));
    assert!(app.attach.echoes.is_empty());
    app.edit_failed_send(ID);
    assert_eq!(app.composer.text, "hey");
}

#[test]
fn another_draft_or_session_is_never_overwritten() {
    let mut app = app_with_echo(false);
    app.composer = TextEdit::new("something newer");
    app.send_failed(ID, "POST /prompt: http status: 422");
    app.edit_failed_send(ID);
    assert_eq!(app.composer.text, "something newer");
    app.composer.clear();
    app.route = Route::Session("ses_other".into());
    app.edit_failed_send(ID);
    assert!(app.composer.text.is_empty());
}

#[test]
fn an_offline_timeout_is_unconfirmed_never_failed_or_retried() {
    let mut app = app_with_echo(false);
    let writes = app.fake_writes();
    timeout(&mut app);
    settle(&mut app);
    assert!(has_echo(&app) && app.unsent.sending(ID));
    assert!(app.toast.current.is_none());
    app.retry_send(ID);
    assert!(writes.try_recv().is_err());
}

#[test]
fn timeout_reconciliation_finishes_when_inbox_answers_before_page() {
    let mut app = app_with_echo(true);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    app.unsent.fetch = Some(Arc::new(move |path| {
        seen.lock().unwrap().push(path.to_owned());
        if path.ends_with("/inbox") { Ok(json!({"data": []})) }
        else { Err(format!("GET {path}: http status: 404")) }
    }));
    timeout(&mut app);
    app.apply_inbox("session.inbox.listed", &json!({"sessionID": SES, "items": []}), SES);
    // Missing from an inbox read mustn't erase the local prompt while its receipt is unknown.
    assert!(app.inboxes[SES].items.iter().any(|i| i.id == ID));
    app.judge_sends_by_page(SES, &[]);
    settle(&mut app);
    assert_eq!(app.unsent.state(ID), Some(State::Failed));
    assert!(has_echo(&app));
    assert_eq!(*calls.lock().unwrap(), vec![format!("/api/session/{SES}/inbox"), format!("/api/session/{SES}/message/{ID}")]);
}

#[test]
fn landed_but_unconfirmed_is_found_even_outside_the_latest_page() {
    let mut app = app_with_echo(true);
    app.unsent.fetch = Some(Arc::new(|path| {
        Ok(if path.ends_with("/inbox") { json!({"data": []}) }
        else { json!({"data": {"id": ID, "type": "user", "text": "hey", "time": {"created": 1}}}) })
    }));
    timeout(&mut app);
    app.judge_sends_by_page(SES, &[]);
    settle(&mut app);
    assert_eq!(app.unsent.state(ID), Some(State::Accepted));
    assert!(has_echo(&app));
    assert!(app.inboxes[SES].items.is_empty());
    assert!(app.toast.current.is_none());
}

#[test]
fn admission_events_win_over_a_timeout_response_or_an_older_absent_receipt() {
    for before_timeout in [true, false] {
        let mut app = app_with_echo(true);
        app.unsent.fetch = Some(Arc::new(|path| if path.ends_with("/inbox") { Ok(json!({"data": []})) } else { Err("http status: 404".into()) }));
        if !before_timeout { timeout(&mut app); }
        app.apply_event("session.inbox.enqueued", &json!({"sessionID": SES, "inboxID": ID,
            "item": {"type": "user", "delivery": "steer", "payload": {"text": "hey"}}}));
        if before_timeout { timeout(&mut app); } else { settle(&mut app); }
        assert_eq!(app.unsent.state(ID), Some(State::Accepted));
        assert!(app.toast.current.is_none());
    }
}

#[test]
fn a_server_page_clears_timeout_without_an_earlier_reconnect() {
    let mut app = app_with_echo(false);
    timeout(&mut app);
    app.judge_sends_by_page(SES, &[message()]);
    assert_eq!(app.unsent.state(ID), Some(State::Accepted));
}

#[test]
fn an_identical_inbox_listing_still_redraws_a_changed_receipt() {
    let mut app = app_with_echo(true);
    timeout(&mut app);
    assert!(app.apply_inbox("session.inbox.listed", &json!({"sessionID": SES, "items": [
        {"id": ID, "type": "user", "delivery": "steer", "payload": {"text": "hey"}}
    ]}), SES));
    assert_eq!(app.unsent.state(ID), Some(State::Accepted));
}

#[test]
fn recovery_retries_reads_but_not_the_prompt_write() {
    let mut app = app_with_echo(false);
    let writes = app.fake_writes();
    timeout(&mut app);
    settle(&mut app);
    app.unsent.fetch = Some(Arc::new(|_| Ok(json!({"data": [{"id": ID}]}))));
    app.check_sends();
    settle(&mut app);
    assert_eq!(app.unsent.state(ID), Some(State::Accepted));
    assert!(writes.try_recv().is_err());
}

#[test]
fn only_two_successful_authoritative_reads_can_prove_absence() {
    for error in ["timeout: global", "http status: 500", "http status: 503"] {
        let result = receipt::read(&|path| if path.ends_with("/inbox") { Ok(json!({"data": []})) } else { Err(error.into()) }, SES, ID);
        assert!(result.is_err());
    }
    assert!(receipt::read(&|_| Err("http status: 404".into()), SES, ID).is_err(), "inbox 404 is not proof of a missing prompt");
    assert!(receipt::read(&|_| Ok(json!({"oops": []})), SES, ID).is_err());
}

#[test]
fn explicit_retry_preserves_request_id_files_quotes_and_delivery() {
    let mut app = app_with_echo(true);
    let writes = app.fake_writes();
    let sent = &mut app.unsent.sent[0];
    let prompt = sent.prompt.as_mut().unwrap();
    prompt.attachments.push(PathAttachment { path: "/project/readme.md".into(), ..Default::default() });
    prompt.files.push(opencode_kit::client::FileInput { uri: "media:abc".into(), ..Default::default() });
    prompt.quotes.push(opencode_kit::quote::Quote { text: "Keep this quoted passage".into(), comment: Some("Explain this".into()) });
    prompt.delivery = Some(Delivery::Queue);
    let expected = prompt.clone();
    app.send_failed(ID, "POST /prompt: http status: 422");
    app.retry_send(ID);
    let opencode_kit::client::Write::Send { session, prompt } = writes.try_recv().unwrap() else { panic!("expected prompt write") };
    assert_eq!(session, SES);
    assert_eq!(prompt, expected);
    assert_eq!(app.unsent.state(ID), Some(State::Sending));
    assert!(!has_echo(&app));
    assert_eq!(app.inboxes[SES].items[0].delivery, Delivery::Queue);
    app.retry_send(ID);
    assert!(writes.try_recv().is_err(), "double clicking retry cannot post twice");
}

#[test]
fn labels_stay_below_each_bubble_and_clear_after_memoized_confirmation() {
    let mut app = App::fixture_session();
    stories::setup(&mut app);
    let mut rt = app.into_runtime();
    rt.frame(0.0);
    for id in [stories::FIRST, stories::SECOND] {
        let els = rt.elements();
        let within = format!("u-{id}/");
        let bubble = els.iter().find(|e| e.path.contains(&within) && e.key.as_deref() == Some("bubble")).unwrap();
        let status = els.iter().find(|e| e.path.contains(&within) && e.key.as_deref() == Some("send-status")).unwrap();
        assert!(status.target[1] >= bubble.target[1] + bubble.target[3], "status belongs below {id}");
    }
    rt.update(|s, tx| { tx.instant(); stories::confirmed(s); });
    rt.frame(0.0);
    let els = rt.elements();
    assert!(!els.iter().any(|e| !e.exiting && e.text.as_deref() == Some("Not confirmed")), "memoized labels clear");
    assert!(els.iter().any(|e| !e.exiting && e.text.as_deref() == Some("Failed to send")));
    assert!(els.iter().any(|e| !e.exiting && e.key.as_deref() == Some("retry-send")));
    assert!(els.iter().any(|e| !e.exiting && e.key.as_deref() == Some(&format!("text-{}", stories::SECOND))), "later sent prompt stays");
}

/// Kit's Scrawl: a long steer folded to Read more and a steer queued under it. Its status sits
/// on its own line under its bubble, right-aligned with it, never beside it.
#[test]
fn status_under_a_collapsed_steer_stays_under_its_bubble() {
    let mut app = App::fixture_session();
    stories::queued_long(&mut app);
    let mut rt = app.into_runtime();
    rt.frame(0.0);
    rt.frame(0.1);
    let els = rt.elements();
    let within = |id: &str| format!("u-{id}/");
    let find = |id: &str, key: &str| els.iter().find(|e| !e.exiting && e.path.contains(&within(id)) && e.key.as_deref() == Some(key));
    assert!(find(stories::FIRST, "fold-toggle").is_some(), "the long steer is folded");
    let (bubble, status) = (find(stories::FIRST, "bubble").unwrap(), find(stories::FIRST, "send-status").unwrap());
    assert!(status.target[1] >= bubble.target[1] + bubble.target[3], "status is under its bubble");
    assert!(status.target[0] >= bubble.target[0], "status is not left of its bubble");
    let next = find(stories::SECOND, "bubble").unwrap();
    assert!(next.target[1] >= status.target[1] + status.target[3], "the next steer starts under the status");
    assert!(find(stories::SECOND, "send-status").is_none());
}

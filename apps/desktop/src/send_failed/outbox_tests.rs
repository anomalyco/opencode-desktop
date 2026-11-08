use super::*;
use opencode_kit::client::{Server, Write};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write as _},
    net::TcpListener,
    sync::mpsc::Receiver,
};

fn fresh(path: &std::path::Path) -> (App, Receiver<Write>) {
    let mut app = App::fixture_session();
    let writer = app.fake_writes();
    app.unsent.path = Some(path.into());
    app.read_outbox();
    (app, writer)
}
fn path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("outbox-{}.json", opencode_kit::client::message_id()))
}
fn send(app: &mut App, body: &str) -> String {
    app.composer = rune::TextEdit::new(body);
    app.send(&mut rune::Tx::default());
    assert!(app.composer.text.is_empty());
    app.unsent.sent.last().unwrap().id.clone()
}
fn post(writes: &Receiver<Write>) -> (String, Prompt) {
    match writes.try_recv().unwrap() {
        Write::Send { session, prompt } | Write::SendMovingHome { session, prompt } => (session, prompt),
        _ => panic!("expected prompt"),
    }
}

#[test]
fn restart_with_two_offline_messages_preserves_order_and_identity() {
    let path = path();
    let (mut app, _) = fresh(&path);
    if let Source::Live { writes, .. } = &mut app.source {
        *writes = None;
    }
    let a = send(&mut app, "first");
    let b = send(&mut app, "second");
    drop(app);
    let (mut app, writes) = fresh(&path);
    assert_eq!(app.unsent.state(&a), Some(State::Waiting));
    app.pump_outbox();
    app.pump_outbox();
    assert_eq!(post(&writes).1.id, Some(a.clone()));
    assert!(writes.try_recv().is_err());
    app.accept_send(&a);
    app.pump_outbox();
    assert_eq!(post(&writes).1.id, Some(b.clone()));
    app.accept_send(&b);
    app.pump_outbox();
    assert!(writes.try_recv().is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "[]");
    std::fs::remove_file(path).unwrap();
}

/// Lose the TCP reply AFTER admission. A fresh app must check the server's inbox,
/// not just find no message in its local transcript and blindly POST again.
#[test]
fn restart_after_server_received_post_but_dropped_reply_never_posts_twice() {
    let path = path();
    let (mut app, writes) = fresh(&path);
    let id = send(&mut app, "once, even across a restart");
    let (session, mut prompt) = post(&writes);
    prompt.model = None;
    prompt.agent = None;
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = Server::new(&format!("http://{}", socket.local_addr().unwrap()));
    let expect = id.clone();
    let mock = std::thread::spawn(move || {
        for step in 0..2 {
            let (mut stream, _) = socket.accept().unwrap();
            let mut input = BufReader::new(&mut stream);
            let mut line = String::new();
            input.read_line(&mut line).unwrap();
            let mut len = 0;
            loop {
                let mut header = String::new();
                input.read_line(&mut header).unwrap();
                if header == "\r\n" || header.is_empty() {
                    break;
                }
                if let Some(n) = header.to_lowercase().strip_prefix("content-length:") {
                    len = n.trim().parse().unwrap();
                }
            }
            if step == 0 {
                assert!(line.starts_with("POST "));
                let mut body = vec![0; len];
                input.read_exact(&mut body).unwrap();
                assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["id"], expect);
                continue;
            }
            assert!(line.starts_with("GET ") && line.contains("/inbox "));
            let body = json!({"data":[{"id":expect}]}).to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        }
    });
    assert!(server.send(&session, &prompt).is_err());
    drop(app);
    let (mut app, writes) = fresh(&path);
    assert_eq!(app.unsent.state(&id), Some(State::Unconfirmed));
    app.receipts_from(&|p| server.read(p), vec![(session, id.clone())]);
    app.pump_outbox();
    assert_eq!(app.unsent.state(&id), Some(State::Accepted));
    assert!(writes.try_recv().is_err());
    mock.join().unwrap();
    std::fs::remove_file(path).unwrap();
}

#[test]
fn retryable_errors_wait_for_receipt_then_reuse_original_request() {
    for failure in
        ["connection refused", "timeout: global", "connection reset", "http status: 408", "http status: 429", "http status: 500", "http status: 503"]
    {
        let path = path();
        let (mut app, writes) = fresh(&path);
        let id = send(&mut app, "don't lose this");
        let (session, original) = post(&writes);
        app.send_failed(&id, failure);
        assert_eq!(app.unsent.state(&id), Some(if failure == "connection refused" { State::Waiting } else { State::Unconfirmed }));
        assert!(writes.try_recv().is_err());
        app.receipts_from(
            &|p| {
                if p.ends_with("/inbox") { Ok(json!({"data":[]})) } else { Err("http status: 404".into()) }
            },
            vec![(session, id.clone())],
        );
        app.pump_outbox();
        app.pump_outbox();
        assert_eq!(post(&writes).1, original);
        assert!(writes.try_recv().is_err());
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn permanent_rejection_restores_words_but_never_overwrites_new_typing() {
    for newer in ["", "another draft"] {
        let path = path();
        let (mut app, writes) = fresh(&path);
        let id = send(&mut app, "original");
        let _ = post(&writes);
        app.composer = rune::TextEdit::new(newer);
        app.send_failed(&id, "http status: 422");
        assert_eq!(app.composer.text, if newer.is_empty() { "original" } else { newer });
        app.pump_outbox();
        assert!(writes.try_recv().is_err());
        assert_eq!(app.unsent.state(&id), Some(State::Failed));
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn busy_offline_prompt_keeps_delivery_and_can_be_edited_or_deleted() {
    let path = path();
    let (mut app, _) = fresh(&path);
    let crate::Route::Session(session) = app.route.clone() else { unreachable!() };
    app.running.insert(session.clone());
    if let Source::Live { writes, .. } = &mut app.source {
        *writes = None;
    }
    let id = send(&mut app, "later");
    assert!(app.unsent.durable(&id).unwrap().prompt.as_ref().unwrap().delivery.is_some());
    assert!(app.transcripts[&session].messages.iter().any(|m| m.id() == id));
    app.composer = rune::TextEdit::new("new typing");
    app.outbox_action(&id, "outbox-edit");
    assert_eq!(app.composer.text, "new typing\n\nlater");
    let id = send(&mut app, "delete this");
    app.outbox_action(&id, "outbox-delete");
    assert_eq!(app.unsent.state(&id), None);
    let (reopened, _) = fresh(&path);
    assert!(reopened.unsent.sent.is_empty());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn journal_write_failure_keeps_the_composer_and_posts_nothing() {
    let path = path();
    std::fs::write(&path, "not a directory").unwrap();
    let (mut app, writes) = fresh(&path.join("unwritable"));
    app.composer = rune::TextEdit::new("keep me");
    app.send(&mut rune::Tx::default());
    assert_eq!(app.composer.text, "keep me");
    assert!(writes.try_recv().is_err());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn editing_a_lost_reply_waits_for_a_receipt_instead_of_minting_a_duplicate() {
    for admitted in [true, false] {
        let path = path();
        let (mut app, writes) = fresh(&path);
        let id = send(&mut app, "original");
        let (session, _) = post(&writes);
        app.unsent.asked = Some(vec![]);
        app.send_failed(&id, "timeout: global");
        app.outbox_action(&id, "outbox-edit");
        assert!(app.composer.text.is_empty());
        assert!(writes.try_recv().is_err());
        app.receipts_from(
            &|p| {
                if p.ends_with("/inbox") {
                    Ok(if admitted { json!({"data":[{"id":id}]}) } else { json!({"data":[]}) })
                } else {
                    Err("http status: 404".into())
                }
            },
            vec![(session, id.clone())],
        );
        assert_eq!(app.composer.text, if admitted { "" } else { "original" });
        app.pump_outbox();
        assert!(writes.try_recv().is_err());
        std::fs::remove_file(path).unwrap();
    }
}

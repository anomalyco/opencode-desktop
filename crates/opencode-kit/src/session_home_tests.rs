use std::io::{Read, Write};
use std::sync::mpsc;

use serde_json::{Value, json};

use super::GONE;
use crate::client::{Prompt, Server};

/// Answers each request in turn with `replies` and hands back what came in.
fn mock(replies: Vec<(u16, Value)>) -> (Server, mpsc::Receiver<String>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for (status, body) in replies {
            let (mut c, _) = listener.accept().unwrap();
            c.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                let n = c.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                let t = String::from_utf8_lossy(&buf);
                if let Some(h) = t.find("\r\n\r\n") {
                    let len = t[..h]
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0)))
                        .unwrap_or(0usize);
                    if buf.len() >= h + 4 + len {
                        break;
                    }
                }
            }
            let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
            let body = body.to_string();
            let _ = write!(c, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len());
        }
    });
    (Server::new(&base), rx)
}

fn prompt() -> Prompt {
    Prompt { id: Some("msg_1".into()), text: "hello".into(), ..Default::default() }
}

fn first_line(req: &str) -> &str {
    req.lines().next().unwrap_or_default()
}

#[test]
fn a_session_whose_folder_is_gone_moves_home_and_hears_the_prompt() {
    let missing = json!({ "_tag": "LocationNotFoundError", "message": "Location not found: /w/fix" });
    let session = json!({ "data": { "id": "ses_b", "projectID": "p1", "location": { "directory": "/w/fix" } } });
    let projects = json!({ "data": [{ "id": "p0", "canonical": "/other" }, { "id": "p1", "canonical": "/code/app" }] });
    let (server, rx) = mock(vec![(404, missing), (200, session), (200, projects), (204, json!(null)), (200, json!({ "data": { "id": "msg_1" } }))]);
    assert_eq!(server.send_moving_home("ses_b", &prompt()).unwrap().as_deref(), Some("msg_1"));
    let reqs: Vec<String> = rx.iter().take(5).collect();
    let lines: Vec<&str> = reqs.iter().map(|r| first_line(r)).collect();
    assert_eq!(
        lines,
        [
            "POST /api/session/ses_b/prompt HTTP/1.1",
            "GET /api/session/ses_b HTTP/1.1",
            "GET /api/project HTTP/1.1",
            "POST /api/session/ses_b/move HTTP/1.1",
            "POST /api/session/ses_b/prompt HTTP/1.1",
        ]
    );
    assert!(reqs[3].ends_with(r#"{"directory":"/code/app"}"#));
    assert!(reqs[4].contains("hello") && reqs[4].contains("moved to /code/app") && reqs[4].contains(r#""id":"msg_1""#), "{}", reqs[4]);
}

#[test]
fn a_prompt_that_lands_is_sent_once() {
    let (server, rx) = mock(vec![(200, json!({ "data": { "id": "msg_1" } }))]);
    server.send_moving_home("ses_b", &prompt()).unwrap();
    assert_eq!(rx.iter().count(), 1);
}

#[test]
fn only_get_saying_404_means_gone() {
    let (server, _rx) = mock(vec![(404, json!({})), (404, json!({}))]);
    let e = server.send_moving_home("ses_dead", &prompt()).unwrap_err();
    assert!(e.starts_with("POST /api/session/ses_dead/prompt: ") && e.contains(GONE), "{e}");

    let (server, _rx) = mock(vec![(404, json!({})), (500, json!({}))]);
    let e = server.send_moving_home("ses_b", &prompt()).unwrap_err();
    assert!(!e.contains(GONE) && e.contains("404"), "a GET that fails otherwise isn't proof: {e}");

    let (server, _rx) = mock(vec![(500, json!({}))]);
    let e = server.send_moving_home("ses_b", &prompt()).unwrap_err();
    assert!(e.contains("500"), "{e}");
}

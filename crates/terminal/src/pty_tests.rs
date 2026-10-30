use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use opencode_kit::client::Server;
use tungstenite::Message;
use tungstenite::protocol::CloseFrame;
use tungstenite::protocol::frame::coding::CloseCode;

use crate::{
    GhosttyKey, KeyInput, RestoreState, ScrollOp, SelectionOp, SessionStatus, TerminalConfig,
    TerminalSession,
};

fn wait_until(timeout: Duration, mut pred: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if pred() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(pred(), "condition not met within {timeout:?}");
}

#[test]
fn off_thread_stream_coalesces_bursts_and_roundtrips_persistence() {
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let (session, handle) = TerminalSession::spawn_stream(
        TerminalConfig {
            cols: 80,
            rows: 24,
            ..TerminalConfig::default()
        },
        None,
        Arc::new(move || {
            w.fetch_add(1, Ordering::Relaxed);
        }),
    )
    .unwrap();

    // Cursor-only sequence keeps terminal unrevealed until real printable text arrives.
    handle.feed(b"\x1b[?25h");
    std::thread::sleep(Duration::from_millis(15));
    assert!(!session.revealed());

    // Feed 5,000 colored lines across many chunks: worker coalesces wakes instead of waking 5,000x.
    for i in 0..5_000 {
        handle.feed(format!("\x1b[32mOK\x1b[0m line-{i:04}\r\n"));
    }
    wait_until(Duration::from_secs(3), || {
        session.snapshot().viewport_text().contains("line-4999")
    });
    assert!(session.revealed());
    assert!(
        wakes.load(Ordering::Relaxed) < 500,
        "burst coalescing must keep UI wakes far below 5,000 chunks"
    );

    // Input encoding & selection run off the main thread.
    let mut key = KeyInput::press(GhosttyKey::Backspace);
    key.cmd = true;
    session.send_key(key);
    wait_until(Duration::from_secs(1), || handle.take_written() == b"\x15");

    session.select(SelectionOp::All);
    wait_until(Duration::from_secs(1), || {
        session
            .selected_text()
            .is_some_and(|s| s.contains("line-4999"))
    });

    // Scroll up and capture persisted state, then restore into a new session.
    session.scroll(ScrollOp::Delta(-10));
    wait_until(Duration::from_secs(1), || !session.snapshot().scrollbar.at_bottom);
    let saved = session
        .persist_snapshot(Duration::from_secs(2))
        .expect("persisted snapshot");
    assert!(!saved.buffer.is_empty());

    let (restored, _) = TerminalSession::spawn_stream(
        TerminalConfig {
            cols: saved.cols,
            rows: saved.rows,
            ..TerminalConfig::default()
        },
        Some(RestoreState {
            buffer: saved.buffer,
            cols: Some(saved.cols),
            rows: Some(saved.rows),
            scroll_y: Some(saved.scroll_y as usize),
            cursor: Some(saved.cursor),
        }),
        Arc::new(|| {}),
    )
    .unwrap();
    assert!(restored.revealed());
    assert!(restored.snapshot().viewport_text().contains("OK line-"));
}

#[test]
fn server_pty_speaks_connect_token_meta_cursor_and_reconnects_cleanly() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let received_input = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let seen_cursors = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let put_sizes = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));

    let srv_in = received_input.clone();
    let srv_cur = seen_cursors.clone();
    let srv_put = put_sizes.clone();

    let stop_server = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_flag = stop_server.clone();
    let server_thread = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let mut ws_connects = 0usize;
        let mut ws_threads = Vec::new();
        while !stop_flag.load(Ordering::Acquire) {
            let (mut stream, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(_) => break,
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();

            let mut peek_buf = [0u8; 2048];
            let n = stream.peek(&mut peek_buf).unwrap();
            let head = String::from_utf8_lossy(&peek_buf[..n]).into_owned();
            let first_line = head.lines().next().unwrap_or("").to_owned();

            if first_line.to_ascii_lowercase().contains("connect?")
                && head.to_ascii_lowercase().contains("upgrade: websocket")
            {
                ws_connects += 1;
                let conn_no = ws_connects;
                srv_cur.lock().push(first_line.clone());
                let srv_in = srv_in.clone();
                ws_threads.push(std::thread::spawn(move || {
                    let mut ws = tungstenite::accept(stream).unwrap();
                    if conn_no == 1 {
                        ws.send(Message::Text("boot-output\r\n".into())).unwrap();
                        let mut meta = vec![0u8];
                        meta.extend_from_slice(br#"{"cursor":42}"#);
                        ws.send(Message::Binary(meta.into())).unwrap();
                        let _ = ws.send(Message::Close(Some(CloseFrame {
                            code: CloseCode::Error,
                            reason: "transient".into(),
                        })));
                        let _ = ws.flush();
                        std::thread::sleep(Duration::from_millis(20));
                    } else {
                        ws.send(Message::Text("live$ ".into())).unwrap();
                        while let Ok(msg) = ws.read() {
                            if let Message::Text(t) = msg {
                                srv_in.lock().push(t.to_string());
                                break;
                            }
                        }
                        let _ = ws.send(Message::Close(Some(CloseFrame {
                            code: CloseCode::Normal,
                            reason: "".into(),
                        })));
                        let _ = ws.flush();
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }));
                continue;
            }

            // Regular HTTP request (`connect-token`, `PUT` size, `GET` pty status).
            let mut req_buf = Vec::new();
            let mut chunk = [0u8; 2048];
            loop {
                let r = stream.read(&mut chunk).unwrap_or(0);
                if r == 0 {
                    break;
                }
                req_buf.extend_from_slice(&chunk[..r]);
                let txt = String::from_utf8_lossy(&req_buf);
                if let Some(idx) = txt.find("\r\n\r\n") {
                    let clen = txt[..idx]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap_or(0usize))
                        })
                        .unwrap_or(0);
                    if req_buf.len() >= idx + 4 + clen {
                        break;
                    }
                }
            }
            let req_str = String::from_utf8_lossy(&req_buf);
            let body = if first_line.starts_with("POST /api/pty/pty_mock/connect-token") {
                r#"{"data":{"ticket":"tok_1"}}"#
            } else if first_line.starts_with("PUT /api/pty/pty_mock") {
                srv_put.lock().push(req_str.into_owned());
                r#"{"data":{"id":"pty_mock","title":"Terminal 1","command":"zsh","args":[],"cwd":"/tmp","status":"running","pid":99}}"#
            } else {
                r#"{"data":{"id":"pty_mock","title":"Terminal 1","command":"zsh","args":[],"cwd":"/tmp","status":"running","pid":99}}"#
            };
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
        }
        for h in ws_threads {
            let _ = h.join();
        }
    });

    let server = Server::new(&base).with_password("mock-pw");
    let session = TerminalSession::spawn_server_pty(
        server,
        "/tmp".into(),
        "pty_mock".into(),
        TerminalConfig {
            cols: 90,
            rows: 28,
            ..TerminalConfig::default()
        },
        None,
        Arc::new(|| {}),
    )
    .unwrap();

    // Wait for reconnect + "live$ " prompt after transient 1011 drop.
    wait_until(Duration::from_secs(5), || {
        session.snapshot().viewport_text().contains("live$")
    });
    assert!(session.snapshot().viewport_text().contains("boot-output"));

    session.paste("pwd\n");
    wait_until(Duration::from_secs(3), || {
        // Unbracketed paste replaces `\n` with `\r` (`ghostty_paste_encode`).
        received_input.lock().iter().any(|s| s == "pwd\r")
    });
    wait_until(Duration::from_secs(3), || {
        matches!(session.status(), SessionStatus::Exited { .. })
    });

    stop_server.store(true, Ordering::Release);
    server_thread.join().unwrap();

    let cursors = seen_cursors.lock().clone();
    assert_eq!(cursors.len(), 2);
    assert!(cursors[0].contains("cursor=0"), "first connect starts at cursor=0: {}", cursors[0]);
    assert!(
        cursors[1].contains("cursor=42"),
        "reconnect resumes from binary control frame cursor=42: {}",
        cursors[1]
    );
    assert!(
        put_sizes
            .lock()
            .iter()
            .any(|r| r.contains(r#""cols":90"#) && r.contains(r#""rows":28"#)),
        "initial viewport size synced via PUT /api/pty/:id"
    );
}

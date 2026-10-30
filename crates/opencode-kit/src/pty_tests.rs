use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use super::*;

fn mock_json(replies: Vec<(&'static str, String)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        replies
            .into_iter()
            .map(|(status, body)| {
                let (mut s, _) = listener.accept().expect("accept");
                s.set_read_timeout(Some(Duration::from_secs(5))).ok();
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = s.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf);
                    if let Some(h) = text.find("\r\n\r\n") {
                        let len = text[..h]
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse().unwrap_or(0))
                            })
                            .unwrap_or(0usize);
                        if buf.len() >= h + 4 + len {
                            break;
                        }
                    }
                }
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(body.as_bytes());
                String::from_utf8_lossy(&buf).into_owned()
            })
            .collect()
    });
    (base, handle)
}

#[test]
fn pty_crud_and_connect_url_match_server_protocol() {
    let info_json = json!({
        "location": { "directory": "/workspace/demo-repo" },
        "data": {
            "id": "pty_01abc",
            "title": "Terminal 1",
            "command": "/bin/zsh",
            "args": ["-l"],
            "cwd": "/workspace/demo-repo",
            "status": "running",
            "pid": 4242
        }
    })
    .to_string();
    let updated_json = json!({
        "data": {
            "id": "pty_01abc",
            "title": "Dev server",
            "command": "/bin/zsh",
            "args": [],
            "cwd": "/workspace/demo-repo",
            "status": "exited",
            "pid": 4242,
            "exitCode": 0
        }
    })
    .to_string();
    let token_json = json!({
        "data": { "ticket": "tkt_secret_1" }
    })
    .to_string();

    let (base, requests) = mock_json(vec![
        ("200 OK", info_json),
        ("200 OK", updated_json),
        ("200 OK", token_json),
        ("204 No Content", String::new()),
    ]);
    let server = Server::new(&base).with_password("test-pw");
    let dir = "/workspace/demo-repo";

    let created = server
        .pty_create(
            dir,
            &PtyCreate {
                title: Some("Terminal 1".into()),
                ..PtyCreate::default()
            },
        )
        .expect("pty_create");
    assert_eq!(created.id, "pty_01abc");
    assert_eq!(created.title, "Terminal 1");
    assert_eq!(created.status, PtyStatus::Running);
    assert_eq!(created.pid, 4242);

    let updated = server
        .pty_update(dir, "pty_01abc", Some("Dev server"), Some((120, 36)))
        .expect("pty_update");
    assert_eq!(updated.title, "Dev server");
    assert_eq!(updated.status, PtyStatus::Exited);
    assert_eq!(updated.exit_code, Some(0));

    let ws_url = server
        .pty_connect_url(dir, "pty_01abc", Some(-1))
        .expect("pty_connect_url");
    let ws_prefix = base.replacen("http://", "ws://", 1);
    assert_eq!(
        ws_url,
        format!(
            "{ws_prefix}/api/pty/pty_01abc/connect?location%5Bdirectory%5D=%2Fworkspace%2Fdemo-repo&cursor=-1&ticket=tkt_secret_1"
        )
    );

    server.pty_remove(dir, "pty_01abc").expect("pty_remove");

    let seen = requests.join().unwrap();
    assert!(seen[0].starts_with("POST /api/pty?location%5Bdirectory%5D=%2Fworkspace%2Fdemo-repo "));
    assert!(seen[0].to_ascii_lowercase().contains("authorization: basic "));
    assert!(seen[0].contains(r#""title":"Terminal 1""#));

    assert!(seen[1].starts_with("PUT /api/pty/pty_01abc?location%5Bdirectory%5D=%2Fworkspace%2Fdemo-repo "));
    assert!(seen[1].contains(r#""cols":120"#) && seen[1].contains(r#""rows":36"#));

    assert!(seen[2].starts_with(
        "POST /api/pty/pty_01abc/connect-token?location%5Bdirectory%5D=%2Fworkspace%2Fdemo-repo "
    ));
    assert!(seen[2].to_ascii_lowercase().contains("x-opencode-ticket: 1"));
    assert!(seen[2].to_ascii_lowercase().contains("authorization: basic "));

    assert!(seen[3].starts_with("DELETE /api/pty/pty_01abc?location%5Bdirectory%5D=%2Fworkspace%2Fdemo-repo "));
}

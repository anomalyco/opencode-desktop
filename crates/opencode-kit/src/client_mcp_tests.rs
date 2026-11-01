use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::handshake::server::{Request, Response};
use tungstenite::{Message, WebSocket};

use super::*;
use crate::client::Server;

fn accept_ws(listener: &TcpListener) -> (WebSocket<TcpStream>, String, Option<String>) {
    let (stream, _) = listener.accept().expect("accept");
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut uri = String::new();
    let mut auth = None;
    let ws = tungstenite::accept_hdr(stream, |req: &Request, resp: Response| {
        uri = req.uri().to_string();
        auth = req
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        Ok(resp)
    })
    .expect("ws upgrade");
    (ws, uri, auth)
}

fn send_rpc(ws: &mut WebSocket<TcpStream>, v: Value) {
    ws.send(Message::Text(v.to_string().into())).expect("ws send");
}

fn recv_rpc(ws: &mut WebSocket<TcpStream>) -> Value {
    loop {
        match ws.read().expect("ws read") {
            Message::Text(t) => return serde_json::from_str(&t).expect("valid json"),
            Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("unexpected ws message: {other:?}"),
        }
    }
}

fn wait_until(timeout: Duration, mut pred: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if pred() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(pred(), "timed out waiting for condition");
}

#[test]
fn url_encodes_directory_name_and_preserves_scheme_and_query() {
    let http = Server::new("http://127.0.0.1:4096");
    assert_eq!(
        http.client_mcp_url("/Users/alex/my project", "rune"),
        "ws://127.0.0.1:4096/api/experimental/client-mcp?location%5Bdirectory%5D=%2FUsers%2Falex%2Fmy%20project&name=rune"
    );

    let https = Server::new("https://example.ts.net:4096?workspace=w1");
    assert_eq!(
        https.client_mcp_url("/repo", "rune"),
        "wss://example.ts.net:4096/api/experimental/client-mcp?workspace=w1&location%5Bdirectory%5D=%2Frepo&name=rune"
    );
}

#[test]
fn handshake_readiness_ping_and_tools_list() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = Server::new(&base).with_password("secret-pw");

    let woke = Arc::new(AtomicUsize::new(0));
    let woke_w = woke.clone();
    let waker: Waker = Arc::new(move || {
        woke_w.fetch_add(1, Ordering::SeqCst);
    });

    let opts = ClientMcpOptions::new(
        "rune",
        "/work/repo",
        vec![McpTool::new(
            "open_file",
            "Open a file",
            json!({ "properties": { "path": { "type": "string" } }, "required": ["path"] }),
        )],
    )
    .with_instructions("Load skill `rune-ui` before writing ```ui outlines.");

    let (mut mcp, _calls) = server.connect_client_mcp(opts, waker);

    let (mut ws, uri, auth) = accept_ws(&listener);
    assert_eq!(
        uri,
        "/api/experimental/client-mcp?location%5Bdirectory%5D=%2Fwork%2Frepo&name=rune"
    );
    assert_eq!(auth.as_deref(), Some("Basic b3BlbmNvZGU6c2VjcmV0LXB3"));

    wait_until(Duration::from_secs(2), || mcp.status() == ClientMcpStatus::Handshaking);
    assert!(!mcp.is_ready(), "must not be ready before tools/list");

    // 1. initialize echoes protocolVersion and includes instructions.
    send_rpc(
        &mut ws,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "opencode", "version": "1.0.0" }
            }
        }),
    );
    let init_reply = recv_rpc(&mut ws);
    assert_eq!(
        init_reply,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "rune", "version": "1.0.0" },
                "instructions": "Load skill `rune-ui` before writing ```ui outlines."
            }
        })
    );
    assert!(!mcp.is_ready(), "still not ready after initialize alone");

    // 2. notifications/initialized gets no response and still waits on tools/list for readiness.
    send_rpc(&mut ws, json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    send_rpc(&mut ws, json!({ "jsonrpc": "2.0", "id": "p1", "method": "ping" }));
    assert_eq!(recv_rpc(&mut ws), json!({ "jsonrpc": "2.0", "id": "p1", "result": {} }));
    assert!(!mcp.is_ready(), "still not ready before tools/list");

    // 3. tools/list marks the connection Ready and normalizes inputSchema.type = "object".
    send_rpc(&mut ws, json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    let list_reply = recv_rpc(&mut ws);
    assert_eq!(
        list_reply,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "tools": [{
                    "name": "open_file",
                    "description": "Open a file",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "path": { "type": "string" } },
                        "required": ["path"]
                    }
                }]
            }
        })
    );
    wait_until(Duration::from_secs(2), || mcp.is_ready());
    assert!(woke.load(Ordering::SeqCst) >= 2);

    // 4. Updating tools emits notifications/tools/list_changed.
    mcp.set_tools(vec![McpTool::new("open_diff", "Open diff", json!({}))]);
    assert_eq!(
        recv_rpc(&mut ws),
        json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" })
    );

    mcp.close();
    assert_eq!(mcp.status(), ClientMcpStatus::Closed);
}

#[test]
fn tools_call_routes_session_id_images_errors_and_concurrent_requests() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = Server { base, auth: None, media_refs: false };

    let opts = ClientMcpOptions::new(
        "rune",
        "/work/repo",
        vec![
            McpTool::new("screenshot", "Capture screenshot", json!({})),
            McpTool::new("open_file", "Open file", json!({})),
        ],
    );
    let (mut mcp, calls) = server.connect_client_mcp(opts, Arc::new(|| {}));
    let (mut ws, _, _) = accept_ws(&listener);

    // Unknown tool -> -32602 without bothering the app channel.
    send_rpc(
        &mut ws,
        json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "tools/call",
            "params": { "name": "nope", "arguments": {} }
        }),
    );
    assert_eq!(
        recv_rpc(&mut ws),
        json!({
            "jsonrpc": "2.0",
            "id": 10,
            "error": { "code": -32602, "message": "Unknown tool: nope" }
        })
    );

    // Unknown method -> -32601.
    send_rpc(&mut ws, json!({ "jsonrpc": "2.0", "id": 11, "method": "resources/list" }));
    assert_eq!(
        recv_rpc(&mut ws),
        json!({
            "jsonrpc": "2.0",
            "id": 11,
            "error": { "code": -32601, "message": "Method not found: resources/list" }
        })
    );

    // Two concurrent tool calls + an interleaved ping while both are in flight.
    send_rpc(
        &mut ws,
        json!({
            "jsonrpc": "2.0",
            "id": 12,
            "method": "tools/call",
            "params": {
                "name": "screenshot",
                "arguments": { "format": "png" },
                "_meta": { "ai.opencode/sessionID": "ses_open" }
            }
        }),
    );
    send_rpc(
        &mut ws,
        json!({
            "jsonrpc": "2.0",
            "id": 13,
            "method": "tools/call",
            "params": {
                "name": "open_file",
                "arguments": { "path": "src/main.rs" },
                "_meta": { "ai.opencode/sessionID": "ses_closed" }
            }
        }),
    );
    send_rpc(&mut ws, json!({ "jsonrpc": "2.0", "id": 14, "method": "ping" }));

    // Ping is answered immediately even before the app answers either tool call!
    assert_eq!(recv_rpc(&mut ws), json!({ "jsonrpc": "2.0", "id": 14, "result": {} }));

    let c1 = calls.recv_timeout(Duration::from_secs(2)).unwrap();
    let c2 = calls.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(c1.directory, "/work/repo");
    assert_eq!(c1.name, "screenshot");
    assert_eq!(c1.session_id.as_deref(), Some("ses_open"));
    assert_eq!(c1.arguments, json!({ "format": "png" }));

    assert_eq!(c2.name, "open_file");
    assert_eq!(c2.session_id.as_deref(), Some("ses_closed"));

    // Respond out of order to prove concurrent multiplexing.
    c2.respond(McpResult::error("This session isn't open in OpenCode (Rune)"));
    assert_eq!(
        recv_rpc(&mut ws),
        json!({
            "jsonrpc": "2.0",
            "id": 13,
            "result": {
                "content": [{ "type": "text", "text": "This session isn't open in OpenCode (Rune)" }],
                "isError": true
            }
        })
    );

    c1.respond(McpResult::with_image(
        json!({ "tabID": 1, "format": "png" }).to_string(),
        "iVBORw0KGgo=",
        "image/png",
    ));
    assert_eq!(
        recv_rpc(&mut ws),
        json!({
            "jsonrpc": "2.0",
            "id": 12,
            "result": {
                "content": [
                    { "type": "text", "text": "{\"format\":\"png\",\"tabID\":1}" },
                    { "type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png" }
                ]
            }
        })
    );

    mcp.close();
}

#[test]
fn reconnects_after_drop_and_resets_backoff_on_initialized() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = Server { base, auth: None, media_refs: false };

    let mut opts = ClientMcpOptions::new(
        "rune",
        "/work/repo",
        vec![McpTool::new("ping_tool", "Ping", json!({}))],
    );
    opts.reconnect_min = Duration::from_millis(40);
    opts.reconnect_max = Duration::from_millis(200);

    let (mut mcp, _calls) = server.connect_client_mcp(opts, Arc::new(|| {}));

    // Connection 1: initialize + tools/list -> Ready, then server drops the socket.
    {
        let (mut ws1, _, _) = accept_ws(&listener);
        send_rpc(&mut ws1, json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" }));
        let _ = recv_rpc(&mut ws1);
        send_rpc(&mut ws1, json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        send_rpc(&mut ws1, json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
        let _ = recv_rpc(&mut ws1);
        wait_until(Duration::from_secs(2), || mcp.is_ready());
        let _ = ws1.close(None);
    }

    // Connection 2: client reconnects automatically and becomes Ready again after tools/list.
    {
        let (mut ws2, _, _) = accept_ws(&listener);
        assert!(!mcp.is_ready(), "dropped readiness while reconnecting");
        send_rpc(&mut ws2, json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" }));
        let _ = recv_rpc(&mut ws2);
        send_rpc(&mut ws2, json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
        let _ = recv_rpc(&mut ws2);
        wait_until(Duration::from_secs(2), || mcp.is_ready());
        mcp.close();
    }
}

#[test]
fn server_without_route_marks_unsupported_quietly_and_retries_later() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = Server { base, auth: None, media_refs: false };

    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_srv = attempts.clone();

    // First request answers 404 Not Found (older server without /api/experimental/client-mcp);
    // second request (after unsupported_retry) upgrades cleanly to WebSocket.
    let srv_thread = std::thread::spawn(move || {
        // Attempt 1: 404 Not Found.
        let (mut s1, _) = listener.accept().unwrap();
        s1.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut buf = [0u8; 2048];
        let _ = s1.read(&mut buf);
        attempts_srv.fetch_add(1, Ordering::SeqCst);
        s1.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        drop(s1);

        // Attempt 2: server upgraded, now accepts WebSocket!
        let (mut ws, _, _) = accept_ws(&listener);
        attempts_srv.fetch_add(1, Ordering::SeqCst);
        send_rpc(&mut ws, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }));
        let _ = recv_rpc(&mut ws);
        // Hold ws open until client closes.
        while ws.read().is_ok() {}
    });

    let mut opts = ClientMcpOptions::new(
        "rune",
        "/work/repo",
        vec![McpTool::new("open_file", "Open file", json!({}))],
    );
    opts.reconnect_min = Duration::from_millis(20);
    opts.unsupported_retry = Duration::from_millis(250);

    let (mut mcp, _calls) = server.connect_client_mcp(opts, Arc::new(|| {}));

    // Must enter Unsupported immediately after 404, without spamming reconnects during the quiet window.
    wait_until(Duration::from_secs(2), || mcp.status() == ClientMcpStatus::Unsupported);
    std::thread::sleep(Duration::from_millis(90));
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "must not hammer a server missing the route at reconnect_min"
    );

    // Once unsupported_retry elapses and the server supports the route, it attaches and becomes Ready.
    wait_until(Duration::from_secs(3), || mcp.is_ready());
    assert_eq!(attempts.load(Ordering::SeqCst), 2);

    mcp.close();
    srv_thread.join().unwrap();
}

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

// The browser test talks to a fake engine over its Unix socket.
#[cfg(any(target_os = "macos", all(unix, feature = "browser")))]
use {
    browser_pane::BrowserHost,
    std::io::{BufRead, BufReader},
    std::os::unix::net::UnixListener,
    std::sync::Mutex,
};
use opencode_kit::client::Server;
use opencode_kit::client_mcp::{ClientMcpStatus, SESSION_ID_META_KEY};
use opencode_kit::model::{Project, ProjectIcon, SessionInfo};
use serde_json::{Value, json};
use tungstenite::handshake::server::{Request, Response};
use tungstenite::{Message, WebSocket};

use super::*;
use crate::fixture_ids::MAIN;
use crate::{App, Route, Source};

const DT: f64 = 1.0 / 120.0;

fn accept_ws(listener: &TcpListener) -> (WebSocket<TcpStream>, String, Option<String>) {
    let (stream, _) = listener.accept().expect("accept");
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut uri = String::new();
    let mut auth = None;
    let ws = tungstenite::accept_hdr(stream, |req: &Request, resp: Response| {
        uri = req.uri().to_string();
        auth = req.headers().get("Authorization").and_then(|v| v.to_str().ok()).map(str::to_owned);
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
            Message::Text(t) => return serde_json::from_str(&t).expect("json"),
            Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("unexpected ws message: {other:?}"),
        }
    }
}

fn recv_rpc_while(ws: &mut WebSocket<TcpStream>, mut step: impl FnMut()) -> Value {
    ws.get_ref().set_read_timeout(Some(Duration::from_millis(15))).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let out = loop {
        step();
        match ws.read() {
            Ok(Message::Text(t)) => break serde_json::from_str(&t).expect("json"),
            Ok(Message::Ping(_) | Message::Pong(_)) => {}
            Err(tungstenite::Error::Io(ref e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                assert!(Instant::now() < deadline, "timed out waiting for rpc reply");
            }
            other => panic!("unexpected ws read: {other:?}"),
        }
    };
    ws.get_ref().set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    out
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

fn handshake_ws(ws: &mut WebSocket<TcpStream>) -> Vec<Value> {
    send_rpc(
        ws,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "opencode", "version": "1.0.0" }
            }
        }),
    );
    let init = recv_rpc(ws);
    assert_eq!(init["result"]["serverInfo"]["name"], MCP_NAME);
    assert_eq!(init["result"]["instructions"], MCP_INSTRUCTIONS);

    send_rpc(ws, json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    send_rpc(ws, json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    let list = recv_rpc(ws);
    list["result"]["tools"].as_array().expect("tools array").clone()
}

fn call_tool_msg(id: u64, name: &str, args: Value, session_id: Option<&str>) -> Value {
    let mut params = json!({ "name": name, "arguments": args });
    if let Some(sid) = session_id {
        params["_meta"] = json!({ SESSION_ID_META_KEY: sid });
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": params,
    })
}

fn make_single_folder_app(server: Server, dir: &str, sid: &str) -> App {
    let mut app = App::empty();
    app.projects = vec![Project {
        id: "proj_1".into(),
        name: "repo".into(),
        directory: dir.into(),
        active: 1,
        icon: ProjectIcon::default(),
    }];
    app.project_state.used = vec!["proj_1".into()];
    app.sessions = vec![SessionInfo {
        id: sid.into(),
        parent: None,
        project: "proj_1".into(),
        directory: dir.into(),
        title: "Session".into(),
        created: 1,
        updated: 2,
        viewed: Some(2),
        ..SessionInfo::default()
    }];
    app.tabs = vec![crate::tabs::Tab::session(sid, "Session", dir)];
    app.route = Route::Session(sid.into());
    app.default_dir = dir.into();
    with_plugins(&mut app);
    app.source = Source::Live { server, requests: None, writes: None, allow_send: true };
    app
}

/// Loads the built-in plugins, whose tools (the review's `open_diff`) list after the app's own,
/// before the window has a server (so they read none) or a socket (so it lists them).
fn with_plugins(app: &mut App) {
    crate::plugin_host::setup(app, Arc::new(|| {}));
    crate::plugin_host::contrib_sync_luau(app);
}

#[test]
fn handshake_readiness_gap_and_full_tool_list() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = Server::new(&base).with_password("pw-123");
    let hub = RuneMcpHub::with_timings(
        Duration::from_millis(30),
        Duration::from_millis(150),
        Duration::from_millis(200),
    );

    let mut app = make_single_folder_app(server, "/work/repo", "ses_main");
    app.attach_test_mcp_hub(hub, Arc::new(|| {}));

    let (mut ws, uri, auth) = accept_ws(&listener);
    assert_eq!(uri, "/api/experimental/client-mcp?location%5Bdirectory%5D=%2Fwork%2Frepo&name=rune");
    assert_eq!(auth.as_deref(), Some("Basic b3BlbmNvZGU6cHctMTIz"));

    // Not ready before initialize or even after notifications/initialized until tools/list replies.
    assert!(!app.rune_mcp_ready("/work/repo"));
    send_rpc(&mut ws, json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" }));
    let init = recv_rpc(&mut ws);
    assert_eq!(init["result"]["instructions"], MCP_INSTRUCTIONS);
    assert!(!app.rune_mcp_ready("/work/repo"));

    send_rpc(&mut ws, json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    send_rpc(&mut ws, json!({ "jsonrpc": "2.0", "id": "p", "method": "ping" }));
    assert_eq!(recv_rpc(&mut ws), json!({ "jsonrpc": "2.0", "id": "p", "result": {} }));
    assert!(!app.rune_mcp_ready("/work/repo"));

    send_rpc(&mut ws, json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    let list = recv_rpc(&mut ws);
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            assert_eq!(t["inputSchema"]["type"], "object");
            t["name"].as_str().unwrap()
        })
        .collect();
    assert_eq!(
        names,
        vec![
            "browser_tabs",
            "browser_open",
            "browser_navigate",
            "browser_focus",
            "browser_close",
            "browser_snapshot",
            "browser_highlight",
            "browser_evaluate",
            "browser_screenshot",
            "browser_cdp",
            "open_file",
            "open_terminal",
            "open_diff",
        ]
    );
    wait_until(Duration::from_secs(2), || app.rune_mcp_ready("/work/repo"));
    app.save_on_close();
}

#[test]
fn e2e_headless_runtime_open_vs_unopened_session_and_side_panel_tools() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = Server::new(&base);
    let hub = RuneMcpHub::with_timings(
        Duration::from_millis(30),
        Duration::from_millis(150),
        Duration::from_millis(200),
    );

    let mut rt = crate::fixture_runtime();
    rt.frame(DT);

    let open_dir = rt.state.session(MAIN).unwrap().directory.clone();

    // Narrow to the single open session folder so the mock listener accepts one deterministic connection.
    rt.update(|s, _| {
        s.project_state.used.clear();
        s.tabs.retain(|t| t.id == MAIN);
        s.ensure_workspace_files(MAIN);
        s.ensure_session_review(MAIN);
        s.composer.focused = true;
        with_plugins(s);
        s.source = Source::Live {
            server: server.clone(),
            requests: None,
            writes: None,
            allow_send: true,
        };
        s.attach_test_mcp_hub(hub.clone(), Arc::new(|| {}));
    });

    let (mut ws, uri, _) = accept_ws(&listener);
    drop(listener);
    assert!(uri.contains("name=rune"), "uri: {uri}");
    let _ = handshake_ws(&mut ws);
    wait_until(Duration::from_secs(2), || rt.state.rune_mcp_ready(&open_dir));

    // 1. Session NOT open in OpenCode (Rune) -> plain error, side panel stays shut.
    send_rpc(&mut ws, call_tool_msg(10, "open_file", json!({ "path": "src/main.rs" }), Some("ses_not_open")));
    let closed_reply = recv_rpc_while(&mut ws, || {
        rt.frame(DT);
    });
    assert_eq!(closed_reply["result"]["isError"], true);
    assert_eq!(closed_reply["result"]["content"][0]["text"], SESSION_NOT_OPEN_ERROR);
    assert!(!rt.state.side_open());

    // 2. Missing sessionID metadata -> same plain error.
    send_rpc(&mut ws, call_tool_msg(11, "browser_tabs", json!({}), None));
    let missing_reply = recv_rpc_while(&mut ws, || {
        rt.frame(DT);
    });
    assert_eq!(missing_reply["result"]["isError"], true);
    assert_eq!(missing_reply["result"]["content"][0]["text"], SESSION_NOT_OPEN_ERROR);

    // 3. `open_file` on open session MAIN -> opens side panel at lines 2..=4 without stealing composer focus.
    send_rpc(
        &mut ws,
        call_tool_msg(
            12,
            "open_file",
            json!({ "path": "src/runtime.rs", "startLine": 2, "endLine": 4 }),
            Some(MAIN),
        ),
    );
    let file_reply = recv_rpc_while(&mut ws, || {
        rt.frame(DT);
    });
    assert_ne!(file_reply["result"]["isError"], Value::Bool(true), "{file_reply}");
    let file_meta: Value = serde_json::from_str(file_reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(file_meta, json!({ "path": "src/runtime.rs", "lines": [2, 4] }));
    assert!(rt.state.side_open());
    assert!(rt.state.composer.focused, "must not steal composer focus");

    // 4. `open_diff` -> switches to split Changes view without stealing composer focus.
    send_rpc(
        &mut ws,
        call_tool_msg(13, "open_diff", json!({ "mode": "uncommitted", "view": "split" }), Some(MAIN)),
    );
    let diff_reply = recv_rpc_while(&mut ws, || {
        rt.frame(DT);
    });
    let diff_meta: Value = serde_json::from_str(diff_reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(diff_meta["mode"], "uncommitted");
    assert_eq!(diff_meta["view"], "split");
    assert!(rt.state.is_review_active());
    assert!(rt.state.composer.focused);

    // 5. `open_terminal` -> opens the terminal's tab without stealing composer focus.
    send_rpc(&mut ws, call_tool_msg(14, "open_terminal", json!({}), Some(MAIN)));
    let term_reply = recv_rpc_while(&mut ws, || {
        rt.frame(DT);
    });
    let term_meta: Value = serde_json::from_str(term_reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(term_meta["open"], true);
    assert!(rt.state.side_open() || rt.state.bottom_open());
    assert!(rt.state.composer.focused);

    // 6. Browser lifecycle (`browser_open` -> `browser_tabs` -> `browser_navigate` -> `browser_focus` -> `browser_close`).
    send_rpc(&mut ws, call_tool_msg(15, "browser_open", json!({ "url": ":3000" }), Some(MAIN)));
    let open_b = recv_rpc_while(&mut ws, || {
        rt.frame(DT);
    });
    let open_b_meta: Value = serde_json::from_str(open_b["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let tab_id = open_b_meta["tabID"].as_u64().unwrap();
    assert_eq!(open_b_meta["url"], "http://localhost:3000");
    assert!(rt.state.composer.focused);

    send_rpc(&mut ws, call_tool_msg(16, "browser_navigate", json!({ "url": ":5173/app" }), Some(MAIN)));
    let nav_b = recv_rpc_while(&mut ws, || {
        rt.frame(DT);
    });
    let nav_meta: Value = serde_json::from_str(nav_b["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(nav_meta, json!({ "tabID": tab_id, "url": "http://localhost:5173/app" }));

    send_rpc(&mut ws, call_tool_msg(17, "browser_tabs", json!({}), Some(MAIN)));
    let list_b = recv_rpc_while(&mut ws, || {
        rt.frame(DT);
    });
    let list_meta: Value = serde_json::from_str(list_b["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(list_meta["focusedTabID"], tab_id);
    assert_eq!(list_meta["tabs"].as_array().unwrap().len(), 1);

    send_rpc(&mut ws, call_tool_msg(18, "browser_focus", json!({ "tabID": tab_id }), Some(MAIN)));
    let focus_b = recv_rpc_while(&mut ws, || {
        rt.frame(DT);
    });
    assert_ne!(focus_b["result"]["isError"], Value::Bool(true));

    send_rpc(&mut ws, call_tool_msg(19, "browser_close", json!({ "tabID": tab_id }), Some(MAIN)));
    let close_b = recv_rpc_while(&mut ws, || {
        rt.frame(DT);
    });
    let close_meta: Value = serde_json::from_str(close_b["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(close_meta["closedTabID"], tab_id);
    assert!(rt.state.browser.host.tabs().is_empty());

    // Path traversal outside workspace is rejected.
    send_rpc(&mut ws, call_tool_msg(20, "open_file", json!({ "path": "../outside.rs" }), Some(MAIN)));
    let bad_path = recv_rpc_while(&mut ws, || {
        rt.frame(DT);
    });
    assert_eq!(bad_path["result"]["isError"], true);

    rt.update(|s, _| s.save_on_close());
    assert_eq!(hub.connection_count(), 0);
}

#[test]
fn multi_window_shares_one_connection_and_routes_by_session_and_subagent() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = Server::new(&base);
    let hub = RuneMcpHub::with_timings(
        Duration::from_millis(30),
        Duration::from_millis(150),
        Duration::from_millis(200),
    );

    let mut win1 = make_single_folder_app(server.clone(), "/work/shared", "ses_w1");
    let mut win2 = make_single_folder_app(server, "/work/shared", "ses_w2");
    // Add a child subagent session under `ses_w2`.
    win2.sessions.push(SessionInfo {
        id: "ses_w2_sub".into(),
        parent: Some("ses_w2".into()),
        project: "proj_1".into(),
        directory: "/work/shared".into(),
        title: "Subagent".into(),
        created: 3,
        updated: 4,
        viewed: Some(4),
        ..SessionInfo::default()
    });

    win1.attach_test_mcp_hub(hub.clone(), Arc::new(|| {}));
    win2.attach_test_mcp_hub(hub.clone(), Arc::new(|| {}));
    assert_eq!(hub.connection_count(), 1, "both windows share one folder socket");

    // Accept the single shared WebSocket.
    let deadline = Instant::now() + Duration::from_secs(3);
    let stream = loop {
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "timed out waiting for shared ws");
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => panic!("accept failed: {e}"),
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut ws = tungstenite::accept(stream).expect("ws accept");
    let _ = handshake_ws(&mut ws);

    // Ensure no second socket was opened by the second window.
    assert!(listener.accept().is_err(), "must not open a second socket for the same folder");

    // Call for `ses_w1` -> lands only in `win1`.
    send_rpc(&mut ws, call_tool_msg(1, "browser_open", json!({ "url": ":4001" }), Some("ses_w1")));
    let r1 = recv_rpc_while(&mut ws, || {
        let mut tx = Tx::default();
        super::poll(&mut win1, &mut tx);
        super::poll(&mut win2, &mut tx);
    });
    assert_ne!(r1["result"]["isError"], Value::Bool(true));
    assert_eq!(win1.browser.host.tabs().len(), 1);
    assert_eq!(win2.browser.host.tabs().len(), 0);

    // Call from subagent `ses_w2_sub` -> routes via parent `ses_w2` to `win2`!
    send_rpc(&mut ws, call_tool_msg(2, "browser_open", json!({ "url": ":4002" }), Some("ses_w2_sub")));
    let r2 = recv_rpc_while(&mut ws, || {
        let mut tx = Tx::default();
        super::poll(&mut win1, &mut tx);
        super::poll(&mut win2, &mut tx);
    });
    assert_ne!(r2["result"]["isError"], Value::Bool(true));
    assert_eq!(win1.browser.host.tabs().len(), 1);
    assert_eq!(win2.browser.host.tabs().len(), 1);
    assert!(win2.side.sessions.get("ses_w2").is_some_and(|st| st.open));

    // Brand-new subagent `ses_w2_brand_new` (before SSE delivers its SessionInfo) resolves parentID via `/api/session/<id>`.
    send_rpc(
        &mut ws,
        call_tool_msg(3, "browser_open", json!({ "url": ":4003" }), Some("ses_w2_brand_new")),
    );
    let r3 = recv_rpc_while(&mut ws, || {
        if let Ok((mut http_s, _)) = listener.accept() {
            http_s.set_nonblocking(false).ok();
            http_s.set_read_timeout(Some(Duration::from_secs(2))).ok();
            let mut buf = [0u8; 2048];
            let n = http_s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let body = if req.contains("GET /api/session/ses_w2_brand_new") {
                r#"{"id":"ses_w2_brand_new","parentID":"ses_w2"}"#
            } else {
                r#"{"id":"ses_w2","parentID":null}"#
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = http_s.write_all(resp.as_bytes());
        }
        let mut tx = Tx::default();
        super::poll(&mut win1, &mut tx);
        super::poll(&mut win2, &mut tx);
    });
    assert_ne!(r3["result"]["isError"], Value::Bool(true));
    assert_eq!(win2.browser.host.tabs().len(), 2);

    // Closing win1 leaves the shared connection open for win2; closing win2 tears it down.
    win1.save_on_close();
    assert_eq!(hub.connection_count(), 1);
    win2.save_on_close();
    assert_eq!(hub.connection_count(), 0);
}

#[test]
fn folder_close_reconnect_and_server_without_route() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = Server::new(&base);
    let hub = RuneMcpHub::with_timings(
        Duration::from_millis(30),
        Duration::from_millis(120),
        Duration::from_millis(300),
    );

    let mut app = make_single_folder_app(server.clone(), "/work/repo", "ses_main");
    app.attach_test_mcp_hub(hub.clone(), Arc::new(|| {}));

    // Initial connection + drop -> reconnects and restores readiness.
    {
        let (mut ws1, _, _) = accept_ws(&listener);
        let _ = handshake_ws(&mut ws1);
        wait_until(Duration::from_secs(2), || app.rune_mcp_ready("/work/repo"));
        ws1.close(None).ok();
    }
    {
        let (mut ws2, _, _) = accept_ws(&listener);
        let _ = handshake_ws(&mut ws2);
        wait_until(Duration::from_secs(2), || app.rune_mcp_ready("/work/repo"));

        // Closing the folder (hiding project + going Home with no tabs) closes the socket.
        let mut tx = Tx::default();
        app.tabs.clear();
        app.route = Route::Home;
        app.hide_project("proj_1", &mut tx);
        super::poll(&mut app, &mut tx);
        assert_eq!(hub.connection_count(), 0);
    }

    // Switching to an older server without `/api/experimental/client-mcp` (HTTP 404) marks Unsupported quietly.
    let old_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let old_base = format!("http://{}", old_listener.local_addr().unwrap());
    let old_server = Server::new(&old_base);
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_srv = hits.clone();
    let stop_old = Arc::new(AtomicBool::new(false));
    let stop_old_srv = stop_old.clone();
    old_listener.set_nonblocking(true).unwrap();
    let old_thread = std::thread::spawn(move || {
        while !stop_old_srv.load(Ordering::Relaxed) {
            if let Ok((mut s, _)) = old_listener.accept() {
                s.set_nonblocking(false).ok();
                s.set_read_timeout(Some(Duration::from_secs(2))).ok();
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                hits_srv.fetch_add(1, Ordering::SeqCst);
                let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    });

    let mut tx = Tx::default();
    app.project_state.used = vec!["proj_1".into()];
    app.source = Source::Live {
        server: old_server.clone(),
        requests: None,
        writes: None,
        allow_send: true,
    };
    super::poll(&mut app, &mut tx);

    wait_until(Duration::from_secs(2), || {
        hub.status(&old_server, "/work/repo") == Some(ClientMcpStatus::Unsupported)
    });
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(hits.load(Ordering::SeqCst), 1, "quiet backoff on 404");

    app.save_on_close();
    stop_old.store(true, Ordering::Relaxed);
    old_thread.join().unwrap();
}

#[cfg(any(target_os = "macos", all(unix, feature = "browser")))]
#[test]
fn browser_cdp_snapshot_highlight_evaluate_and_screenshot_with_image() {
    let dir = std::env::temp_dir().join(format!("rune-mcp-sidecar-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock_path = dir.join("browser.sock");
    let _ = std::fs::remove_file(&sock_path);
    let sidecar_listener = UnixListener::bind(&sock_path).unwrap();

    let highlighted: Arc<Mutex<Vec<Option<i64>>>> = Arc::new(Mutex::new(Vec::new()));
    let highlighted_srv = highlighted.clone();

    let sidecar_thread = std::thread::spawn(move || {
        let (stream, _) = sidecar_listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut writer = stream.try_clone().unwrap();
        let reader = BufReader::new(stream);
        let send_engine = |w: &mut std::os::unix::net::UnixStream, v: Value| {
            writeln!(w, "{v}").unwrap();
            w.flush().unwrap();
        };
        send_engine(&mut writer, json!({ "type": "ready", "chromium_major": 156, "pid": 999 }));

        for line in reader.lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            let msg: Value = serde_json::from_str(&line).unwrap();
            match msg["type"].as_str().unwrap_or("") {
                "highlight_node" => {
                    highlighted_srv.lock().unwrap().push(msg["backend_node_id"].as_i64());
                }
                "cdp_call" => {
                    let tab_id = msg["tab_id"].as_u64().unwrap();
                    let req_id = msg["request_id"].as_u64().unwrap();
                    let result = match msg["method"].as_str().unwrap() {
                        "Accessibility.getFullAXTree" => json!({
                            "nodes": [
                                {
                                    "role": { "value": "heading" },
                                    "name": { "value": "Dashboard" }
                                },
                                {
                                    "role": { "value": "button" },
                                    "name": { "value": "Deploy" },
                                    "backendDOMNodeId": 42
                                }
                            ]
                        }),
                        "Runtime.evaluate" => json!({
                            "result": { "type": "number", "value": 7 }
                        }),
                        "Page.captureScreenshot" => json!({
                            "data": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
                        }),
                        _ => json!({ "pong": true }),
                    };
                    send_engine(
                        &mut writer,
                        json!({
                            "type": "cdp_response",
                            "tab_id": tab_id,
                            "request_id": req_id,
                            "ok": true,
                            "result": result,
                        }),
                    );
                }
                "shutdown" => break,
                _ => {}
            }
        }
    });

    let ws_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", ws_listener.local_addr().unwrap());
    let server = Server::new(&base);
    let hub = RuneMcpHub::with_timings(
        Duration::from_millis(30),
        Duration::from_millis(120),
        Duration::from_millis(200),
    );

    let mut app = make_single_folder_app(server, "/work/repo", "ses_main");
    app.browser.host = BrowserHost::with_socket(sock_path.clone());
    app.setup_browser(Arc::new(|| {}));
    app.attach_test_mcp_hub(hub, Arc::new(|| {}));

    let (mut ws, _, _) = accept_ws(&ws_listener);
    let _ = handshake_ws(&mut ws);

    let step = |app: &mut App| {
        let mut tx = Tx::default();
        app.poll_browser(&mut tx);
        super::poll(app, &mut tx);
    };

    // 1. Open tab -> attaches to mock sidecar and waits for Ready.
    send_rpc(&mut ws, call_tool_msg(1, "browser_open", json!({ "url": "https://anoma.ly" }), Some("ses_main")));
    let open_res = recv_rpc_while(&mut ws, || step(&mut app));
    let open_meta: Value = serde_json::from_str(open_res["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let tab_id = open_meta["tabID"].as_u64().unwrap();
    wait_until(Duration::from_secs(2), || {
        step(&mut app);
        app.browser.host.has_live_sidecar()
    });

    // 2. Snapshot -> formats AXTree with `@e1` actionable ref.
    send_rpc(&mut ws, call_tool_msg(2, "browser_snapshot", json!({ "tabID": tab_id }), Some("ses_main")));
    let snap_res = recv_rpc_while(&mut ws, || step(&mut app));
    let snap_meta: Value = serde_json::from_str(snap_res["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(snap_meta["snapshot"], "[heading] \"Dashboard\"\n@e1 [button] \"Deploy\"");

    // 3. Highlight `@e1` -> resolves backendDOMNodeId 42.
    send_rpc(
        &mut ws,
        call_tool_msg(3, "browser_highlight", json!({ "tabID": tab_id, "ref": "@e1" }), Some("ses_main")),
    );
    let hi_res = recv_rpc_while(&mut ws, || step(&mut app));
    let hi_meta: Value = serde_json::from_str(hi_res["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(hi_meta["backendNodeId"], 42);
    wait_until(Duration::from_secs(2), || highlighted.lock().unwrap().as_slice() == [Some(42)]);

    // 4. Evaluate -> returns unwrapped JS value.
    send_rpc(
        &mut ws,
        call_tool_msg(4, "browser_evaluate", json!({ "tabID": tab_id, "expression": "3 + 4" }), Some("ses_main")),
    );
    let eval_res = recv_rpc_while(&mut ws, || step(&mut app));
    let eval_meta: Value = serde_json::from_str(eval_res["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(eval_meta, json!({ "tabID": tab_id, "value": 7 }));

    // 5. Screenshot -> returns compact JSON metadata as text AND the PNG as `type: "image"`.
    send_rpc(&mut ws, call_tool_msg(5, "browser_screenshot", json!({ "tabID": tab_id }), Some("ses_main")));
    let shot_res = recv_rpc_while(&mut ws, || step(&mut app));
    let content = shot_res["result"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "text");
    let shot_meta: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(shot_meta["tabID"], tab_id);
    assert_eq!(shot_meta["format"], "png");
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["mimeType"], "image/png");
    assert!(content[1]["data"].as_str().unwrap().starts_with("iVBORw0KGgo"));

    // 6. Raw CDP call.
    send_rpc(
        &mut ws,
        call_tool_msg(6, "browser_cdp", json!({ "tabID": tab_id, "method": "Page.bringToFront" }), Some("ses_main")),
    );
    let raw_res = recv_rpc_while(&mut ws, || step(&mut app));
    let raw_meta: Value = serde_json::from_str(raw_res["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(raw_meta["result"], json!({ "pong": true }));

    app.browser.host.shutdown_sidecar();
    app.save_on_close();
    drop(app);
    sidecar_thread.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

//! Loopback JSON control server (`127.0.0.1:0`) so agents can drive `OpenCode (Rune)`'s live
//! browser pane (`tabs.list`, `tabs.open`, `tabs.focus`, `tabs.close`, `navigate`, `evaluate`,
//! `snapshot` AXTree `@eN` refs, `highlight`, and `screenshot`).
//!
//! Writes `<data_dir>/rune-opencode/browser-api.json` when running live; never starts in tests
//! or headless snapshots.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use rune::Waker;
use serde_json::{Value, json};

use crate::App;

struct PendingCall {
    reply: Sender<Value>,
    kind: CallKind,
}

enum CallKind {
    PassThrough,
    Snapshot { tab_id: u64 },
}

pub struct AgentServer {
    pub port: u16,
    info_file: Option<PathBuf>,
    stop: Arc<AtomicBool>,
    rx: Receiver<(Value, Sender<Value>)>,
    in_flight: HashMap<u64, PendingCall>,
    ref_nodes: HashMap<(u64, String), i64>,
    next_ref: u64,
    handle: Option<JoinHandle<()>>,
}

impl AgentServer {
    pub fn start(waker: Waker) -> Option<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").ok()?;
        let port = listener.local_addr().ok()?.port();
        listener.set_nonblocking(true).ok()?;

        let info_file = crate::restore::data_dir().map(|d| d.join("browser-api.json"));
        if let Some(path) = &info_file {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(
                path,
                json!({ "port": port, "pid": std::process::id() }).to_string(),
            );
        }

        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let (tx, rx) = mpsc::channel::<(Value, Sender<Value>)>();

        let handle = std::thread::Builder::new()
            .name("desktop-browser-api".into())
            .spawn(move || {
                while !stop_flag.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                            let mut buf = vec![0u8; 65536];
                            let n = stream.read(&mut buf).unwrap_or(0);
                            if n == 0 {
                                continue;
                            }
                            let raw = String::from_utf8_lossy(&buf[..n]);
                            let body_str = raw.split("\r\n\r\n").nth(1).unwrap_or("");
                            let req: Value = serde_json::from_str(body_str).unwrap_or(Value::Null);
                            let (rtx, rrx) = mpsc::channel();
                            if tx.send((req, rtx)).is_ok() {
                                waker();
                                let resp = rrx
                                    .recv_timeout(Duration::from_secs(10))
                                    .unwrap_or_else(|_| json!({ "ok": false, "error": "timeout" }));
                                let out = resp.to_string();
                                let hdr = format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                    out.len()
                                );
                                let _ = stream.write_all(hdr.as_bytes());
                                let _ = stream.write_all(out.as_bytes());
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(15));
                        }
                        Err(_) => break,
                    }
                }
            })
            .ok()?;

        Some(Self {
            port,
            info_file,
            stop,
            rx,
            in_flight: HashMap::new(),
            ref_nodes: HashMap::new(),
            next_ref: 1,
            handle: Some(handle),
        })
    }
}

impl Drop for AgentServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(p) = &self.info_file {
            let _ = std::fs::remove_file(p);
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

pub fn poll(s: &mut App) -> bool {
    let Some(mut api) = s.browser.api.take() else {
        return false;
    };
    let mut changed = false;

    while let Ok((req, reply)) = api.rx.try_recv() {
        changed = true;
        handle_request(s, &mut api, req, reply);
    }

    let ids: Vec<u64> = api.in_flight.keys().copied().collect();
    for req_id in ids {
        if let Some(res) = s.browser.host.take_cdp_response(req_id)
            && let Some(pending) = api.in_flight.remove(&req_id)
        {
            match (res, pending.kind) {
                (Ok(val), CallKind::PassThrough) => {
                    let _ = pending.reply.send(json!({ "ok": true, "result": val }));
                }
                (Ok(val), CallKind::Snapshot { tab_id }) => {
                    let text = format_ax_snapshot(&mut api, tab_id, &val);
                    let _ = pending.reply.send(json!({ "ok": true, "snapshot": text }));
                }
                (Err(err), _) => {
                    let _ = pending.reply.send(json!({ "ok": false, "error": err }));
                }
            }
        }
    }

    s.browser.api = Some(api);
    changed
}

fn handle_request(s: &mut App, api: &mut AgentServer, req: Value, reply: Sender<Value>) {
    let op = req["op"].as_str().unwrap_or("");
    let tab_id = req["tabID"]
        .as_u64()
        .or_else(|| s.browser.host.focused_tab_id());

    match op {
        "tabs.list" => {
            let tabs: Vec<Value> = s
                .browser
                .host
                .tabs()
                .iter()
                .map(|t| {
                    json!({
                        "tabID": t.id,
                        "url": t.url,
                        "title": t.title,
                        "loading": t.loading,
                        "canGoBack": t.can_go_back,
                        "canGoForward": t.can_go_forward,
                    })
                })
                .collect();
            let _ = reply.send(json!({
                "ok": true,
                "open": s.browser.open,
                "focusedTabID": s.browser.host.focused_tab_id(),
                "tabs": tabs,
            }));
        }
        "tabs.open" => {
            let url = req["url"].as_str().unwrap_or("about:blank");
            s.browser.open = true;
            let id = s.open_new_browser_tab(url);
            let _ = reply.send(json!({ "ok": true, "tabID": id }));
        }
        "tabs.focus" => {
            if let Some(id) = tab_id {
                s.browser.open = true;
                s.browser.host.focus_tab(id);
                let _ = reply.send(json!({ "ok": true, "tabID": id }));
            } else {
                let _ = reply.send(json!({ "ok": false, "error": "no tabID" }));
            }
        }
        "tabs.close" => {
            if let Some(id) = tab_id {
                s.browser.host.close_tab(id);
                if s.browser.host.tabs().is_empty() {
                    s.browser.open = false;
                }
                let _ = reply.send(json!({ "ok": true }));
            } else {
                let _ = reply.send(json!({ "ok": false, "error": "no tabID" }));
            }
        }
        "navigate" => {
            let url = req["url"].as_str().unwrap_or("about:blank");
            if let Some(id) = tab_id {
                s.browser.host.navigate(id, url);
                let _ = reply.send(json!({ "ok": true, "tabID": id }));
            } else {
                s.open_browser_url(url);
                let _ = reply.send(json!({ "ok": true, "tabID": s.browser.host.focused_tab_id() }));
            }
        }
        "evaluate" => {
            let Some(id) = tab_id else {
                let _ = reply.send(json!({ "ok": false, "error": "no open browser tab" }));
                return;
            };
            let expr = req["expression"].as_str().unwrap_or("null");
            let cdp_id = s.browser.host.send_cdp(
                id,
                "Runtime.evaluate",
                json!({ "expression": expr, "returnByValue": true, "awaitPromise": true }),
                None,
            );
            api.in_flight.insert(cdp_id, PendingCall { reply, kind: CallKind::PassThrough });
        }
        "snapshot" => {
            let Some(id) = tab_id else {
                let _ = reply.send(json!({ "ok": false, "error": "no open browser tab" }));
                return;
            };
            let cdp_id = s.browser.host.send_cdp(
                id,
                "Accessibility.getFullAXTree",
                json!({ "depth": req["depth"].as_u64().unwrap_or(8) }),
                None,
            );
            api.in_flight
                .insert(cdp_id, PendingCall { reply, kind: CallKind::Snapshot { tab_id: id } });
        }
        "highlight" => {
            let Some(id) = tab_id else {
                let _ = reply.send(json!({ "ok": false, "error": "no open browser tab" }));
                return;
            };
            let eref = req["ref"].as_str().unwrap_or("").trim_start_matches('@');
            let backend = api.ref_nodes.get(&(id, format!("@{eref}"))).copied();
            s.browser.host.highlight_node(id, backend);
            let _ = reply.send(json!({ "ok": true, "backendNodeId": backend }));
        }
        "screenshot" => {
            let Some(id) = tab_id else {
                let _ = reply.send(json!({ "ok": false, "error": "no open browser tab" }));
                return;
            };
            let cdp_id = s.browser.host.send_cdp(
                id,
                "Page.captureScreenshot",
                json!({ "format": req["format"].as_str().unwrap_or("png") }),
                None,
            );
            api.in_flight.insert(cdp_id, PendingCall { reply, kind: CallKind::PassThrough });
        }
        "cdp" => {
            let Some(id) = tab_id else {
                let _ = reply.send(json!({ "ok": false, "error": "no open browser tab" }));
                return;
            };
            let method = req["method"].as_str().unwrap_or("Runtime.evaluate");
            let params = req.get("params").cloned().unwrap_or_else(|| json!({}));
            let session_id = req["sessionId"].as_str().map(str::to_owned);
            let cdp_id = s.browser.host.send_cdp(id, method, params, session_id);
            api.in_flight.insert(cdp_id, PendingCall { reply, kind: CallKind::PassThrough });
        }
        _ => {
            let _ = reply.send(json!({ "ok": false, "error": format!("unknown op {op:?}") }));
        }
    }
}

fn format_ax_snapshot(api: &mut AgentServer, tab_id: u64, val: &Value) -> String {
    api.ref_nodes.retain(|(tid, _), _| *tid != tab_id);
    let Some(nodes) = val["nodes"].as_array() else {
        return String::new();
    };
    let mut lines = Vec::new();
    for node in nodes.iter().take(300) {
        if node["ignored"].as_bool() == Some(true) {
            continue;
        }
        let role = node["role"]["value"].as_str().unwrap_or("node");
        let name = node["name"]["value"].as_str().unwrap_or("").trim();
        let actionable = matches!(
            role,
            "button" | "link" | "textbox" | "searchbox" | "combobox" | "checkbox" | "radio" | "option"
        );
        let prefix = if actionable && let Some(bid) = node["backendDOMNodeId"].as_i64() {
            let eref = format!("@e{}", api.next_ref);
            api.next_ref += 1;
            api.ref_nodes.insert((tab_id, eref.clone()), bid);
            format!("{eref} ")
        } else {
            String::new()
        };
        if actionable || !name.is_empty() {
            lines.push(format!("{prefix}[{role}] {name:?}"));
        }
    }
    lines.join("\n")
}

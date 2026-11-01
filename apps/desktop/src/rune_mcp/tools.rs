//! The `rune` client MCP tool definitions and execution on `App` (`tools.rune.*` in Code Mode).
//!
//! Exposes OpenCode (Rune)'s own browser pane (porting all capabilities of the retired loopback
//! browser API) and side-panel openers (`open_file`, `open_diff`, `open_terminal`).

use std::time::{Duration, Instant};

use opencode_kit::client_mcp::{McpCall, McpResult, McpTool};
use rune::Tx;
use rune_kit::diff::DiffMode;
use serde_json::{Value, json};

use crate::App;
use crate::review::ChangeMode;

const CDP_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_TEXT_BYTES: usize = 32 * 1024;

pub(super) struct PendingCdp {
    pub call: McpCall,
    pub tab_id: u64,
    pub kind: CdpKind,
    pub deadline: Instant,
}

pub(super) enum CdpKind {
    Snapshot,
    Evaluate,
    Screenshot { format: String },
    Raw,
}

pub fn catalog() -> Vec<McpTool> {
    vec![
        McpTool::new(
            "browser_tabs",
            "List browser tabs in this session's OpenCode (Rune) side panel and which tab is focused.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        McpTool::new(
            "browser_open",
            "Open a URL or local ':port' in a new browser tab in the OpenCode (Rune) side panel.",
            json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "URL or ':port' address to open (defaults to 'about:blank')."
                    }
                },
                "additionalProperties": false
            }),
        ),
        McpTool::new(
            "browser_navigate",
            "Navigate a browser tab in OpenCode (Rune) to a URL or ':port', opening a tab if none exists.",
            json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "URL or ':port' address to navigate to."
                    },
                    "tabID": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Target browser tab ID (defaults to this session's active browser tab)."
                    }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
        ),
        McpTool::new(
            "browser_focus",
            "Show and focus an existing browser tab in the OpenCode (Rune) side panel.",
            json!({
                "type": "object",
                "properties": {
                    "tabID": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Browser tab ID from browser_tabs."
                    }
                },
                "required": ["tabID"],
                "additionalProperties": false
            }),
        ),
        McpTool::new(
            "browser_close",
            "Close a browser tab in the OpenCode (Rune) side panel.",
            json!({
                "type": "object",
                "properties": {
                    "tabID": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Browser tab ID to close (defaults to this session's active browser tab)."
                    }
                },
                "additionalProperties": false
            }),
        ),
        McpTool::new(
            "browser_snapshot",
            "Read the accessibility tree of a browser tab in OpenCode (Rune) with '@eN' refs on actionable elements.",
            json!({
                "type": "object",
                "properties": {
                    "tabID": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Browser tab ID (defaults to this session's active browser tab)."
                    },
                    "depth": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 32,
                        "description": "Maximum accessibility tree depth (default 8)."
                    }
                },
                "additionalProperties": false
            }),
        ),
        McpTool::new(
            "browser_highlight",
            "Highlight an '@eN' accessibility ref in a browser tab in OpenCode (Rune), or omit 'ref' to clear.",
            json!({
                "type": "object",
                "properties": {
                    "ref": {
                        "type": "string",
                        "description": "Element ref from browser_snapshot (e.g. '@e3'); omit or pass '' to clear."
                    },
                    "tabID": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Browser tab ID (defaults to this session's active browser tab)."
                    }
                },
                "additionalProperties": false
            }),
        ),
        McpTool::new(
            "browser_evaluate",
            "Evaluate a JavaScript expression in a browser tab in OpenCode (Rune) and return its value.",
            json!({
                "type": "object",
                "properties": {
                    "expression": {
                        "type": "string",
                        "description": "JavaScript expression to evaluate (promises are awaited)."
                    },
                    "tabID": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Browser tab ID (defaults to this session's active browser tab)."
                    }
                },
                "required": ["expression"],
                "additionalProperties": false
            }),
        ),
        McpTool::new(
            "browser_screenshot",
            "Capture a screenshot of a browser tab in OpenCode (Rune), returning compact metadata and the image.",
            json!({
                "type": "object",
                "properties": {
                    "tabID": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Browser tab ID (defaults to this session's active browser tab)."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["png", "jpeg", "webp"],
                        "description": "Image format (default 'png')."
                    }
                },
                "additionalProperties": false
            }),
        ),
        McpTool::new(
            "browser_cdp",
            "Send a raw Chrome DevTools Protocol command to a browser tab in OpenCode (Rune).",
            json!({
                "type": "object",
                "properties": {
                    "method": {
                        "type": "string",
                        "description": "CDP method name, e.g. 'Input.dispatchMouseEvent' or 'DOM.getDocument'."
                    },
                    "params": {
                        "type": "object",
                        "description": "CDP method parameters."
                    },
                    "tabID": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Browser tab ID (defaults to this session's active browser tab)."
                    },
                    "sessionId": {
                        "type": "string",
                        "description": "Optional CDP target sessionId."
                    }
                },
                "required": ["method"],
                "additionalProperties": false
            }),
        ),
        McpTool::new(
            "open_file",
            "Open a workspace file in the OpenCode (Rune) side panel, optionally selecting a line or line range.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative or absolute file path inside the session folder."
                    },
                    "startLine": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-based start line to select and scroll into view."
                    },
                    "endLine": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-based end line (defaults to startLine)."
                    },
                    "preview": {
                        "type": "boolean",
                        "description": "Open in the transient preview tab instead of pinning (default false)."
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        McpTool::new(
            "open_diff",
            "Open the Changes diff view in the OpenCode (Rune) side panel, optionally scoped to a mode and file.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Optional file path in Changes to reveal and scroll to."
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["uncommitted", "branch", "turn"],
                        "description": "Which changes to show (defaults to the session's current mode)."
                    },
                    "view": {
                        "type": "string",
                        "enum": ["unified", "split"],
                        "description": "Unified or split diff layout."
                    },
                    "line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Optional 1-based line number in the new file to scroll near when 'path' is set."
                    }
                },
                "additionalProperties": false
            }),
        ),
        McpTool::new(
            "open_terminal",
            "Show the integrated terminal dock in OpenCode (Rune) for this session's workspace.",
            json!({
                "type": "object",
                "properties": {
                    "newTab": {
                        "type": "boolean",
                        "description": "Create a new terminal tab instead of reusing the active one (default false)."
                    }
                },
                "additionalProperties": false
            }),
        ),
    ]
}

pub(super) fn execute(s: &mut App, sid: &str, call: McpCall, tx: &mut Tx) {
    let name = call.name.clone();
    let args = call.arguments.clone();
    let call_dir = call.directory.clone();
    let tab_arg = args["tabID"].as_u64();

    match name.as_str() {
        "browser_tabs" => {
            let scoped = s.browser.session_tabs.get(sid);
            let tabs: Vec<Value> = s
                .browser
                .host
                .tabs()
                .iter()
                .filter(|t| scoped.is_none_or(|ids| ids.is_empty() || ids.contains(&t.id)))
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
            let side_open = s.side.sessions.get(sid).is_some_and(|st| st.open);
            let focused = s.default_browser_tab_for_session(sid);
            call.respond(McpResult::json(json!({
                "open": side_open,
                "focusedTabID": focused,
                "tabs": tabs,
            })));
        }
        "browser_open" => {
            tx.instant();
            let raw = args["url"].as_str().unwrap_or("about:blank");
            let id = s.open_new_browser_tab_for_session(Some(sid), raw, None, false);
            let url = s.browser.host.tab(id).map_or_else(|| browser_pane::resolve_address(raw), |t| t.url.clone());
            call.respond(McpResult::json(json!({ "tabID": id, "url": url })));
        }
        "browser_navigate" => {
            let Some(raw) = args["url"].as_str().filter(|u| !u.trim().is_empty()) else {
                call.respond(McpResult::error("Missing 'url'"));
                return;
            };
            tx.instant();
            if let Some(req_id) = tab_arg
                && s.browser.host.tab(req_id).is_none()
            {
                call.respond(McpResult::error(format!("No browser tab with tabID {req_id}")));
                return;
            }
            let target = tab_arg.or_else(|| s.default_browser_tab_for_session(sid));
            let id = match target {
                Some(id) => {
                    s.browser.host.navigate(id, raw);
                    s.register_existing_browser_tab_for_session(sid, id, false);
                    id
                }
                None => s.open_new_browser_tab_for_session(Some(sid), raw, None, false),
            };
            let url = s.browser.host.tab(id).map_or_else(|| browser_pane::resolve_address(raw), |t| t.url.clone());
            call.respond(McpResult::json(json!({ "tabID": id, "url": url })));
        }
        "browser_focus" => {
            let Some(id) = tab_arg else {
                call.respond(McpResult::error("Missing 'tabID'"));
                return;
            };
            let Some((url, title)) = s.browser.host.tab(id).map(|t| (t.url.clone(), t.title.clone())) else {
                call.respond(McpResult::error(format!("No browser tab with tabID {id}")));
                return;
            };
            tx.instant();
            s.register_existing_browser_tab_for_session(sid, id, false);
            s.browser.host.focus_tab(id);
            s.sync_browser_address(true);
            call.respond(McpResult::json(json!({ "tabID": id, "url": url, "title": title })));
        }
        "browser_close" => {
            let Some(id) = tab_arg.or_else(|| s.default_browser_tab_for_session(sid)) else {
                call.respond(McpResult::error("No open browser tab"));
                return;
            };
            if s.browser.host.tab(id).is_none() {
                call.respond(McpResult::error(format!("No browser tab with tabID {id}")));
                return;
            }
            tx.instant();
            s.close_browser_tab_for_session(sid, id, tx);
            call.respond(McpResult::json(json!({ "closedTabID": id })));
        }
        "browser_highlight" => {
            let Some(id) = resolve_live_tab(s, sid, tab_arg, &call) else {
                call.respond(missing_tab_error(tab_arg));
                return;
            };
            let raw_ref = args["ref"].as_str().unwrap_or("").trim();
            if raw_ref.is_empty() {
                s.browser.host.highlight_node(id, None);
                call.respond(McpResult::json(json!({ "tabID": id, "backendNodeId": Value::Null })));
                return;
            }
            let key = format!("@{}", raw_ref.trim_start_matches('@'));
            let Some(backend) = s.browser.ref_nodes.get(&(id, key.clone())).copied() else {
                call.respond(McpResult::error(format!(
                    "Unknown ref '{key}' for tab {id}; run browser_snapshot first"
                )));
                return;
            };
            s.browser.host.highlight_node(id, Some(backend));
            call.respond(McpResult::json(json!({ "tabID": id, "ref": key, "backendNodeId": backend })));
        }
        "browser_snapshot" => {
            let depth = args["depth"].as_u64().unwrap_or(8).clamp(1, 32);
            send_cdp_call(
                s,
                sid,
                tab_arg,
                call,
                "Accessibility.getFullAXTree",
                json!({ "depth": depth }),
                None,
                CdpKind::Snapshot,
            );
        }
        "browser_evaluate" => {
            let Some(expr) = args["expression"].as_str() else {
                call.respond(McpResult::error("Missing 'expression'"));
                return;
            };
            send_cdp_call(
                s,
                sid,
                tab_arg,
                call,
                "Runtime.evaluate",
                json!({ "expression": expr, "returnByValue": true, "awaitPromise": true }),
                None,
                CdpKind::Evaluate,
            );
        }
        "browser_screenshot" => {
            let fmt = match args["format"].as_str().unwrap_or("png") {
                "jpeg" | "jpg" => "jpeg",
                "webp" => "webp",
                _ => "png",
            };
            send_cdp_call(
                s,
                sid,
                tab_arg,
                call,
                "Page.captureScreenshot",
                json!({ "format": fmt }),
                None,
                CdpKind::Screenshot { format: fmt.to_owned() },
            );
        }
        "browser_cdp" => {
            let Some(method) = args["method"].as_str().filter(|m| !m.trim().is_empty()) else {
                call.respond(McpResult::error("Missing 'method'"));
                return;
            };
            let params = args.get("params").filter(|v| v.is_object()).cloned().unwrap_or_else(|| json!({}));
            let cdp_session = args["sessionId"].as_str().map(str::to_owned);
            let method_owned = method.to_owned();
            send_cdp_call(s, sid, tab_arg, call, &method_owned, params, cdp_session, CdpKind::Raw);
        }
        "open_file" => {
            let Some(raw_path) = args["path"].as_str().filter(|p| !p.trim().is_empty()) else {
                call.respond(McpResult::error("Missing 'path'"));
                return;
            };
            let dir = session_directory(s, sid, &call_dir);
            let Some(path) = resolve_workspace_path(raw_path, &dir) else {
                call.respond(McpResult::error(format!(
                    "Path '{raw_path}' is outside the session folder '{dir}'"
                )));
                return;
            };
            let start = args["startLine"].as_u64().map(|n| n.max(1) as u32);
            let lines = start.map(|a| {
                let b = args["endLine"].as_u64().map_or(a, |n| (n.max(1) as u32).max(a));
                (a, b)
            });
            let preview = args["preview"].as_bool().unwrap_or(false);
            s.open_workspace_file_for_session(sid, &path, preview, lines, false, tx);
            call.respond(McpResult::json(json!({
                "path": path,
                "lines": lines.map(|(a, b)| json!([a, b])),
            })));
        }
        "open_diff" => {
            let mode = match args["mode"].as_str() {
                Some("uncommitted") => Some(ChangeMode::Git),
                Some("branch") => Some(ChangeMode::Branch),
                Some("turn") => Some(ChangeMode::Turn),
                None => None,
                Some(other) => {
                    call.respond(McpResult::error(format!("Invalid diff mode '{other}'")));
                    return;
                }
            };
            let view = match args["view"].as_str() {
                Some("unified") => Some(DiffMode::Unified),
                Some("split") => Some(DiffMode::Split),
                None => None,
                Some(other) => {
                    call.respond(McpResult::error(format!("Invalid diff view '{other}'")));
                    return;
                }
            };
            let dir = session_directory(s, sid, &call_dir);
            let rel_path = match args["path"].as_str().filter(|p| !p.trim().is_empty()) {
                Some(raw) => match resolve_workspace_path(raw, &dir) {
                    Some(p) => Some(p),
                    None => {
                        call.respond(McpResult::error(format!(
                            "Path '{raw}' is outside the session folder '{dir}'"
                        )));
                        return;
                    }
                },
                None => None,
            };
            let line = args["line"].as_u64().map(|n| n.max(1) as u32);
            s.open_review_for_session(sid, mode, view, rel_path.as_deref(), line, tx);
            let active_mode = match s.review.sessions.get(sid).map(|r| r.mode).unwrap_or_default() {
                ChangeMode::Git => "uncommitted",
                ChangeMode::Branch => "branch",
                ChangeMode::Turn => "turn",
            };
            let active_view = match s.review.diff_mode {
                DiffMode::Unified => "unified",
                DiffMode::Split => "split",
            };
            call.respond(McpResult::json(json!({
                "mode": active_mode,
                "view": active_view,
                "path": rel_path,
            })));
        }
        "open_terminal" => {
            tx.instant();
            let new_tab = args["newTab"].as_bool().unwrap_or(false);
            let tab_id = s.open_terminal_for_session(sid, new_tab);
            call.respond(McpResult::json(json!({
                "open": true,
                "tabID": tab_id,
                "placement": s.dock_placement().name(),
            })));
        }
        other => call.respond(McpResult::error(format!("Unknown tool: {other}"))),
    }
}

#[allow(clippy::too_many_arguments)]
fn send_cdp_call(
    s: &mut App,
    sid: &str,
    tab_arg: Option<u64>,
    call: McpCall,
    method: &str,
    params: Value,
    cdp_session: Option<String>,
    kind: CdpKind,
) {
    let Some(id) = resolve_live_tab(s, sid, tab_arg, &call) else {
        call.respond(missing_tab_error(tab_arg));
        return;
    };
    if !s.browser.host.has_live_sidecar() {
        let msg = match s.browser.host.status() {
            browser_pane::EngineStatus::NotInstalled | browser_pane::EngineStatus::Downloading { .. } => {
                "Browser engine isn't installed yet in OpenCode (Rune)".to_owned()
            }
            browser_pane::EngineStatus::Failed(err) => format!("Browser engine failed to start: {err}"),
            _ => "Browser engine is still starting; try again in a moment".to_owned(),
        };
        call.respond(McpResult::error(msg));
        return;
    }
    let req_id = s.browser.host.send_cdp(id, method, params, cdp_session);
    s.rune_mcp.in_flight.insert(
        req_id,
        PendingCdp {
            call,
            tab_id: id,
            kind,
            deadline: Instant::now() + CDP_TIMEOUT,
        },
    );
}

pub(super) fn poll_cdp(s: &mut App, tx: &mut Tx) -> bool {
    if s.rune_mcp.in_flight.is_empty() {
        return false;
    }
    let now = Instant::now();
    let mut changed = false;
    let ids: Vec<u64> = s.rune_mcp.in_flight.keys().copied().collect();
    for req_id in ids {
        if let Some(res) = s.browser.host.take_cdp_response(req_id) {
            if let Some(pending) = s.rune_mcp.in_flight.remove(&req_id) {
                finish_cdp(s, pending, res);
                changed = true;
            }
            continue;
        }
        let timed_out = s.rune_mcp.in_flight.get(&req_id).is_some_and(|p| now >= p.deadline);
        if timed_out && let Some(pending) = s.rune_mcp.in_flight.remove(&req_id) {
            pending.call.respond(McpResult::error("Browser command timed out"));
            changed = true;
        }
    }
    if let Some(next_deadline) = s.rune_mcp.in_flight.values().map(|p| p.deadline).min() {
        tx.wake_after(next_deadline.saturating_duration_since(now).max(Duration::from_millis(25)));
    }
    changed
}

fn finish_cdp(s: &mut App, pending: PendingCdp, res: Result<Value, Value>) {
    let tab_id = pending.tab_id;
    let (url, title) = s
        .browser
        .host
        .tab(tab_id)
        .map_or_else(|| (String::new(), String::new()), |t| (t.url.clone(), t.title.clone()));

    let val = match res {
        Ok(v) => v,
        Err(err) => {
            let msg = err["message"].as_str().map(str::to_owned).unwrap_or_else(|| err.to_string());
            pending.call.respond(McpResult::error(msg));
            return;
        }
    };

    match pending.kind {
        CdpKind::Snapshot => {
            let snapshot = format_ax_snapshot(&mut s.browser, tab_id, &val);
            pending.call.respond(McpResult::json(json!({
                "tabID": tab_id,
                "url": url,
                "title": title,
                "snapshot": snapshot,
            })));
        }
        CdpKind::Evaluate => {
            if let Some(exc) = val.get("exceptionDetails").filter(|v| !v.is_null()) {
                let detail = exc["exception"]["description"]
                    .as_str()
                    .or_else(|| exc["text"].as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| exc.to_string());
                pending.call.respond(McpResult::error(detail));
                return;
            }
            let result = val.get("result").cloned().unwrap_or(Value::Null);
            let value = if let Some(v) = result.get("value") {
                compact_value(v.clone())
            } else if let Some(desc) = result.get("description").and_then(Value::as_str) {
                Value::String(truncate_str(desc, MAX_TEXT_BYTES))
            } else {
                compact_value(result)
            };
            pending.call.respond(McpResult::json(json!({ "tabID": tab_id, "value": value })));
        }
        CdpKind::Screenshot { format } => {
            let Some(data) = val["data"].as_str().filter(|d| !d.is_empty()) else {
                pending.call.respond(McpResult::error("Screenshot returned no image data"));
                return;
            };
            let mime = match format.as_str() {
                "jpeg" => "image/jpeg",
                "webp" => "image/webp",
                _ => "image/png",
            };
            let meta = json!({ "tabID": tab_id, "url": url, "title": title, "format": format });
            pending.call.respond(McpResult::with_image(meta.to_string(), data, mime));
        }
        CdpKind::Raw => {
            if let Some(data) = val.get("data").and_then(Value::as_str).filter(|d| d.len() > 256)
                && val.as_object().is_some_and(|o| o.len() == 1)
            {
                let meta = json!({ "tabID": tab_id, "url": url, "title": title });
                pending.call.respond(McpResult::with_image(meta.to_string(), data, "image/png"));
                return;
            }
            pending.call.respond(McpResult::json(json!({
                "tabID": tab_id,
                "result": compact_value(val),
            })));
        }
    }
}

fn format_ax_snapshot(browser: &mut crate::browser::BrowserPane, tab_id: u64, val: &Value) -> String {
    browser.ref_nodes.retain(|(tid, _), _| *tid != tab_id);
    let Some(nodes) = val["nodes"].as_array() else { return String::new() };
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
            let eref = format!("@e{}", browser.next_ref);
            browser.next_ref += 1;
            browser.ref_nodes.insert((tab_id, eref.clone()), bid);
            format!("{eref} ")
        } else {
            String::new()
        };
        if actionable || !name.is_empty() {
            lines.push(format!("{prefix}[{role}] {name:?}"));
        }
    }
    truncate_str(&lines.join("\n"), MAX_TEXT_BYTES)
}

fn resolve_live_tab(s: &App, sid: &str, tab_arg: Option<u64>, _call: &McpCall) -> Option<u64> {
    let id = tab_arg.or_else(|| s.default_browser_tab_for_session(sid))?;
    s.browser.host.tab(id).map(|_| id)
}

fn missing_tab_error(tab_arg: Option<u64>) -> McpResult {
    match tab_arg {
        Some(id) => McpResult::error(format!("No browser tab with tabID {id}")),
        None => McpResult::error("No open browser tab"),
    }
}

fn session_directory(s: &App, sid: &str, fallback: &str) -> String {
    s.session(sid)
        .map(|i| i.directory.as_str())
        .filter(|d| !d.is_empty())
        .or_else(|| (!fallback.is_empty()).then_some(fallback))
        .unwrap_or(&s.draft_dir)
        .to_owned()
}

fn resolve_workspace_path(raw: &str, directory: &str) -> Option<String> {
    if raw.contains("..") && raw.split('/').any(|seg| seg == "..") {
        return None;
    }
    crate::tool_open::relative(raw, directory)
}

fn truncate_str(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_owned();
    }
    let end = s.floor_char_boundary(max_bytes);
    format!("{}… [truncated {} bytes]", &s[..end], s.len() - end)
}

fn compact_value(v: Value) -> Value {
    match v {
        Value::String(s) => Value::String(truncate_str(&s, MAX_TEXT_BYTES)),
        other => {
            let raw = other.to_string();
            if raw.len() <= MAX_TEXT_BYTES {
                other
            } else {
                json!({ "truncated": true, "preview": truncate_str(&raw, MAX_TEXT_BYTES) })
            }
        }
    }
}

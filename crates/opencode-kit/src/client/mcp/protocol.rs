//! Shared MCP JSON-RPC 2.0 frame dispatch and payload formatting for native and web transports.

use std::sync::mpsc::Sender;

use rune::Waker;
use serde_json::{Value, json};

use super::{
    ClientMcpOptions, Conn, McpCall, McpTool, Outbound, Outbox, PROTOCOL_VERSION,
    SESSION_ID_META_KEY, Shared, lock, mark_ready, transition,
};

/// What goes on the socket for an answer or a tool-list change; an answer settles a call.
pub(super) fn outbound_message(out: Outbound, in_flight: &mut usize) -> Value {
    match out {
        Outbound::Reply { id, result } => {
            *in_flight = in_flight.saturating_sub(1);
            json!({ "jsonrpc": "2.0", "id": id, "result": result.to_json() })
        }
        Outbound::ToolsChanged => json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" }),
    }
}

/// One JSON-RPC frame from the server: answers go out through `send` (false when the socket is
/// gone), a `tools/call` goes to the app. False when the app stopped taking calls.
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_frame(
    send: &mut dyn FnMut(&Value) -> bool,
    raw: &str,
    options: &ClientMcpOptions,
    calls: &Sender<McpCall>,
    waker: &Waker,
    shared: &Shared,
    out: &Outbox,
    in_flight: &mut usize,
    initialized: &mut bool,
) -> bool {
    let Ok(msg) = serde_json::from_str::<Value>(raw) else { return true };
    let Some(method) = msg.as_object().and_then(|o| o.get("method")).and_then(Value::as_str) else { return true };
    if method == "notifications/initialized" {
        // Match `connectClientMcp`: reset backoff only after the server confirms initialization.
        *initialized = true;
    }
    let Some(id) = msg.get("id").filter(|v| v.is_string() || v.is_number()).cloned() else { return true };
    let params = msg.get("params");

    match method {
        "initialize" => {
            let requested = params.and_then(|p| p.get("protocolVersion")).and_then(Value::as_str);
            let supports = requested.is_some_and(|v| v >= "2024-11-05");
            // 2024-11-05..2025-11-25 deliver `notifications/tools/list_changed` on the socket;
            // 2026-07-28+ moved list-changed onto `subscriptions/listen`, so cap at 2025-11-25.
            let proto = match requested {
                Some(v) if ("2024-11-05".."2026-07-28").contains(&v) => v,
                _ => PROTOCOL_VERSION,
            };
            let mut result = json!({
                "protocolVersion": proto,
                "capabilities": { "tools": { "listChanged": true } },
                "serverInfo": { "name": options.name, "version": options.version },
            });
            if let Some(inst) = &options.instructions {
                result["instructions"] = Value::String(inst.clone());
            }
            if send(&json!({ "jsonrpc": "2.0", "id": id, "result": result })) {
                transition(shared, |conn| {
                    if let Conn::Open { list_changed, .. } = conn {
                        *list_changed = supports;
                    }
                    None
                }, waker);
            }
        }
        "ping" => {
            send(&json!({ "jsonrpc": "2.0", "id": id, "result": {} }));
        }
        "tools/list" => {
            let (listed_gen, tools_json) = {
                let st = lock(&shared.state);
                (st.tools_gen, st.tools.iter().map(tool_descriptor_json).collect::<Vec<Value>>())
            };
            if send(&json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": tools_json } })) {
                mark_ready(shared, listed_gen, waker);
            }
        }
        "tools/call" => {
            let tool_name = params.and_then(|p| p.get("name")).and_then(Value::as_str);
            let known = tool_name.is_some_and(|name| lock(&shared.state).tools.iter().any(|t| t.name == name));
            let Some(name) = tool_name.filter(|_| known) else {
                let raw_name = params.and_then(|p| p.get("name")).map(display_param_name).unwrap_or_else(|| "undefined".into());
                send(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32602, "message": format!("Unknown tool: {raw_name}") } }));
                return true;
            };
            let arguments = params.and_then(|p| p.get("arguments")).filter(|v| v.is_object()).cloned().unwrap_or_else(|| json!({}));
            let session_id = params
                .and_then(|p| p.get("_meta"))
                .and_then(|m| m.get(SESSION_ID_META_KEY))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let call = McpCall {
                directory: options.directory.clone(),
                name: name.to_owned(),
                arguments,
                session_id,
                reply: Some((id, out.clone())),
            };
            if calls.send(call).is_err() {
                return false;
            }
            *in_flight += 1;
            waker();
        }
        _ => {
            send(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("Method not found: {method}") } }));
        }
    }
    true
}

fn display_param_name(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        other => other.to_string(),
    }
}

fn tool_descriptor_json(tool: &McpTool) -> Value {
    let mut schema = match &tool.input_schema {
        Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    schema.entry("type".to_owned()).or_insert_with(|| Value::String("object".into()));
    json!({
        "name": tool.name,
        "description": tool.description,
        "inputSchema": Value::Object(schema),
    })
}

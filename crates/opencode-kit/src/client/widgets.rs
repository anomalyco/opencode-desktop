//! The widgets plugin's RPC (`opencode.widget`, `Widget.Definition` in anomalyco/opencode
//! `packages/schema/src/widget.ts`): `POST /api/rpc/opencode.widget/<method>` in the session's
//! location (`packages/protocol/src/groups/rpc.ts`), `{ input }` in, `{ output }` back, a declared
//! failure as a 400 `RpcError` naming its `type`.

use std::fmt;
use std::time::Duration;

use serde_json::{Value, json};

use super::{Server, encode_component};
use crate::model::WidgetState;
use crate::wire_error::WireError;

const RPC_ID: &str = "opencode.widget";

/// Why a widget call failed.
#[derive(Clone, Debug, PartialEq)]
pub enum WidgetError {
    /// `closed`: it was already answered, here or on another client.
    Closed,
    /// `not_found`: no such widget, or no such widget template version.
    NotFound,
    /// `rate_limited`: over 10 sends a second.
    RateLimited,
    /// The plugin isn't there (the `widgets` flag is off) or another RPC failure.
    Rpc { kind: String, message: String },
    Wire(WireError),
}

impl fmt::Display for WidgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WidgetError::Closed => f.write_str("This widget was already answered."),
            WidgetError::NotFound => f.write_str("The widget wasn't found."),
            WidgetError::RateLimited => f.write_str("The widget sent too many messages at once."),
            WidgetError::Rpc { message, .. } => f.write_str(message),
            WidgetError::Wire(e) => write!(f, "{e}"),
        }
    }
}

/// A widget template version's program, as `program` serves it.
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetProgram {
    /// `prelude + "\n" + source`.
    pub program: String,
    /// The widget template's own source, the program's tail.
    pub source: String,
    /// `"step"` or `"live"`.
    pub mode: String,
    /// What a live widget template may use beyond rendering (`{ https?: { hosts } }`).
    pub capabilities: Value,
}

impl Server {
    fn widget_rpc(&self, directory: &str, method: &str, input: Value) -> Result<Value, WidgetError> {
        let path = format!("/api/rpc/{RPC_ID}/{}?location%5Bdirectory%5D={}", encode_component(method), encode_component(directory));
        let body = json!({ "input": input }).to_string().into_bytes();
        let answer = self.exchange("POST", &path, Some(&body), Some(Duration::from_secs(15)), &[]).map_err(WidgetError::Wire)?;
        if answer.status >= 300 {
            let v: Value = serde_json::from_slice(&answer.body).unwrap_or(Value::Null);
            let kind = v["type"].as_str().unwrap_or("").to_owned();
            return Err(match kind.as_str() {
                "closed" => WidgetError::Closed,
                "not_found" => WidgetError::NotFound,
                "rate_limited" => WidgetError::RateLimited,
                _ => WidgetError::Rpc { message: v["message"].as_str().map_or_else(|| format!("HTTP {} from {RPC_ID}/{method}", answer.status), str::to_owned), kind },
            });
        }
        let v: Value = serde_json::from_slice(&answer.body).unwrap_or(Value::Null);
        Ok(v["output"].clone())
    }

    /// Every widget of `session`, oldest first (`states`).
    pub fn widget_states(&self, directory: &str, session: &str) -> Result<Vec<WidgetState>, WidgetError> {
        let out = self.widget_rpc(directory, "states", json!({ "sessionID": session }))?;
        Ok(out.as_array().into_iter().flatten().filter_map(WidgetState::parse).collect())
    }

    /// Widget template `name` at `version` with its program (`program`): a version never changes, so
    /// callers cache it by `name@version` for good.
    pub fn widget_program(&self, directory: &str, name: &str, version: &str) -> Result<WidgetProgram, WidgetError> {
        let out = self.widget_rpc(directory, "program", json!({ "name": name, "version": version }))?;
        let program = out["program"].as_str().ok_or_else(|| WidgetError::Rpc { kind: "shape".into(), message: "no program in the reply".into() })?.to_owned();
        Ok(WidgetProgram { program, source: out["source"].as_str().unwrap_or("").to_owned(), mode: out["mode"].as_str().unwrap_or("step").to_owned(), capabilities: out["capabilities"].clone() })
    }

    /// Answers open widget `call` of `message` (`respond`): `response` is `{ value }` or
    /// `{ dismiss: true }`.
    pub fn widget_respond(&self, directory: &str, session: &str, message: &str, call: &str, response: Value) -> Result<(), WidgetError> {
        self.widget_rpc(directory, "respond", json!({ "sessionID": session, "messageID": message, "callID": call, "response": response })).map(|_| ())
    }

    /// Tells the agent that running widget `call` of `message` failed on this client (`report`):
    /// `error` is `{ phase, message, line?, column? }`.
    /// With `ask` (the user's "Ask agent to fix it"), the error goes to the agent in the widget's
    /// session even when that session didn't make the widget template.
    pub fn widget_report(&self, directory: &str, session: &str, message: &str, call: &str, error: Value, ask: bool) -> Result<(), WidgetError> {
        let mut input = json!({ "sessionID": session, "messageID": message, "callID": call, "error": error, "client": "rune" });
        if ask {
            input["ask"] = true.into();
        }
        self.widget_rpc(directory, "report", input).map(|_| ())
    }

    /// Whether `e` says the plugin isn't loaded in the location yet (`rpc.unavailable`).
    pub fn widget_unavailable(e: &WidgetError) -> bool {
        matches!(e, WidgetError::Rpc { kind, .. } if kind == "rpc.unavailable")
    }

    /// Sends a non-final message from open widget `call` of `message` to the agent (`send`).
    pub fn widget_send(&self, directory: &str, session: &str, message: &str, call: &str, value: Value, wake: bool) -> Result<(), WidgetError> {
        self.widget_rpc(directory, "send", json!({ "sessionID": session, "messageID": message, "callID": call, "value": value, "wake": wake })).map(|_| ())
    }
}

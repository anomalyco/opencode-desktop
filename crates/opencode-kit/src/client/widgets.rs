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
    /// `not_found`: no such widget, or no such template version.
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

/// A template version's program, as `program` serves it.
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetProgram {
    /// `prelude + "\n" + source`.
    pub program: String,
    /// `"step"` or `"live"`.
    pub mode: String,
    /// What a live template may use beyond rendering (`{ https?: { hosts } }`).
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

    /// Template `name` at `version` with its program (`program`): a version never changes, so
    /// callers cache it by `name@version` for good.
    pub fn widget_program(&self, directory: &str, name: &str, version: &str) -> Result<WidgetProgram, WidgetError> {
        let out = self.widget_rpc(directory, "program", json!({ "name": name, "version": version }))?;
        let program = out["program"].as_str().ok_or_else(|| WidgetError::Rpc { kind: "shape".into(), message: "no program in the reply".into() })?.to_owned();
        Ok(WidgetProgram { program, mode: out["mode"].as_str().unwrap_or("step").to_owned(), capabilities: out["capabilities"].clone() })
    }

    /// Answers open widget `call` of `message` (`respond`): `response` is `{ value }` or
    /// `{ dismiss: true }`.
    pub fn widget_respond(&self, directory: &str, session: &str, message: &str, call: &str, response: Value) -> Result<(), WidgetError> {
        self.widget_rpc(directory, "respond", json!({ "sessionID": session, "messageID": message, "callID": call, "response": response })).map(|_| ())
    }

    /// Sends a non-final message from open widget `call` of `message` to the agent (`send`).
    pub fn widget_send(&self, directory: &str, session: &str, message: &str, call: &str, value: Value, wake: bool) -> Result<(), WidgetError> {
        self.widget_rpc(directory, "send", json!({ "sessionID": session, "messageID": message, "callID": call, "value": value, "wake": wake })).map(|_| ())
    }
}

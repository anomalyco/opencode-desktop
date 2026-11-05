//! Read the inbox BEFORE the exact message: delivery can move it between the two reads.
//! A page can omit old prompts, and a timeout/5xx is never proof of absence.
use opencode_kit::model::{Message, parse_message};
use serde_json::Value;

pub(super) enum Receipt { Admitted, Delivered(Message), Missing }

pub(super) fn read(fetch: &dyn Fn(&str) -> Result<Value, String>, session: &str, id: &str) -> Result<Receipt, String> {
    let inbox = fetch(&format!("/api/session/{session}/inbox"))?;
    let items = inbox["data"].as_array().or_else(|| inbox.as_array()).ok_or("Invalid inbox response")?;
    if items.iter().any(|item| item["id"] == id) { return Ok(Receipt::Admitted); }
    match fetch(&format!("/api/session/{session}/message/{id}")) {
        Ok(value) => parse_message(&value["data"]).filter(|m| m.id() == id).map(Receipt::Delivered).ok_or_else(|| "Invalid message receipt".into()),
        Err(error) if error.ends_with("http status: 404") => Ok(Receipt::Missing),
        Err(error) => Err(error),
    }
}

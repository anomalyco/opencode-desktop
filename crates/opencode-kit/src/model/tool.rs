//! What a tool call keeps in its `metadata` besides the server's own: its error message, when it
//! began running, and its text result, kept across the server's later metadata updates.

use serde_json::Value;

use super::ToolStatus;

/// Where a failed tool call's error message is kept in its `metadata`.
const TOOL_ERROR: &str = "opencode-kit.error";

/// Where a tool call's start, in Unix milliseconds, is kept in its `metadata`.
const TOOL_RAN: &str = "opencode-kit.ran";

/// Where a finished tool call's text result is kept in its `metadata`.
const TOOL_OUTPUT: &str = "opencode-kit.output";

/// A failed tool call's error message, from its `metadata`.
pub fn tool_error(metadata: &Value) -> Option<&str> {
    metadata[TOOL_ERROR].as_str()
}

/// Keeps `error`'s message (an `{ message }` object or a string) in `metadata`.
pub(super) fn keep_tool_error(metadata: &mut Value, error: &Value) {
    let Some(message) = error["message"].as_str().or(error.as_str()) else { return };
    object(metadata)[TOOL_ERROR] = message.into();
}

/// When a tool call began running (`time.ran`, else `time.created`), once a read has said.
pub fn tool_ran(metadata: &Value) -> Option<i64> {
    metadata[TOOL_RAN].as_i64()
}

pub(super) fn keep_tool_ran(metadata: &mut Value, time: &Value) {
    let Some(ran) = time["ran"].as_i64().or(time["created"].as_i64()) else { return };
    object(metadata)[TOOL_RAN] = ran.into();
}

/// A tool call's text result: what a running call has streamed so far (`metadata.output`), or
/// the text items of what a finished one returned, else what it streamed before it ended.
pub fn tool_output<'a>(status: &ToolStatus, metadata: &'a Value) -> Option<&'a str> {
    match status {
        ToolStatus::Running => metadata["output"].as_str(),
        _ => metadata[TOOL_OUTPUT].as_str().or_else(|| metadata["output"].as_str()),
    }
}

/// Keeps the text items of a tool result's `content` in `metadata`.
pub(super) fn keep_tool_output(metadata: &mut Value, content: &Value) {
    let texts: Vec<&str> = content.as_array().into_iter().flatten().filter(|c| c["type"] == "text").filter_map(|c| c["text"].as_str()).collect();
    if texts.is_empty() {
        return;
    }
    object(metadata)[TOOL_OUTPUT] = texts.join("\n").into();
}

/// Swaps in the server's newer `metadata`, keeping what this crate keeps there: when the call
/// began running, and its result and error, which a late progress event mustn't wipe.
pub(super) fn replace_tool_metadata(metadata: &mut Value, new: &Value) {
    let kept: Vec<(&str, Value)> = [TOOL_RAN, TOOL_OUTPUT, TOOL_ERROR].into_iter().map(|k| (k, metadata[k].clone())).filter(|(_, v)| !v.is_null()).collect();
    *metadata = new.clone();
    if kept.is_empty() {
        return;
    }
    let m = object(metadata);
    for (k, v) in kept {
        m[k] = v;
    }
}

/// `metadata` as an object, made one when it isn't.
fn object(metadata: &mut Value) -> &mut Value {
    if !metadata.is_object() {
        *metadata = Value::Object(Default::default());
    }
    metadata
}

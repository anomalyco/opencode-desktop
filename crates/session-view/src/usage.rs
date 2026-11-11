//! A session's usage as Desktop reads it off its messages: the step whose tokens fill the
//! context window. The Context plugin draws it (the header's ring and the Context tab), reading
//! it through the plugin host; the cost is the server's (`App::session_cost`).

use opencode_kit::model::{Assistant, CompactionStatus, Message, Tokens, Transcript};

/// Desktop's `lastAssistantWithUsage` (`client/src/solid/usage.ts:8-21`) over the messages Desktop
/// holds (`held`): the last step that reported its tokens, after the last finished compaction and
/// before a staged revert at `boundary`. A boundary Desktop doesn't hold finds nothing, as there.
pub fn last_with_usage<'a>(t: &'a Transcript, boundary: Option<&str>, held: impl Fn(&str) -> bool) -> Option<(&'a Assistant, &'a Tokens)> {
    let end = match boundary {
        Some(b) => t.messages.iter().position(|m| m.id() == b && held(b))?,
        None => t.messages.len(),
    };
    let after = t.messages[..end].iter().rposition(|m| matches!(m, Message::Compaction(c) if c.status == CompactionStatus::Completed && held(&c.id))).map_or(0, |i| i + 1);
    t.messages[after..end].iter().rev().find_map(|m| match m {
        Message::Assistant(a) if held(&a.id) => a.tokens.as_ref().map(|tokens| (a, tokens)),
        _ => None,
    })
}

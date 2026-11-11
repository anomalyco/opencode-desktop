//! A session's usage as Desktop reads it off its messages: the step whose tokens fill the
//! context window. The Context plugin draws it (the header's ring and the Context tab), reading
//! it through the plugin host; the cost is the server's (`App::session_cost`).

use opencode_kit::model::{Assistant, CompactionStatus, Message, Tokens, Transcript};

/// Desktop's `lastAssistantWithUsage` (`client/src/solid/usage.ts`): the last step that reported
/// its tokens, after the last finished compaction and before a staged revert at `boundary`.
pub fn last_with_usage<'a>(t: &'a Transcript, boundary: Option<&str>) -> Option<(&'a Assistant, &'a Tokens)> {
    let end = match boundary {
        Some(b) => t.messages.iter().position(|m| m.id() == b)?,
        None => t.messages.len(),
    };
    let after = t.messages[..end].iter().rposition(|m| matches!(m, Message::Compaction(c) if c.status == CompactionStatus::Completed)).map_or(0, |i| i + 1);
    t.messages[after..end].iter().rev().find_map(|m| match m {
        Message::Assistant(a) => a.tokens.as_ref().map(|tokens| (a, tokens)),
        _ => None,
    })
}

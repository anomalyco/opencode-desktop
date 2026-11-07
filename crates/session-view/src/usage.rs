//! A session's usage as Desktop reads it off its messages: the step whose tokens fill the
//! context window, and what the steps cost. The Context plugin draws them (the header's ring
//! and the Context tab), reading them through the plugin host.

use opencode_kit::model::{Assistant, CompactionStatus, Message, Transcript};

/// Desktop's `lastAssistantWithUsage`: the last step that reported its tokens, after the last
/// finished compaction and before a staged revert at `boundary`.
pub fn last_with_usage<'a>(t: &'a Transcript, boundary: Option<&str>) -> Option<&'a Assistant> {
    let end = match boundary {
        Some(b) => t.messages.iter().position(|m| m.id() == b)?,
        None => t.messages.len(),
    };
    let after = t.messages[..end].iter().rposition(|m| matches!(m, Message::Compaction(c) if c.status == CompactionStatus::Completed)).map_or(0, |i| i + 1);
    t.messages[after..end].iter().rev().find_map(|m| match m {
        // A step reports its tokens as it finishes.
        Message::Assistant(a) if a.completed.is_some() || a.tokens != Default::default() => Some(a),
        _ => None,
    })
}

/// What the transcript's steps cost, summed.
pub fn spent(t: &Transcript) -> f64 {
    t.messages.iter().map(|m| if let Message::Assistant(a) = m { a.cost } else { 0.0 }).sum()
}

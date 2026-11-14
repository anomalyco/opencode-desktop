//! A session's usage as Desktop reads it off its messages: the step whose tokens fill the
//! context window. The Context plugin draws it (the header's ring and the Context tab), reading
//! it through the plugin host; the cost is the server's (`App::session_cost`).

use opencode_kit::model::{Assistant, CompactionStatus, Message, Tokens, Transcript};

/// Desktop's `lastAssistantWithUsage` (`client/src/solid/usage.ts:8-21`) over the messages Desktop
/// holds (`held`): the last step that reported its tokens, after the last finished compaction and
/// before a staged revert at `boundary`. A boundary Desktop doesn't hold finds nothing, as there.
pub fn last_with_usage<'a>(t: &'a Transcript, boundary: Option<&str>, held: impl Fn(&str) -> bool) -> Option<(&'a Assistant, &'a Tokens)> {
    at(t, last_with_usage_at(t, boundary, held)?)
}

/// The step [`last_with_usage`] finds, by its place in `t`'s messages.
pub fn at(t: &Transcript, i: usize) -> Option<(&Assistant, &Tokens)> {
    match t.messages.get(i)? {
        Message::Assistant(a) => a.tokens.as_ref().map(|tokens| (a, tokens)),
        _ => None,
    }
}

/// [`last_with_usage`]'s place in `t`'s messages.
pub fn last_with_usage_at(t: &Transcript, boundary: Option<&str>, held: impl Fn(&str) -> bool) -> Option<usize> {
    let end = match boundary {
        Some(b) => t.messages.iter().position(|m| m.id() == b && held(b))?,
        None => t.messages.len(),
    };
    // One pass back from the end: the first step with usage, unless a finished compaction comes
    // first. A long session's newest steps carry usage, so it reads a few messages.
    for (i, m) in t.messages[..end].iter().enumerate().rev() {
        match m {
            Message::Compaction(c) if c.status == CompactionStatus::Completed && held(&c.id) => return None,
            // A step without usage is passed over before asking whether it's held: the host's
            // answer costs a lookup, and a long session's steps mostly carry none.
            Message::Assistant(a) if a.tokens.is_some() && held(&a.id) => return Some(i),
            _ => {}
        }
    }
    None
}

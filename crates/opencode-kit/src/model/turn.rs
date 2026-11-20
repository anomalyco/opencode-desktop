//! A turn's measurements, as OpenCode's `SessionTurn` takes them
//! (`packages/util/src/session-turn.ts`), the TUI's and Desktop's reply footers read them.

use super::{Message, Notice, NoticeKind};

/// Output and reasoning tokens per second while the provider streamed them, over the steps of
/// the turn up to message `index`; `None` until every one of those steps has streamed
/// (`SessionTurn.tokensPerSecond`, `session-turn.ts:43-56`).
///
/// Each step counts from its first streamed token (`time.firstToken`, else its start for steps
/// recorded before that existed) to the end of its response body (`time.streamed`), so neither
/// the wait for the first token nor tool time is divided in. Tokens and time are summed before
/// dividing, so each step weighs by how long it streamed.
pub fn tokens_per_second(messages: &[Message], index: usize) -> Option<f64> {
    let end = (index + 1).min(messages.len());
    let start = input_index(messages, end, legacy(messages)).map_or(0, |i| i + 1);
    let mut steps = 0;
    let mut output = 0;
    let mut total = 0;
    for m in &messages[start.min(end)..end] {
        let Message::Assistant(a) = m else { continue };
        steps += 1;
        total += (a.streamed? - a.first_token.unwrap_or(a.created)).max(0);
        output += a.tokens.as_ref().map_or(0, |t| t.output + t.reasoning);
    }
    if steps == 0 || output == 0 || total <= 0 {
        return None;
    }
    Some(output as f64 / (total as f64 / 1000.0))
}

/// Whether the session has no idle markers, so a turn starts at its nearest prompt
/// (`SessionTurn.legacy`).
fn legacy(messages: &[Message]) -> bool {
    !messages.iter().any(idle)
}

/// Where the turn ending before `end` starts (`inputIndex`, `session-turn.ts:69-82`): the nearest
/// input in legacy sessions, otherwise the first input after the previous idle marker.
///
/// The transcript drops synthetic messages Desktop shows nothing for (a quiet shell's report),
/// so a turn one of those started finds no input after its idle marker: the marker bounds it
/// instead, where the dropped message sat, rather than reaching back over the whole session.
fn input_index(messages: &[Message], end: usize, legacy: bool) -> Option<usize> {
    let mut found = None;
    for i in (0..end).rev() {
        let m = &messages[i];
        if idle(m) {
            return found.or(Some(i));
        }
        if !input(m) {
            continue;
        }
        if legacy {
            return Some(i);
        }
        found = Some(i);
    }
    found
}

fn idle(m: &Message) -> bool {
    matches!(m, Message::Notice(Notice { kind: NoticeKind::Idle { .. }, .. }))
}

/// A `user` or `synthetic` message: a prompt, or what a run started from without one (a
/// restart, a background shell or subagent's report, a job's).
fn input(m: &Message) -> bool {
    matches!(m, Message::User { .. } | Message::Notice(Notice { kind: NoticeKind::Restart | NoticeKind::Finished(_) | NoticeKind::Job { .. }, .. }))
}

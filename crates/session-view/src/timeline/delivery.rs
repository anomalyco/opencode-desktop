//! Between assistant steps the server promotes steers and background job reports together.
//! A prompt does not own a report just because it was admitted one slot earlier.

use super::*;
use crate::detail::{Detail, Placement, notice_placement};

/// Consecutive prompts, including only intervening notices we can positively tie to this
/// turn's calls. Never cross an idle marker, control message or new assistant step.
pub(super) fn end(steps: &[Entry], messages: &[Message], first: usize) -> usize {
    let mut end = first + 1;
    while let Some(m) = messages.get(end) {
        match m {
            Message::User { .. } => {}
            Message::Notice(model::Notice { kind: model::NoticeKind::Job { id, .. }, .. })
                if steps.iter().rev().take_while(|e| !matches!(e, Entry::Skip)).any(|e| matches!(e,
                    Entry::Step(a) if a.parts.iter().any(|p| matches!(p,
                        Part::Tool { metadata, .. } if metadata["jobID"].as_str() == Some(id.as_str()))))) => {}
            _ => break,
        }
        end += 1;
    }
    end
}

pub(super) fn rows(out: &mut Vec<RowSpec>, messages: &[Message], span: std::ops::Range<usize>,
    older: bool, detail: &Detail, fold: &impl Fn(&str) -> Fold, pending: Option<&InboxItem>) {
    for i in span.clone() {
        if let Message::Notice(n) = &messages[i] && notice_placement(n, detail) == Placement::Separate {
            out.push(RowSpec { key: format!("n-{}", n.id), content: RowContent::Notice(i, Run::default(), rhythm::NOTICE_PT), streaming: false });
        }
    }
    let at_end = span.end == messages.len();
    let mut prompts = span.filter(|i| matches!(messages[*i], Message::User { .. })).peekable();
    let mut prev = None;
    while let Some(i) = prompts.next() {
        let m = &messages[i];
        let Message::User { id, text } = m else { continue };
        let above = prev.is_some_and(|p| joins(p, m));
        let below = prompts.peek().map_or_else(|| at_end && pending.is_some_and(|n| prompts_join(text, n)), |n| joins(m, &messages[*n]));
        let tight = prompt_images(out, id, older);
        if !text.trim().is_empty() || !model::user_comments(id).is_empty() {
            out.push(RowSpec { key: format!("u-{id}"), content: RowContent::User { message: i, tight, above, below, reply: 0.0, fold: fold(id) }, streaming: false });
        }
        prev = Some(m);
    }
}

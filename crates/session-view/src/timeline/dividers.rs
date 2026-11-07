//! Dividers back to back sit as one tight group, and a group sits the turn's usual gap from the
//! rows around it (p265): each divider row learns what's above and below it.

use opencode_kit::model::{Message, Transcript};

use super::{RowContent, RowSpec};
use crate::TimelineHost;
use crate::compaction::{Beside, Run};

/// Notes on each divider row in `out` what sits beside it.
pub(super) fn stack(t: &Transcript, out: &mut [RowSpec]) {
    let shown: Vec<usize> = (0..out.len()).filter(|&i| !matches!(out[i].content, RowContent::Lane(_))).collect();
    let kinds: Vec<Kind> = shown.iter().map(|&i| kind(t, &out[i].content)).collect();
    for (k, &i) in shown.iter().enumerate() {
        if kinds[k] != Kind::Divider {
            continue;
        }
        let above = match k.checked_sub(1).map(|j| kinds[j]) {
            Some(Kind::Divider) => Beside::Divider,
            Some(Kind::Prompt) => Beside::Spaced,
            _ => Beside::Other,
        };
        let below = match kinds.get(k + 1) {
            Some(Kind::Divider) => Beside::Divider,
            Some(Kind::Prompt | Kind::Roomy) | None => Beside::Spaced,
            Some(Kind::Other) => Beside::Other,
        };
        let run = Run { above, below };
        match &mut out[i].content {
            RowContent::Notice(_, r) | RowContent::Compaction { run: r, .. } | RowContent::CompactionQueued(r) | RowContent::TurnDivider(r) => *r = run,
            _ => {}
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Divider,
    /// A prompt: room over it, and its copy line under it.
    Prompt,
    /// Room over it only: a prompt's pictures, a note, Working.
    Roomy,
    Other,
}

fn kind(t: &Transcript, c: &RowContent) -> Kind {
    match c {
        RowContent::Notice(i, _) => match &t.messages[*i] {
            Message::Notice(n) if crate::turn_errors::is_divider(n) => Kind::Divider,
            _ => Kind::Other,
        },
        RowContent::Compaction { message, piece, .. } => match &t.messages[*message] {
            Message::Compaction(_) if crate::compaction::is_divider(*piece) => Kind::Divider,
            _ => Kind::Other,
        },
        RowContent::CompactionQueued(_) | RowContent::TurnDivider(_) => Kind::Divider,
        RowContent::User { .. } | RowContent::Pending { .. } => Kind::Prompt,
        RowContent::UserImages { .. } | RowContent::Note(_) | RowContent::Actions { .. } => Kind::Roomy,
        _ => Kind::Other,
    }
}

/// Each divider row's key and what sits beside it, top to bottom.
pub fn divider_runs<S: TimelineHost>(s: &S, t: &Transcript) -> Vec<(String, Run)> {
    super::rows(s, t).into_iter().filter(|r| kind(t, &r.content) == Kind::Divider).map(|r| {
        let run = match r.content {
            RowContent::Notice(_, run) | RowContent::Compaction { run, .. } | RowContent::CompactionQueued(run) | RowContent::TurnDivider(run) => run,
            _ => unreachable!(),
        };
        (r.key, run)
    }).collect()
}

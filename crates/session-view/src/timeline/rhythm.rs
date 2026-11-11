//! The timeline's vertical rhythm: the room between two rows, measured between what a reader
//! sees (a line of text, a card's edge), never between the rows’ boxes. Four gaps, by the
//! kinds of the two neighbours:
//!
//! | above, below                                              | gap        |
//! |-----------------------------------------------------------|------------|
//! | prose, prose (a reply's words)                            | `PROSE` 12 |
//! | quiet, quiet (notices, Used N, thoughts, calls, subagents) | `TIGHT` 8  |
//! | any other pair: a card (code, a question, an error), prose beside quiet rows, a prompt above | `BLOCK` 16 |
//! | a reply's last row, the next prompt                       | `TURN` 48  |
//!
//! So prose runs at its own paragraph gap, quiet rows hold together as one group with even room
//! above and below it, a card has the same room on both sides (Markdown's code blocks too:
//! `style::markdown_style`), and the next prompt stands `TURN` under a reply, more than the
//! room between a prompt and its own reply, so a prompt reads with the work under it.
//!
//! Dividers, Changed files, a reply's footer and a tool's pictures keep their own room; a row
//! under one of them keeps the room it was built with.

use opencode_kit::model::{Message, Part, Transcript};

use super::{RowContent, RowSpec};
use crate::turns::{self, Block};

/// Between two pieces of prose: Markdown's own paragraph gap.
pub const PROSE: f32 = 12.0;
/// Between quiet rows, which read as one group.
pub const TIGHT: f32 = 8.0;
/// Around a group of quiet rows, a card, and under a prompt.
pub const BLOCK: f32 = 16.0;
/// Over a prompt under a reply: past a prompt's 28 meta room and `BLOCK` under it (44).
pub const TURN: f32 = 48.0;
/// A notice's room over it where nothing here spaces it (under a divider or a footer).
pub const NOTICE_PT: f32 = 12.0;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Prose,
    Quiet,
    Card,
    Prompt,
    /// Spaces itself: nothing here changes the room over it, or under it.
    Other,
}

/// A row's kind at its top and bottom edges, and how far inside its box what shows starts and
/// ends: a 28 high trigger's line sits 4 in, a 20 high notice's 2.
#[derive(Clone, Copy)]
struct Edges {
    top: (Kind, f32),
    bottom: (Kind, f32),
}

const fn both(kind: Kind, inset: f32) -> Edges {
    Edges { top: (kind, inset), bottom: (kind, inset) }
}

/// Words, or a card at an edge where the words open or close with a fenced block.
fn prose(body: &str) -> Edges {
    let edge = |fenced: bool| if fenced { (Kind::Card, 0.0) } else { (Kind::Prose, 0.0) };
    Edges { top: edge(body.trim_start().starts_with("```")), bottom: edge(body.trim_end().ends_with("```")) }
}

fn edges(t: &Transcript, detail: &crate::detail::Detail, row: &RowContent) -> Edges {
    match row {
        RowContent::User { .. } | RowContent::Pending { .. } => both(Kind::Prompt, 0.0),
        RowContent::Notice(i, ..) => match &t.messages[*i] {
            Message::Notice(n) if crate::turn_errors::is_divider(n) => both(Kind::Other, 0.0),
            _ => both(Kind::Quiet, 2.0),
        },
        RowContent::PendingNotice(..) => both(Kind::Quiet, 2.0),
        RowContent::Error { .. } | RowContent::Retry { .. } => both(Kind::Card, 0.0),
        RowContent::TurnText { message, part, .. } => match &t.messages[*message] {
            Message::Assistant(a) => match a.parts.get(*part) {
                Some(Part::Text { text }) => prose(text),
                _ => both(Kind::Prose, 0.0),
            },
            _ => both(Kind::Prose, 0.0),
        },
        RowContent::Turn { start, end, block, live, .. } => {
            let blocks = turns::fold_with(&super::entries(&t.messages[*start..*end], detail), *live, detail);
            match blocks.get(*block) {
                Some(Block::Text { body, .. }) => prose(body),
                Some(Block::Group { .. } | Block::Thought { .. } | Block::Calls { .. }) => both(Kind::Quiet, 4.0),
                Some(Block::Agent { .. }) => both(Kind::Quiet, 0.0),
                Some(Block::Question { .. }) => both(Kind::Card, 0.0),
                None => both(Kind::Other, 0.0),
            }
        }
        _ => both(Kind::Other, 0.0),
    }
}

fn gap(above: Kind, below: Kind) -> f32 {
    match (above, below) {
        (Kind::Prose, Kind::Prose) => PROSE,
        (Kind::Quiet, Kind::Quiet) => TIGHT,
        _ => BLOCK,
    }
}

/// Sets the room over each row in `out` from the row above it (see the table above).
pub(super) fn space(t: &Transcript, detail: &crate::detail::Detail, out: &mut [RowSpec]) {
    let mut above: Option<Edges> = None;
    for row in out.iter_mut() {
        if matches!(row.content, RowContent::Lane(_)) {
            continue;
        }
        let here = edges(t, detail, &row.content);
        if let Some(prev) = above.filter(|p| p.bottom.0 != Kind::Other) {
            let room = (gap(prev.bottom.0, here.top.0) - prev.bottom.1 - here.top.1).max(0.0);
            match &mut row.content {
                RowContent::User { reply, .. } | RowContent::Pending { reply, .. } => *reply = prev.bottom.0 != Kind::Prompt,
                RowContent::Notice(_, _, pt) | RowContent::PendingNotice(_, pt) if here.top.0 != Kind::Other => *pt = room,
                RowContent::Error { pt, .. } | RowContent::Retry { pt, .. } | RowContent::TurnText { pt, .. } | RowContent::Turn { pt, .. } => *pt = room,
                _ => {}
            }
        }
        above = Some(here);
    }
}

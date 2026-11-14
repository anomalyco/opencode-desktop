//! The timeline's vertical rhythm: the room between two rows, measured between what a reader
//! sees (a line of text, a card's edge), never between the rows’ boxes. Four gaps, by the
//! kinds of the two neighbours:
//!
//! | above, below                                              | gap        |
//! |-----------------------------------------------------------|------------|
//! | prose, prose (a reply's words)                            | `PROSE` 12 |
//! | quiet, quiet (notices, Used N, thoughts, calls, subagents) | `TIGHT` 8  |
//! | a prompt (its 28 meta room included), its reply's first row | `REPLY` 24 |
//! | a prompt, a tool row (Used N, a thought, calls) opening its reply | 0, to the row's box |
//! | a tool row beside anything but a quiet row                | `BLOCK` 16, box to box |
//! | any other pair: a card (code, a question, an error), prose beside quiet rows | `BLOCK` 16 |
//! | a reply's last row, the next prompt                       | `TURN` 44  |
//!
//! So prose runs at its own paragraph gap, quiet rows hold together as one group with even room
//! above and below it, a card has the same room on both sides (inside a reply, Markdown's code
//! blocks keep Desktop's own 12 over and 24 under), and the next prompt stands `TURN` under a reply,
//! as far as Desktop's.
//!
//! Dividers, Changed files, a reply's footer and a tool's pictures keep their own room; a row
//! under one of them keeps the room it was built with. A finished turn a later prompt follows
//! holds its footer under its words (`parts::turn_footer`, shown under the pointer): its 28 count
//! toward the room under them, so what a reader sees keeps the same gaps.

use opencode_kit::model::{Message, Part, Transcript};

use super::{RowContent, RowSpec};
use crate::turns::{self, Block};

/// Between two pieces of prose: Markdown's own paragraph gap.
pub const PROSE: f32 = 12.0;
/// Between quiet rows, which read as one group.
pub const TIGHT: f32 = 8.0;
/// Around a group of quiet rows and a card.
pub const BLOCK: f32 = 16.0;
/// Under a prompt's meta row, over its reply: Desktop's 24 from the user message's box to the
/// reply's first block (`proof/layout-cards/blocks/turn-1`: 129 to 153), 52 from the bubble.
pub const REPLY: f32 = 24.0;
/// Over a prompt under a reply: Desktop's room from a reply's last line box to the next prompt's
/// bubble (43.78 on Windows, `proof/layout-cards/blocks/turn-1`; 52.2 to its text on the Mac).
pub const TURN: f32 = 44.0;
/// A notice's room over it where nothing here spaces it (under a divider or a footer).
pub const NOTICE_PT: f32 = 12.0;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Prose,
    Quiet,
    /// A collapsed tool group, a thought or a run of calls: a 28 high trigger, its line 4 in.
    Tool,
    Card,
    Prompt,
    /// Spaces itself: nothing here changes the room over it, or under it.
    Other,
}

/// A row's kind at its top and bottom edges, and how far inside its box what shows starts and
/// ends: a 28 high trigger's line sits 4 in, a 20 high notice's 2. `footer`: the room at its
/// foot a finished turn's footer holds, which shows only under the pointer (`parts::turn_footer`).
#[derive(Clone, Copy)]
struct Edges {
    top: (Kind, f32),
    bottom: (Kind, f32),
    footer: f32,
}

const fn both(kind: Kind, inset: f32) -> Edges {
    Edges { top: (kind, inset), bottom: (kind, inset), footer: 0.0 }
}

/// Words, or a card at an edge where the words open or close with a fenced block; `footer`,
/// they end a turn a later prompt follows, under its footer.
fn prose(body: &str, footer: bool) -> Edges {
    let edge = |fenced: bool| if fenced { (Kind::Card, 0.0) } else { (Kind::Prose, 0.0) };
    let footer = if footer { crate::working::COPY_AT.1 + crate::working::COPY } else { 0.0 };
    Edges { top: edge(body.trim_start().starts_with("```")), bottom: edge(body.trim_end().ends_with("```")), footer }
}

/// The last turn [`edges`] folded: a turn's rows sit together, so each turn folds once.
type Folded<'a> = Option<((usize, usize, bool), Vec<Block<'a>>)>;

fn edges<'a>(t: &'a Transcript, detail: &crate::detail::Detail, row: &RowContent, folded: &mut Folded<'a>) -> Edges {
    match row {
        RowContent::User { .. } | RowContent::Pending { .. } => both(Kind::Prompt, 0.0),
        RowContent::Notice(i, ..) => match &t.messages[*i] {
            Message::Notice(n) if crate::turn_errors::is_divider(n) => both(Kind::Other, 0.0),
            _ => both(Kind::Quiet, 2.0),
        },
        RowContent::PendingNotice(..) => both(Kind::Quiet, 2.0),
        RowContent::Error { .. } | RowContent::Retry { .. } => both(Kind::Card, 0.0),
        RowContent::TurnText { message, part, live, .. } => match &t.messages[*message] {
            Message::Assistant(a) => match a.parts.get(*part) {
                Some(Part::Text { text }) => prose(text, !live && super::turn_footer_text(t, *message, *part)),
                _ => both(Kind::Prose, 0.0),
            },
            _ => both(Kind::Prose, 0.0),
        },
        RowContent::Turn { start, end, block, live, .. } => {
            let turn = (*start, *end, *live);
            if folded.as_ref().is_none_or(|(at, _)| *at != turn) {
                *folded = Some((turn, turns::fold_with(&super::entries(&t.messages[*start..*end], detail), *live, detail)));
            }
            match folded.as_ref().and_then(|(_, blocks)| blocks.get(*block)) {
                Some(Block::Text { body, msg, part, live, .. }) => prose(body, !live && super::turn_footer_text(t, start + msg, *part)),
                Some(Block::Group { .. } | Block::Thought { .. } | Block::Calls { .. }) => both(Kind::Tool, TOOL_INSET),
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
        (Kind::Quiet | Kind::Tool, Kind::Quiet | Kind::Tool) => TIGHT,
        // Desktop's tool row opens a reply at the top of its turn (only a text part takes the 24
        // over it, `text-part` `margin-top`), and every later row stands 16 under the one above
        // (`pt-4`), box to box: measured from the trigger's box, not its line.
        (Kind::Prompt, Kind::Tool) => TOOL_INSET,
        (Kind::Prompt, _) => REPLY,
        (_, Kind::Tool) | (Kind::Tool, _) => BLOCK + TOOL_INSET,
        _ => BLOCK,
    }
}

/// How far in a tool trigger's line sits (its `Edges` inset).
const TOOL_INSET: f32 = 4.0;

/// Sets the room over each row in `out` from the row above it (see the table above).
pub(super) fn space(t: &Transcript, detail: &crate::detail::Detail, out: &mut [RowSpec]) {
    space_at(t, detail, out, 0..out.len());
}

/// [`space`] for rows `at` alone, from the row above them: where two spans of a projection meet
/// (`projection`).
pub(super) fn space_at(t: &Transcript, detail: &crate::detail::Detail, out: &mut [RowSpec], at: std::ops::Range<usize>) {
    let mut folded: Folded = None;
    let mut above: Option<Edges> = out[..at.start.min(out.len())].iter().rev()
        .find(|r| !matches!(r.content, RowContent::Lane(_)))
        .map(|r| edges(t, detail, &r.content, &mut folded));
    let end = at.end.min(out.len());
    for row in out[at.start.min(end)..end].iter_mut() {
        if matches!(row.content, RowContent::Lane(_)) {
            continue;
        }
        let here = edges(t, detail, &row.content, &mut folded);
        if let Some(prev) = above.filter(|p| p.bottom.0 != Kind::Other) {
            // A footer's room counts toward the gap under the words it closes.
            let room = (gap(prev.bottom.0, here.top.0) - prev.bottom.1 - prev.footer - here.top.1).max(0.0);
            match &mut row.content {
                RowContent::User { reply, .. } | RowContent::Pending { reply, .. } => *reply = if prev.bottom.0 != Kind::Prompt { (TURN - prev.footer).max(0.0) } else { 0.0 },
                RowContent::Notice(_, _, pt) | RowContent::PendingNotice(_, pt) if here.top.0 != Kind::Other => *pt = room,
                RowContent::Error { pt, .. } | RowContent::Retry { pt, .. } | RowContent::TurnText { pt, .. } | RowContent::Turn { pt, .. } => *pt = room,
                _ => {}
            }
        }
        above = Some(here);
    }
}
//! The timeline's vertical rhythm, as Desktop's New timeline spaces rows
//! (`timeline-next/spacing.ts`, `timeline-next.css`): by who did what. Each thing the user did
//! (a prompt, a command) stands apart, the agent's reply reads as one stretch, and what happened
//! to the session (a switch, a notice) sits together. The room is measured between what a reader
//! sees (a line of text, a card's edge), never between the rows' boxes:
//!
//! | above, below                                                    | gap     |
//! |-----------------------------------------------------------------|---------|
//! | a different block, or either is the user's, or work reporting back | `L` 32 |
//! | the agent's words beside its activity, in a block               | `M` 20  |
//! | two of the same kind (words, or lines of activity), in a block  | `S` 16  |
//!
//! Dividers, compactions, Changed files, a reply's footer and a tool's pictures keep their own
//! room; a row under one of them keeps the room it was built with. A finished turn a later
//! prompt follows holds its footer under its words (`parts::turn_footer`, shown under the
//! pointer): its room counts toward the gap under them.

use opencode_kit::model::{Message, Part, Transcript};

use super::{RowContent, RowSpec};
use crate::turns::{self, Block};

/// The small gap: between rows of activity in a reply, or a run of session events
/// (`--timeline-gap-s`).
pub const S: f32 = 16.0;
/// The medium gap: between the agent's words and its activity (`--timeline-gap-m`).
pub const M: f32 = 20.0;
/// The large gap: around each thing the user did, and wherever a block starts
/// (`--timeline-gap-l`).
pub const L: f32 = 32.0;
/// A notice's room over it where nothing here spaces it (under a divider or a footer).
pub const NOTICE_PT: f32 = 12.0;

/// Who a row is (`TimelineBlock`); `None` spaces itself.
#[derive(Clone, Copy, PartialEq)]
enum Who {
    User,
    Agent,
    Event,
}

/// What a row shows (`TimelineItem`): the agent's words, or a line of activity.
#[derive(Clone, Copy, PartialEq)]
enum Item {
    Prose,
    Line,
}

/// A row's place in the rhythm: who and what, how far inside its box what shows starts and
/// ends (a 28 pt trigger's 16 pt line sits 6 in; Markdown's trimmed half-leading, prose_insets), whether it
/// is work reporting back, which always starts a block, and the room at its foot a finished
/// turn's footer holds (`parts::turn_footer`).
#[derive(Clone, Copy)]
struct Edges {
    who: Who,
    item: Item,
    top: f32,
    bottom: f32,
    reports: bool,
    footer: f32,
}

const fn edges(who: Who, item: Item, inset: f32) -> Edges {
    Edges { who, item, top: inset, bottom: inset, reports: false, footer: 0.0 }
}

/// A 28 pt trigger around a 16 pt line.
const LINE_INSET: f32 = 6.0;
/// Half-leading over and under `md` in points, as Desktop trims it (`timeline-next.css:449-452`,
/// `text-box: trim-both text`): Inter's ascent (1984/2048 em) and descent (494/2048) rounded to
/// device pixels, the leading floored above them and the rest below, as Blink splits it and Rune
/// lays the line out.
fn line_insets(md: rune::TextStyle) -> (f32, f32) {
    let s = crate::scroller::device_scale();
    let line = (md.line_px() * s * 64.0).round() / 64.0;
    let text = (md.size * s * 1984.0 / 2048.0).round() + (md.size * s * 494.0 / 2048.0).round();
    let over = ((line - text) / 2.0).floor();
    (over / s, (line - text - over) / s)
}

/// Markdown's half-leading over and under its prose text at 14/22.4 (`line_insets(MD)`).
/// 2 and 3.4 at 1x; 2.67 and 3.2 at 1.875x.
pub(crate) fn prose_insets() -> (f32, f32) {
    line_insets(crate::style::MD)
}

fn atx_heading(line: &str) -> Option<rune::TextStyle> {
    let s = line.trim_start_matches(' ');
    if line.len() - s.len() > 3 { return None; }
    let n = s.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&n) { return None; }
    let rest = &s[n..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) { return None; }
    Some(crate::style::MD_H[(n - 1).min(3)])
}

/// Words; `footer`, they end a turn a later prompt follows, under its footer. A fenced block at an
/// edge is a card's edge there; an ATX heading trims at its own `MD_H` line-height (`markdown.css`).
fn prose(body: &str, footer: bool) -> Edges {
    let footer = if footer { crate::working::COPY_AT.1 + crate::working::COPY } else { 0.0 };
    let trimmed = body.trim();
    let first = trimmed.lines().next().unwrap_or("");
    let last = trimmed.lines().next_back().unwrap_or("");
    let top = if first.starts_with("```") { 0.0 } else { line_insets(atx_heading(first).unwrap_or(crate::style::MD)).0 };
    let bottom = if last.ends_with("```") { 0.0 } else { line_insets(atx_heading(last).unwrap_or(crate::style::MD)).1 };
    Edges { top, bottom, footer, ..edges(Who::Agent, Item::Prose, 0.0) }
}

/// The last turn [`edges_of`] folded: a turn's rows sit together, so each turn folds once.
type Folded<'a> = Option<((usize, usize, bool), Vec<Block<'a>>)>;

fn edges_of<'a>(t: &'a Transcript, detail: &crate::detail::Detail, row: &RowContent, folded: &mut Folded<'a>) -> Option<Edges> {
    Some(match row {
        RowContent::User { .. } | RowContent::Pending { .. } => edges(Who::User, Item::Line, 0.0),
        RowContent::Notice(i, ..) => match &t.messages[*i] {
            Message::Notice(n) if crate::turn_errors::is_divider(n) => return None,
            Message::Notice(n) if matches!(n.kind, opencode_kit::model::NoticeKind::Finished(_)) => Edges { reports: true, ..edges(Who::Agent, Item::Line, LINE_INSET) },
            _ => edges(Who::Event, Item::Line, 2.0),
        },
        RowContent::PendingNotice(..) => Edges { reports: true, ..edges(Who::Agent, Item::Line, LINE_INSET) },
        RowContent::Error { .. } | RowContent::Retry { .. } | RowContent::Widget { .. } => edges(Who::Agent, Item::Line, 0.0),
        RowContent::TurnText { message, part, live, .. } => match &t.messages[*message] {
            Message::Assistant(a) => match a.parts.get(*part) {
                Some(Part::Text { text }) => prose(text, !live && super::turn_footer_text(t, *message, *part)),
                _ => prose("", false),
            },
            _ => prose("", false),
        },
        RowContent::Turn { start, end, block, live, .. } => {
            let turn = (*start, *end, *live);
            if folded.as_ref().is_none_or(|(at, _)| *at != turn) {
                *folded = Some((turn, turns::fold_with(&super::entries(&t.messages[*start..*end], detail), *live, detail)));
            }
            match folded.as_ref().and_then(|(_, blocks)| blocks.get(*block)) {
                Some(Block::Text { body, msg, part, live, .. }) => prose(body, !live && super::turn_footer_text(t, start + msg, *part)),
                // Work reporting back starts a block of its own, which the reply continues.
                Some(Block::Events { steps, .. }) => Edges { reports: steps.iter().all(|s| matches!(s, turns::Step::Notice(_))), ..edges(Who::Agent, Item::Line, LINE_INSET) },
                // A subagent's rows, its reports too, continue the reply (`spacing.ts`'s `reportsBack`
                // counts only events rows).
                Some(Block::Subagent { .. }) => edges(Who::Agent, Item::Line, LINE_INSET),
                Some(Block::Group { .. } | Block::Thought { .. } | Block::Calls { .. }) => edges(Who::Agent, Item::Line, LINE_INSET),
                Some(Block::Question { .. }) => edges(Who::Agent, Item::Line, 0.0),
                None => return None,
            }
        }
        _ => return None,
    })
}

/// The gap between two rows (`spacing.ts`'s `gap`).
fn gap(above: Edges, below: Edges) -> f32 {
    if below.reports || above.who != below.who || below.who == Who::User {
        L
    } else if above.item == below.item {
        S
    } else {
        M
    }
}

/// Sets the room over each row in `out` from the row above it (see the table above).
pub(super) fn space(t: &Transcript, detail: &crate::detail::Detail, out: &mut [RowSpec]) {
    space_at(t, detail, out, 0..out.len());
}

/// [`space`] for rows `at` alone, from the row above them: where two spans of a projection meet
/// (`projection`).
pub(super) fn space_at(t: &Transcript, detail: &crate::detail::Detail, out: &mut [RowSpec], at: std::ops::Range<usize>) {
    let mut folded: Folded = None;
    let mut above: Option<Option<Edges>> = out[..at.start.min(out.len())].iter().rev()
        .find(|r| !matches!(r.content, RowContent::Lane(_)))
        .map(|r| edges_of(t, detail, &r.content, &mut folded));
    let end = at.end.min(out.len());
    for row in out[at.start.min(end)..end].iter_mut() {
        if matches!(row.content, RowContent::Lane(_)) {
            continue;
        }
        let here = edges_of(t, detail, &row.content, &mut folded);
        if let (Some(Some(prev)), Some(h)) = (above, here) {
            let room = (gap(prev, h) - prev.bottom - prev.footer - h.top).max(0.0);
            match &mut row.content {
                // Consecutive prompts hold together as a run (`TurnGap`), which spaces them.
                RowContent::User { reply, .. } | RowContent::Pending { reply, .. } => *reply = if prev.who != Who::User { (L - prev.bottom - prev.footer).max(0.0) } else { 0.0 },
                RowContent::Notice(_, _, pt) | RowContent::PendingNotice(_, pt) => *pt = room,
                RowContent::Error { pt, .. } | RowContent::Retry { pt, .. } | RowContent::TurnText { pt, .. } | RowContent::Turn { pt, .. } | RowContent::Widget { pt, .. } => *pt = room,
                _ => {}
            }
        }
        above = Some(here);
    }
}

/// Extra bottom tail room past the 36 pt Working lane (`tailExtra` in `message-timeline.tsx:226`,
/// `spacing.ts`'s `laneGap`): 0 (`s`), 4 (`m`), or 16 (`l`) so the Working status line keeps its
/// clearance above the floating activity pill.
pub(super) fn tail_extra(t: &Transcript, detail: &crate::detail::Detail, rows: &super::projection::Projection) -> f32 {
    let mut folded: Folded = None;
    let end = (0..rows.len())
        .rev()
        .find(|&i| matches!(rows.get(i), Some(RowSpec { content: RowContent::Actions { .. }, .. })))
        .unwrap_or(rows.len());
    let last = (0..end).rev().find_map(|i| {
        let r = rows.get(i)?;
        if matches!(r.content, RowContent::Lane(_)) {
            return None;
        }
        edges_of(t, detail, &r.content, &mut folded)
    });
    let Some(prev) = last else { return 0.0 };
    (gap(prev, edges(Who::Agent, Item::Line, LINE_INSET)) - S).max(0.0)
}

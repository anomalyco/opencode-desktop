//! Dragging a group's header to move the whole group among the side tabs
//! (`packages/app/src/shell/titlebar/tab-group-drag-machine.ts`, its `move-group` path): idle,
//! pending until the pointer travels 4 pt (8 by touch), then dragging, the group following the
//! pointer and a line where it would land; a release commits the move.
//!
//! Desktop's machine also drags single tabs between groups (`place-tab`, `join-group`, and a
//! 220 ms dwell that combines two tabs into a new group). Here a tab drags within its own
//! section, by the tab strip's own reorder, so those intents aren't ported.

use std::collections::HashMap;

use crate::model::{Doc, ManualGroup};

/// A top-level block of the strip in content points (`MeasuredTopBlock`): a group with all its
/// tabs, or one ungrouped tab.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Group { key: String, tabs: Vec<String>, top: f32, bottom: f32 },
    Tab { key: String, top: f32, bottom: f32 },
}

impl Block {
    fn top(&self) -> f32 {
        match self {
            Block::Group { top, .. } | Block::Tab { top, .. } => *top,
        }
    }

    fn bottom(&self) -> f32 {
        match self {
            Block::Group { bottom, .. } | Block::Tab { bottom, .. } => *bottom,
        }
    }

    fn is_group(&self, key: &str) -> bool {
        matches!(self, Block::Group { key: k, .. } if k == key)
    }
}

/// Where the dragged group lands (`VerticalDragIntent`'s `move-group`): before block
/// `before`, or after the last at `blocks.len()`.
#[derive(Clone, Debug, PartialEq)]
pub struct Intent {
    pub group: String,
    pub before: usize,
}

/// The gesture (`VerticalDragState`). `Dropped` holds the release that ended a drag so the
/// header's trailing press doesn't toggle the group (Desktop's `suppress-click` effect).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    #[default]
    Idle,
    Pending { group: String, touch: bool },
    Dragging { group: String, blocks: Vec<Block>, grab: f32, offset: f32, line: Option<f32>, intent: Option<Intent> },
    Dropped,
}

pub enum Event {
    /// `pointer.down` on a group's header.
    Down { group: String, touch: bool },
    /// `pointer.move`: travel since the press, the pointer's place in the strip at the press,
    /// and the strip's blocks as laid out (read once, as the drag starts).
    Move { dx: f32, dy: f32, grab: f32, blocks: Vec<Block> },
    Up,
    Cancel,
    /// The header's press after the gesture: a click unless a drag just ended.
    Press,
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    None,
    Redraw,
    /// The header was clicked: toggle the group.
    Click,
    Commit { blocks: Vec<Block>, intent: Intent },
}

impl State {
    pub fn dragging(&self, group: &str) -> Option<f32> {
        match self {
            State::Dragging { group: g, offset, .. } if g == group => Some(*offset),
            _ => None,
        }
    }

    pub fn line(&self) -> Option<f32> {
        match self {
            State::Dragging { line, .. } => *line,
            _ => None,
        }
    }

    pub fn active(&self) -> bool {
        matches!(self, State::Dragging { .. })
    }
}

/// `transitionVerticalDrag`.
pub fn step(state: &mut State, event: Event) -> Effect {
    match (std::mem::take(state), event) {
        (State::Idle | State::Dropped | State::Pending { .. }, Event::Down { group, touch }) => {
            *state = State::Pending { group, touch };
            Effect::None
        }
        (State::Pending { group, touch }, Event::Move { dx, dy, grab, blocks }) => {
            // `selectNeedsDragMeasurement`: 4 pt, or 8 for a finger.
            if dx.hypot(dy) < if touch { 8.0 } else { 4.0 } {
                *state = State::Pending { group, touch };
                return Effect::None;
            }
            *state = dragging(group, blocks, grab, dy);
            Effect::Redraw
        }
        (State::Dragging { group, blocks, grab, .. }, Event::Move { dy, .. }) => {
            *state = dragging(group, blocks, grab, dy);
            Effect::Redraw
        }
        (State::Dragging { blocks, intent, .. }, Event::Up) => {
            *state = State::Dropped;
            match intent {
                Some(intent) => Effect::Commit { blocks, intent },
                None => Effect::Redraw,
            }
        }
        (State::Dragging { .. }, Event::Cancel) => Effect::Redraw,
        (State::Idle | State::Pending { .. }, Event::Press) => Effect::Click,
        (State::Dropped, Event::Press) => Effect::None,
        (State::Pending { .. }, Event::Up | Event::Cancel) => Effect::None,
        (s, _) => {
            *state = s;
            Effect::None
        }
    }
}

fn dragging(group: String, blocks: Vec<Block>, grab: f32, dy: f32) -> State {
    let offset = clamped_offset(&group, &blocks, dy);
    let (line, intent) = match hit(&group, &blocks, grab + dy) {
        Some((line, intent)) => (Some(line), Some(intent)),
        None => (None, None),
    };
    State::Dragging { group, blocks, grab, offset, line, intent }
}

/// `computeClampedDragOffsetY` for a group: it stays within the strip.
fn clamped_offset(group: &str, blocks: &[Block], dy: f32) -> f32 {
    let Some(block) = blocks.iter().find(|b| b.is_group(group)) else { return dy };
    let first = blocks.first().map_or(0.0, Block::top);
    let last = blocks.last().map_or(0.0, Block::bottom);
    dy.clamp(first - block.top() - 4.0, (last - block.bottom() + 14.0).max(0.0)).round()
}

/// `hitTestGroupMove`: the pointer passes a block above at its middle, one below 36 pt (or half
/// its height) into it. Landing where it already is moves nothing.
fn hit(group: &str, blocks: &[Block], y: f32) -> Option<(f32, Intent)> {
    if blocks.len() <= 1 {
        return None;
    }
    let me = blocks.iter().position(|b| b.is_group(group))?;
    let before = blocks
        .iter()
        .enumerate()
        .position(|(i, b)| {
            let h = (b.bottom() - b.top()).max(1.0);
            let pivot = if i < me { (b.top() + b.bottom()) / 2.0 } else { b.top() + (h * 0.5).min(36.0) };
            y < pivot
        })
        .unwrap_or(blocks.len());
    if before == me || before == me + 1 {
        return None;
    }
    let line = match before {
        0 => (blocks[0].top() - 2.0).max(3.0),
        b if b >= blocks.len() => blocks[blocks.len() - 1].bottom() + 3.0,
        b => (blocks[b - 1].bottom() + blocks[b].top()) / 2.0,
    };
    Some((line, Intent { group: group.to_owned(), before }))
}

/// What a commit leaves (`VerticalDragCommitResult`): the window's tab order and the saved
/// state's group order and manual groups.
pub struct Commit {
    pub tabs: Vec<String>,
    pub group_order: Vec<String>,
    pub manual_groups: Vec<ManualGroup>,
}

/// `applyVerticalDragCommit`'s `move-group`: the blocks in their new order give the tab order
/// and the saved group order; manual groups keep the same order as their sections.
pub fn commit(all: &[String], doc: &Doc, blocks: &[Block], intent: &Intent) -> Commit {
    let mut out = Commit { tabs: all.to_vec(), group_order: doc.group_order.clone(), manual_groups: doc.manual_groups.clone() };
    let Some(from) = blocks.iter().position(|b| b.is_group(&intent.group)) else { return out };
    let to = if intent.before > from { intent.before - 1 } else { intent.before };
    let mut moved = blocks.to_vec();
    let block = moved.remove(from);
    moved.insert(to.min(moved.len()), block);
    let ordered: Vec<String> = moved
        .iter()
        .flat_map(|b| match b {
            Block::Group { tabs, .. } => tabs.clone(),
            Block::Tab { key, .. } => vec![key.clone()],
        })
        .collect();
    let rest: Vec<String> = all.iter().filter(|k| !ordered.contains(k)).cloned().collect();
    out.tabs = ordered.into_iter().chain(rest).collect();
    out.group_order = moved
        .iter()
        .filter_map(|b| match b {
            Block::Group { key, .. } => Some(key.clone()),
            Block::Tab { .. } => None,
        })
        .collect();
    if out.manual_groups.len() > 1 {
        let rank: HashMap<&str, usize> = out.group_order.iter().filter_map(|k| k.strip_prefix("manual:")).enumerate().map(|(i, id)| (id, i)).collect();
        out.manual_groups.sort_by_key(|g| rank.get(g.id.as_str()).copied().unwrap_or(999));
    }
    out
}

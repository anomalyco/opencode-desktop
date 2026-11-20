//! Full 4-intent vertical drag state machine for grouped side tabs
//! (`packages/app/src/shell/titlebar/tab-group-drag-machine.ts`, #64407):
//! - `MoveGroup`: drag a group header to move the entire group.
//! - `PlaceTab`: drag a tab within a group, across manual groups, or outdent to ungroup.
//! - `JoinGroup`: drop a tab onto a manual group header to join it (expanding if collapsed).
//! - `CombineTabs`: dwell 220 ms over another ungrouped tab to create a new manual group.

mod hit;

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use crate::model::{Color, Doc, ManualGroup, Mode, canonical_order, next_manual_color, next_manual_number, prune_empty};

/// `TAB_COMBINE_DWELL_MS` (`tab-group-drag-machine.ts:12`): 220 ms dwell arms combining two tabs.
pub const COMBINE_DWELL_SECS: f64 = 0.22;

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Group { key: String, manual_id: Option<String>, tabs: Vec<String>, top: f32, bottom: f32 },
    Tab { key: String, top: f32, bottom: f32 },
}

impl Block {
    pub(super) fn top(&self) -> f32 {
        match self {
            Block::Group { top, .. } | Block::Tab { top, .. } => *top,
        }
    }

    pub(super) fn bottom(&self) -> f32 {
        match self {
            Block::Group { bottom, .. } | Block::Tab { bottom, .. } => *bottom,
        }
    }

    pub(super) fn is_group(&self, key: &str) -> bool {
        matches!(self, Block::Group { key: k, .. } if k == key)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum StripRow {
    Header {
        group_key: String,
        manual_id: Option<String>,
        color: Color,
        collapsed: bool,
        first_tab: Option<String>,
        last_tab: Option<String>,
        top: f32,
        bottom: f32,
    },
    Tab {
        tab_key: String,
        group_key: Option<String>,
        manual_id: Option<String>,
        color: Option<Color>,
        is_last: bool,
        top: f32,
        bottom: f32,
    },
}

impl StripRow {
    pub(super) fn top(&self) -> f32 {
        match self {
            StripRow::Header { top, .. } | StripRow::Tab { top, .. } => *top,
        }
    }

    pub(super) fn bottom(&self) -> f32 {
        match self {
            StripRow::Header { bottom, .. } | StripRow::Tab { bottom, .. } => *bottom,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Geometry {
    pub list_left: f32,
    pub freeform: bool,
    pub next_color: Option<Color>,
    pub blocks: Vec<Block>,
    pub rows: Vec<StripRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Group { group_key: String },
    Tab { tab_key: String, group_key: Option<String>, manual_id: Option<String> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneShift {
    In,
    Out,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Indicator {
    Line { y: f32, visual_y: f32, indented: bool, group_key: Option<String>, color: Option<Color> },
    CombineTab { tab_key: String, color: Color, source_before: bool },
    JoinGroup { group_key: String, color: Color },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    MoveGroup { group_key: String, before_block: usize },
    PlaceTab { tab_key: String, target_manual_id: Option<String>, anchor_tab: Option<String>, before: bool },
    JoinGroup { tab_key: String, group_key: String, manual_id: String, prepend: bool },
    CombineTabs { source_tab: String, target_tab: String, source_before: bool },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shifts {
    pub tab_shift_y: HashMap<String, f32>,
    pub group_shift_y: HashMap<String, f32>,
    pub header_shift_y: HashMap<String, f32>,
    pub lane_shift: Option<LaneShift>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    #[default]
    Idle,
    Pending { source: Source, touch: bool },
    Dragging {
        source: Source,
        geometry: Geometry,
        grab: f32,
        x: f32,
        content_y: f32,
        offset_y: f32,
        dwell_candidate: Option<String>,
        dwell_due: Option<f64>,
        armed_combine: Option<String>,
        indicator: Option<Indicator>,
        intent: Option<Intent>,
        shifts: Shifts,
    },
    /// Trailing click suppression right after a drag ends (`suppress-click`).
    Dropped,
}

pub enum Event {
    Down { source: Source, touch: bool },
    Move { x: f32, dx: f32, dy: f32, grab: f32, geometry: Option<Geometry>, now: f64 },
    Tick { now: f64 },
    Up,
    Cancel,
    Press,
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    None,
    Redraw { wake_after: Option<Duration> },
    Click,
    Commit { blocks: Vec<Block>, intent: Intent },
}

impl State {
    pub fn active(&self) -> bool {
        matches!(self, State::Dragging { .. })
    }

    pub fn dragging_group(&self, key: &str) -> Option<f32> {
        match self {
            State::Dragging { source: Source::Group { group_key }, offset_y, .. } if group_key == key => Some(*offset_y),
            _ => None,
        }
    }

    pub fn dragging_tab(&self, key: &str) -> Option<(f32, Option<LaneShift>)> {
        match self {
            State::Dragging { source: Source::Tab { tab_key, .. }, offset_y, shifts, .. } if tab_key == key => {
                Some((*offset_y, shifts.lane_shift))
            }
            _ => None,
        }
    }

    pub fn group_shift(&self, key: &str) -> f32 {
        match self {
            State::Dragging { shifts, .. } => shifts.group_shift_y.get(key).copied().unwrap_or(0.0),
            _ => 0.0,
        }
    }

    pub fn header_shift(&self, key: &str) -> f32 {
        match self {
            State::Dragging { shifts, .. } => shifts.header_shift_y.get(key).copied().unwrap_or(0.0),
            _ => 0.0,
        }
    }

    pub fn tab_shift(&self, key: &str) -> f32 {
        match self {
            State::Dragging { shifts, .. } => shifts.tab_shift_y.get(key).copied().unwrap_or(0.0),
            _ => 0.0,
        }
    }

    pub fn indicator(&self) -> Option<&Indicator> {
        match self {
            State::Dragging { indicator, .. } => indicator.as_ref(),
            _ => None,
        }
    }

    pub fn join_group_key(&self) -> Option<&str> {
        match self.indicator() {
            Some(Indicator::JoinGroup { group_key, .. }) => Some(group_key.as_str()),
            _ => None,
        }
    }

    pub fn combine_tab(&self, tab_key: &str) -> Option<(Color, bool)> {
        match self.indicator() {
            Some(Indicator::CombineTab { tab_key: k, color, source_before }) if k == tab_key => Some((*color, *source_before)),
            _ => None,
        }
    }

    pub fn dwell_due(&self) -> Option<f64> {
        match self {
            State::Dragging { dwell_due, .. } => *dwell_due,
            _ => None,
        }
    }
}

pub fn step(state: &mut State, event: Event) -> Effect {
    match (std::mem::take(state), event) {
        (State::Idle | State::Dropped | State::Pending { .. }, Event::Down { source, touch }) => {
            *state = State::Pending { source, touch };
            Effect::None
        }
        (State::Pending { source, touch }, Event::Move { x, dx, dy, grab, geometry, now }) => {
            if dx.hypot(dy) < if touch { 8.0 } else { 4.0 } {
                *state = State::Pending { source, touch };
                return Effect::None;
            }
            let Some(geometry) = geometry else {
                *state = State::Pending { source, touch };
                return Effect::None;
            };
            let (next, wake) = start_or_update(source, geometry, grab, x, dy, None, None, None, now);
            *state = next;
            Effect::Redraw { wake_after: wake }
        }
        (
            State::Dragging {
                source,
                geometry,
                grab,
                dwell_candidate,
                dwell_due,
                armed_combine,
                ..
            },
            Event::Move { x, dy, now, .. },
        ) => {
            let content_y = grab + dy;
            let kept_armed = armed_combine.filter(|k| combine_locked(k, &geometry, x, content_y));
            let (next, wake) = start_or_update(source, geometry, grab, x, dy, dwell_candidate, dwell_due, kept_armed, now);
            *state = next;
            Effect::Redraw { wake_after: wake }
        }
        (
            State::Dragging {
                source,
                geometry,
                grab,
                x,
                content_y,
                offset_y,
                dwell_candidate: Some(candidate),
                dwell_due: Some(due),
                ..
            },
            Event::Tick { now },
        ) => {
            if now < due {
                let rem = Duration::from_secs_f64((due - now).max(0.005));
                *state = State::Dragging {
                    source,
                    geometry,
                    grab,
                    x,
                    content_y,
                    offset_y,
                    dwell_candidate: Some(candidate),
                    dwell_due: Some(due),
                    armed_combine: None,
                    indicator: None,
                    intent: None,
                    shifts: Shifts::default(),
                };
                return Effect::Redraw { wake_after: Some(rem) };
            }
            let hit = hit::hit_test(&source, &geometry, x, content_y, Some(candidate.as_str()));
            *state = State::Dragging {
                source,
                geometry,
                grab,
                x,
                content_y,
                offset_y,
                dwell_candidate: None,
                dwell_due: None,
                armed_combine: Some(candidate),
                indicator: hit.indicator,
                intent: hit.intent,
                shifts: hit.shifts,
            };
            Effect::Redraw { wake_after: None }
        }
        (State::Dragging { geometry, intent, .. }, Event::Up) => {
            *state = State::Dropped;
            match intent {
                Some(intent) => Effect::Commit { blocks: geometry.blocks, intent },
                None => Effect::Redraw { wake_after: None },
            }
        }
        (State::Dragging { .. }, Event::Cancel) => Effect::Redraw { wake_after: None },
        (State::Idle | State::Pending { .. }, Event::Press) => Effect::Click,
        (State::Dropped, Event::Press) => Effect::None,
        (State::Pending { .. }, Event::Up | Event::Cancel) => Effect::None,
        (s, _) => {
            *state = s;
            Effect::None
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn start_or_update(
    source: Source,
    geometry: Geometry,
    grab: f32,
    x: f32,
    dy: f32,
    prev_candidate: Option<String>,
    prev_due: Option<f64>,
    armed: Option<String>,
    now: f64,
) -> (State, Option<Duration>) {
    let content_y = grab + dy;
    let offset_y = clamped_offset(&source, &geometry, dy);
    let hit = hit::hit_test(&source, &geometry, x, content_y, armed.as_deref());
    let (dwell_candidate, dwell_due, wake) = match (&hit.dwell_candidate, &prev_candidate) {
        (Some(cand), Some(prev)) if cand == prev => {
            let due = prev_due.unwrap_or(now + COMBINE_DWELL_SECS);
            let wake = (due > now).then(|| Duration::from_secs_f64((due - now).max(0.005)));
            (Some(cand.clone()), Some(due), wake)
        }
        (Some(cand), _) => (Some(cand.clone()), Some(now + COMBINE_DWELL_SECS), Some(Duration::from_secs_f64(COMBINE_DWELL_SECS))),
        (None, _) => (None, None, None),
    };
    let next = State::Dragging {
        source,
        geometry,
        grab,
        x,
        content_y,
        offset_y,
        dwell_candidate,
        dwell_due,
        armed_combine: armed,
        indicator: hit.indicator,
        intent: hit.intent,
        shifts: hit.shifts,
    };
    (next, wake)
}

fn combine_locked(armed_key: &str, geometry: &Geometry, x: f32, content_y: f32) -> bool {
    if !geometry.freeform || x < geometry.list_left + 12.0 {
        return false;
    }
    geometry.rows.iter().any(|r| match r {
        StripRow::Tab { tab_key, manual_id: None, top, bottom, .. } if tab_key == armed_key => {
            content_y >= *top - 16.0 && content_y <= *bottom + 16.0
        }
        _ => false,
    })
}

fn clamped_offset(source: &Source, geometry: &Geometry, dy: f32) -> f32 {
    match source {
        Source::Group { group_key } => {
            let Some(block) = geometry.blocks.iter().find(|b| b.is_group(group_key)) else { return dy };
            let first = geometry.blocks.first().map_or(0.0, Block::top);
            let last = geometry.blocks.last().map_or(0.0, Block::bottom);
            dy.clamp(first - block.top() - 4.0, (last - block.bottom() + 14.0).max(0.0)).round()
        }
        Source::Tab { tab_key, .. } => {
            let Some(row) = geometry.rows.iter().find(|r| matches!(r, StripRow::Tab { tab_key: k, .. } if k == tab_key)) else {
                return dy;
            };
            let first = geometry.rows.first().map_or(0.0, StripRow::top);
            let last = geometry.rows.last().map_or(0.0, StripRow::bottom);
            dy.clamp(first - row.top() - 6.0, (last - row.bottom() + 26.0).max(0.0)).round()
        }
    }
}

pub struct Commit {
    pub tabs: Vec<String>,
    pub mode: Mode,
    pub group_order: Vec<String>,
    pub collapsed: BTreeMap<String, bool>,
    pub manual_groups: Vec<ManualGroup>,
    pub manual_assignments: BTreeMap<String, String>,
}

/// Applies any of the 4 drag intents cleanly (`applyVerticalDragCommit`).
pub fn commit(
    all: &[String],
    doc: &Doc,
    blocks: &[Block],
    intent: &Intent,
    new_id: &dyn Fn() -> String,
    format_name: &dyn Fn(usize) -> String,
) -> Commit {
    let mut out = Commit {
        tabs: all.to_vec(),
        mode: doc.mode,
        group_order: doc.group_order.clone(),
        collapsed: doc.collapsed.clone(),
        manual_groups: doc.manual_groups.clone(),
        manual_assignments: doc.manual_assignments.clone(),
    };
    match intent {
        Intent::MoveGroup { group_key, before_block } => {
            let Some(from) = blocks.iter().position(|b| b.is_group(group_key)) else { return out };
            let to = if *before_block > from { before_block - 1 } else { *before_block };
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
                let rank: HashMap<&str, usize> = out
                    .group_order
                    .iter()
                    .filter_map(|k| k.strip_prefix("manual:"))
                    .enumerate()
                    .map(|(i, id)| (id, i))
                    .collect();
                out.manual_groups.sort_by_key(|g| rank.get(g.id.as_str()).copied().unwrap_or(999));
            }
        }
        Intent::CombineTabs { source_tab, target_tab, source_before } => {
            out.mode = Mode::None;
            let id = new_id();
            let color = next_manual_color(&out.manual_groups);
            let name = format_name(next_manual_number(&out.manual_groups, format_name));
            out.manual_groups.push(ManualGroup { id: id.clone(), name, color });
            out.manual_assignments.insert(target_tab.clone(), id.clone());
            out.manual_assignments.insert(source_tab.clone(), id);
            out.tabs = place_relative(&out.tabs, source_tab, target_tab, *source_before);
            out.manual_groups = prune_empty(&out.manual_groups, &out.manual_assignments);
            out.tabs = canonical_order(&out.tabs, &out.manual_assignments);
        }
        Intent::JoinGroup { tab_key, manual_id, prepend, .. } => {
            out.mode = Mode::None;
            out.manual_assignments.insert(tab_key.clone(), manual_id.clone());
            out.collapsed.insert(format!("manual:{manual_id}"), false);
            let members: Vec<&String> = out
                .tabs
                .iter()
                .filter(|k| *k != tab_key && out.manual_assignments.get(*k).map(String::as_str) == Some(manual_id.as_str()))
                .collect();
            if let Some(anchor) = if *prepend { members.first() } else { members.last() } {
                out.tabs = place_relative(&out.tabs, tab_key, anchor, *prepend);
            }
            out.manual_groups = prune_empty(&out.manual_groups, &out.manual_assignments);
            out.tabs = canonical_order(&out.tabs, &out.manual_assignments);
        }
        Intent::PlaceTab { tab_key, target_manual_id, anchor_tab, before } => {
            if out.mode == Mode::None || !out.manual_groups.is_empty() || target_manual_id.is_some() {
                match target_manual_id {
                    Some(mid) => {
                        out.mode = Mode::None;
                        out.manual_assignments.insert(tab_key.clone(), mid.clone());
                    }
                    None => {
                        out.manual_assignments.remove(tab_key);
                    }
                }
                out.manual_groups = prune_empty(&out.manual_groups, &out.manual_assignments);
            }
            if let Some(anchor) = anchor_tab {
                out.tabs = place_relative(&out.tabs, tab_key, anchor, *before);
            }
            if out.mode == Mode::None && !out.manual_groups.is_empty() {
                out.tabs = canonical_order(&out.tabs, &out.manual_assignments);
            }
        }
    }
    out
}

fn place_relative(keys: &[String], moved: &str, anchor: &str, before: bool) -> Vec<String> {
    if moved == anchor {
        return keys.to_vec();
    }
    let mut out: Vec<String> = keys.iter().filter(|k| k.as_str() != moved).cloned().collect();
    let Some(idx) = out.iter().position(|k| k == anchor) else {
        return keys.to_vec();
    };
    let at = if before { idx } else { idx + 1 };
    out.insert(at.min(out.len()), moved.to_owned());
    out
}

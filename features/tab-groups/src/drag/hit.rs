//! Hit testing and slide-out shift calculation for grouped side-tab vertical drags.

use super::{Block, Geometry, Indicator, Intent, LaneShift, Shifts, Source, StripRow};
use crate::model::Color;

pub(super) struct Hit {
    pub(super) indicator: Option<Indicator>,
    pub(super) intent: Option<Intent>,
    pub(super) shifts: Shifts,
    pub(super) dwell_candidate: Option<String>,
}

impl Hit {
    fn empty(dwell_candidate: Option<String>) -> Self {
        Self { indicator: None, intent: None, shifts: Shifts::default(), dwell_candidate }
    }
}

pub(super) fn hit_test(source: &Source, geometry: &Geometry, x: f32, content_y: f32, armed: Option<&str>) -> Hit {
    match source {
        Source::Group { group_key } => hit_group_move(group_key, &geometry.blocks, content_y),
        Source::Tab { tab_key, group_key, manual_id } => {
            let raw = if geometry.freeform {
                hit_freeform_tab(tab_key, manual_id.as_deref(), geometry, x, content_y, armed)
            } else {
                hit_intra_group_tab(tab_key, group_key.as_deref(), manual_id.as_deref(), &geometry.rows, content_y)
            };
            attach_tab_shifts(tab_key, group_key.as_deref(), geometry, raw)
        }
    }
}

fn hit_group_move(group_key: &str, blocks: &[Block], y: f32) -> Hit {
    if blocks.len() <= 1 {
        return Hit::empty(None);
    }
    let Some(me) = blocks.iter().position(|b| b.is_group(group_key)) else {
        return Hit::empty(None);
    };
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
        return Hit::empty(None);
    }
    let self_b = &blocks[me];
    let stride = if me + 1 < blocks.len() {
        blocks[me + 1].top() - self_b.top()
    } else if me > 0 {
        self_b.bottom() - blocks[me - 1].bottom()
    } else {
        self_b.bottom() - self_b.top() + 5.0
    };
    let mut shifts = Shifts::default();
    if before < me {
        for b in &blocks[before..me] {
            match b {
                Block::Group { key, .. } => _ = shifts.group_shift_y.insert(key.clone(), stride),
                Block::Tab { key, .. } => _ = shifts.tab_shift_y.insert(key.clone(), stride),
            }
        }
    } else {
        for b in &blocks[me + 1..before] {
            match b {
                Block::Group { key, .. } => _ = shifts.group_shift_y.insert(key.clone(), -stride),
                Block::Tab { key, .. } => _ = shifts.tab_shift_y.insert(key.clone(), -stride),
            }
        }
    }
    let line_y = match before {
        0 => (blocks[0].top() - 2.0).max(3.0),
        b if b >= blocks.len() => blocks[blocks.len() - 1].bottom() + 3.0,
        b => (blocks[b - 1].bottom() + blocks[b].top()) / 2.0,
    };
    let visual_y = if before < me { line_y } else { (blocks[before - 1].bottom() - stride + 2.5).max(3.0) };
    Hit {
        indicator: Some(Indicator::Line { y: line_y, visual_y, indented: false, group_key: None, color: None }),
        intent: Some(Intent::MoveGroup { group_key: group_key.to_owned(), before_block: before }),
        shifts,
        dwell_candidate: None,
    }
}

fn hit_intra_group_tab(tab_key: &str, group_key: Option<&str>, manual_id: Option<&str>, rows: &[StripRow], y: f32) -> Hit {
    let peers: Vec<&StripRow> = rows
        .iter()
        .filter(|r| matches!(r, StripRow::Tab { group_key: gk, .. } if gk.as_deref() == group_key))
        .collect();
    if peers.len() <= 1 {
        return Hit::empty(None);
    }
    let Some(me) = peers.iter().position(|r| matches!(r, StripRow::Tab { tab_key: k, .. } if k == tab_key)) else {
        return Hit::empty(None);
    };
    let insert = peers
        .iter()
        .position(|r| y < (r.top() + r.bottom()) / 2.0)
        .unwrap_or(peers.len());
    if insert == me || insert == me + 1 {
        return Hit::empty(None);
    }
    let color = match peers[0] {
        StripRow::Tab { color, .. } => *color,
        _ => None,
    };
    let indented = group_key.is_some();
    let (raw_y, anchor, before) = if insert == 0 {
        let StripRow::Tab { tab_key: a, .. } = peers[0] else { return Hit::empty(None) };
        ((peers[0].top() - 1.5).max(3.0), a.clone(), true)
    } else if insert >= peers.len() {
        let last = peers[peers.len() - 1];
        let StripRow::Tab { tab_key: a, .. } = last else { return Hit::empty(None) };
        (last.bottom() + 1.5, a.clone(), false)
    } else {
        let prev = peers[insert - 1];
        let next = peers[insert];
        let StripRow::Tab { tab_key: a, .. } = next else { return Hit::empty(None) };
        ((prev.bottom() + next.top()) / 2.0, a.clone(), true)
    };
    Hit {
        indicator: Some(Indicator::Line {
            y: raw_y,
            visual_y: raw_y,
            indented,
            group_key: group_key.map(str::to_owned),
            color,
        }),
        intent: Some(Intent::PlaceTab {
            tab_key: tab_key.to_owned(),
            target_manual_id: manual_id.map(str::to_owned),
            anchor_tab: Some(anchor),
            before,
        }),
        shifts: Shifts::default(),
        dwell_candidate: None,
    }
}

fn seam_before(rows: &[StripRow], i: usize) -> f32 {
    if i == 0 { (rows[0].top() - 2.0).max(3.0) } else { (rows[i - 1].bottom() + rows[i].top()) / 2.0 }
}

fn seam_after(rows: &[StripRow], i: usize) -> f32 {
    if i + 1 >= rows.len() { rows[rows.len() - 1].bottom() + 4.0 } else { (rows[i].bottom() + rows[i + 1].top()) / 2.0 }
}

fn hit_freeform_tab(tab_key: &str, src_manual: Option<&str>, geometry: &Geometry, x: f32, y: f32, armed: Option<&str>) -> Hit {
    let rows = &geometry.rows;
    if rows.is_empty() {
        return Hit::empty(None);
    }
    let outdent = x < geometry.list_left + 12.0;
    let src_center = rows
        .iter()
        .find(|r| matches!(r, StripRow::Tab { tab_key: k, .. } if k == tab_key))
        .map_or(y, |r| (r.top() + r.bottom()) / 2.0);

    if let Some(armed_key) = armed
        && !outdent
        && let Some(StripRow::Tab { tab_key: target, manual_id: None, top, .. }) =
            rows.iter().find(|r| matches!(r, StripRow::Tab { tab_key: k, .. } if k == armed_key))
        && target != tab_key
    {
        let source_before = *top < src_center;
        let color = geometry.next_color.unwrap_or(Color::Blue);
        return Hit {
            indicator: Some(Indicator::CombineTab { tab_key: target.clone(), color, source_before }),
            intent: Some(Intent::CombineTabs { source_tab: tab_key.to_owned(), target_tab: target.clone(), source_before }),
            shifts: Shifts::default(),
            dwell_candidate: None,
        };
    }

    let make_place = |line_y: f32, target_manual: Option<&str>, gk: Option<&str>, color: Option<Color>, anchor: Option<&str>, before: bool, dwell: Option<String>| -> Hit {
        if is_noop_placement(tab_key, src_manual, rows, target_manual, anchor, before) {
            return Hit::empty(dwell);
        }
        let indented = target_manual.is_some();
        let clamped = line_y.max(3.0);
        Hit {
            indicator: Some(Indicator::Line {
                y: clamped,
                visual_y: clamped,
                indented,
                group_key: indented.then(|| gk.map(str::to_owned)).flatten(),
                color: indented.then_some(color).flatten(),
            }),
            intent: Some(Intent::PlaceTab {
                tab_key: tab_key.to_owned(),
                target_manual_id: target_manual.map(str::to_owned),
                anchor_tab: anchor.map(str::to_owned),
                before,
            }),
            shifts: Shifts::default(),
            dwell_candidate: dwell,
        }
    };

    if y < rows[0].top() {
        let anchor = match &rows[0] {
            StripRow::Tab { tab_key: a, .. } => Some(a.as_str()),
            StripRow::Header { first_tab, .. } => first_tab.as_deref(),
        };
        return make_place(seam_before(rows, 0), None, None, None, anchor, true, None);
    }
    let last_i = rows.len() - 1;
    if y > rows[last_i].bottom() {
        let anchor = match &rows[last_i] {
            StripRow::Tab { tab_key: a, .. } => Some(a.as_str()),
            StripRow::Header { last_tab, .. } => last_tab.as_deref(),
        };
        return make_place(seam_after(rows, last_i), None, None, None, anchor, false, None);
    }

    for (i, row) in rows.iter().enumerate() {
        if y >= row.top() && y <= row.bottom() {
            let h = (row.bottom() - row.top()).max(1.0);
            match row {
                StripRow::Header { group_key, manual_id, color, collapsed, first_tab, last_tab, top, bottom } => {
                    if y < top + h * 0.22 {
                        return make_place(seam_before(rows, i), None, None, None, first_tab.as_deref(), true, None);
                    }
                    if *collapsed && y > bottom - h * 0.22 {
                        return make_place(seam_after(rows, i), None, None, None, last_tab.as_deref(), false, None);
                    }
                    let Some(mid) = manual_id.as_deref() else { return Hit::empty(None) };
                    if src_manual == Some(mid) {
                        let next_y = match rows.get(i + 1) {
                            Some(StripRow::Tab { group_key: Some(gk), top: nt, .. }) if gk == group_key => (bottom + nt) / 2.0,
                            _ => bottom + 1.5,
                        };
                        return make_place(next_y, Some(mid), Some(group_key), Some(*color), first_tab.as_deref(), true, None);
                    }
                    return Hit {
                        indicator: Some(Indicator::JoinGroup { group_key: group_key.clone(), color: *color }),
                        intent: Some(Intent::JoinGroup {
                            tab_key: tab_key.to_owned(),
                            group_key: group_key.clone(),
                            manual_id: mid.to_owned(),
                            prepend: !collapsed,
                        }),
                        shifts: Shifts::default(),
                        dwell_candidate: None,
                    };
                }
                StripRow::Tab { tab_key: k, group_key, manual_id, color, is_last, top, bottom } => {
                    if k == tab_key {
                        if let Some(mid) = manual_id.as_deref()
                            && outdent
                        {
                            return outdent_after_group(rows, mid, i, k, &make_place);
                        }
                        return Hit::empty(None);
                    }
                    if let Some(mid) = manual_id.as_deref() {
                        if outdent {
                            return outdent_after_group(rows, mid, i, k, &make_place);
                        }
                        if y < (top + bottom) / 2.0 {
                            return make_place(seam_before(rows, i), Some(mid), group_key.as_deref(), *color, Some(k), true, None);
                        }
                        let after_y = if *is_last { bottom + 1.5 } else { seam_after(rows, i) };
                        return make_place(after_y, Some(mid), group_key.as_deref(), *color, Some(k), false, None);
                    }
                    let dragging_up = *top < src_center;
                    let cand = (!outdent).then(|| k.clone());
                    if dragging_up {
                        if y < top + 6.0 {
                            return make_place(seam_before(rows, i), None, None, None, Some(k), true, None);
                        }
                        return make_place(seam_after(rows, i), None, None, None, Some(k), false, cand);
                    }
                    if y > bottom - 6.0 {
                        return make_place(seam_after(rows, i), None, None, None, Some(k), false, None);
                    }
                    return make_place(seam_before(rows, i), None, None, None, Some(k), true, cand);
                }
            }
        }
        if let Some(next) = rows.get(i + 1)
            && y > row.bottom()
            && y < next.top()
        {
            let mid_gap = (row.bottom() + next.top()) / 2.0;
            if let (StripRow::Header { group_key: hgk, manual_id: Some(hmid), color, .. }, StripRow::Tab { group_key: Some(tgk), tab_key: tk, .. }) = (row, next)
                && hgk == tgk
            {
                return make_place(mid_gap, Some(hmid), Some(hgk), Some(*color), Some(tk), true, None);
            }
            if let (StripRow::Tab { manual_id: Some(m1), .. }, StripRow::Tab { manual_id: Some(m2), group_key: tgk, color, tab_key: tk, .. }) = (row, next)
                && m1 == m2
            {
                if outdent {
                    return outdent_after_group(rows, m1, i, tk, &make_place);
                }
                return make_place(mid_gap, Some(m1), tgk.as_deref(), *color, Some(tk), true, None);
            }
            let next_anchor = match next {
                StripRow::Tab { tab_key: a, .. } => Some(a.as_str()),
                StripRow::Header { first_tab, .. } => first_tab.as_deref(),
            };
            if next_anchor.is_some() {
                return make_place(mid_gap, None, None, None, next_anchor, true, None);
            }
            if let StripRow::Tab { tab_key: a, .. } = row {
                return make_place(mid_gap, None, None, None, Some(a.as_str()), false, None);
            }
        }
    }
    Hit::empty(None)
}

fn outdent_after_group(
    rows: &[StripRow],
    mid: &str,
    fallback_idx: usize,
    fallback_key: &str,
    make_place: &impl Fn(f32, Option<&str>, Option<&str>, Option<Color>, Option<&str>, bool, Option<String>) -> Hit,
) -> Hit {
    let last_idx = rows
        .iter()
        .rposition(|r| matches!(r, StripRow::Tab { manual_id: Some(m), .. } if m == mid))
        .unwrap_or(fallback_idx);
    let anchor = match &rows[last_idx] {
        StripRow::Tab { tab_key, .. } => tab_key.as_str(),
        _ => fallback_key,
    };
    make_place(seam_after(rows, last_idx), None, None, None, Some(anchor), false, None)
}

fn is_noop_placement(
    src_key: &str,
    src_manual: Option<&str>,
    rows: &[StripRow],
    target_manual: Option<&str>,
    anchor: Option<&str>,
    before: bool,
) -> bool {
    if src_manual != target_manual {
        return false;
    }
    if anchor == Some(src_key) {
        return true;
    }
    let tabs: Vec<(&str, Option<&str>)> = rows
        .iter()
        .filter_map(|r| match r {
            StripRow::Tab { tab_key, manual_id, .. } => Some((tab_key.as_str(), manual_id.as_deref())),
            _ => None,
        })
        .collect();
    let Some(me) = tabs.iter().position(|(k, _)| *k == src_key) else { return false };
    if before {
        tabs.get(me + 1).is_some_and(|(k, m)| Some(*k) == anchor && *m == target_manual)
    } else {
        me > 0 && tabs.get(me - 1).is_some_and(|(k, m)| Some(*k) == anchor && *m == target_manual)
    }
}

fn attach_tab_shifts(tab_key: &str, group_key: Option<&str>, geometry: &Geometry, mut hit: Hit) -> Hit {
    let Some(indicator) = &hit.indicator else {
        return hit;
    };
    let rows = &geometry.rows;
    let Some(me) = rows.iter().position(|r| matches!(r, StripRow::Tab { tab_key: k, .. } if k == tab_key)) else {
        return hit;
    };
    let src = &rows[me];
    let stride = (src.bottom() - src.top()) + if group_key.is_some() { 2.0 } else { 3.0 };
    let src_center = (src.top() + src.bottom()) / 2.0;
    let src_indented = group_key.is_some();

    let (boundary, lane_shift, visual_y) = match indicator {
        Indicator::CombineTab { tab_key: target, source_before, .. } => {
            let b = rows
                .iter()
                .find(|r| matches!(r, StripRow::Tab { tab_key: k, .. } if k == target))
                .map(|r| if *source_before { r.top() - 0.5 } else { r.bottom() + 0.5 });
            (b, (!src_indented).then_some(LaneShift::In), None)
        }
        Indicator::JoinGroup { group_key: gk, .. } => {
            let b = rows
                .iter()
                .find(|r| matches!(r, StripRow::Header { group_key: k, .. } if k == gk))
                .map(|r| r.bottom() + 0.5);
            (b, (!src_indented).then_some(LaneShift::In), None)
        }
        Indicator::Line { y, indented, .. } => {
            let vy = if *y < src_center { *y + stride * 0.5 } else { *y - stride * 0.5 }.max(3.0);
            let lane = if *indented == src_indented {
                None
            } else if *indented {
                Some(LaneShift::In)
            } else {
                Some(LaneShift::Out)
            };
            (Some(*y), lane, Some(vy))
        }
    };

    if let Some(bound) = boundary {
        for r in rows {
            if matches!(r, StripRow::Tab { tab_key: k, .. } if k == tab_key) {
                continue;
            }
            let c = (r.top() + r.bottom()) / 2.0;
            let shift = if bound < src_center {
                if bound < c && c < src_center { stride } else { 0.0 }
            } else if src_center < c && c < bound {
                -stride
            } else {
                0.0
            };
            if shift == 0.0 {
                continue;
            }
            match r {
                StripRow::Header { group_key: gk, .. } => _ = hit.shifts.header_shift_y.insert(gk.clone(), shift),
                StripRow::Tab { tab_key: tk, .. } => _ = hit.shifts.tab_shift_y.insert(tk.clone(), shift),
            }
        }
    }
    hit.shifts.lane_shift = lane_shift;
    if let (Some(Indicator::Line { visual_y: vy, .. }), Some(next_vy)) = (&mut hit.indicator, visual_y) {
        *vy = next_vy;
    }
    hit
}

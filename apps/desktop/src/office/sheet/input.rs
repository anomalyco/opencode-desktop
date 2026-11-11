//! The grid's pointer and keys (`spreadsheet.tsx` `press`, `move`, `release`, `key`, `copy`), and
//! what the window does once the workbook's thread answered (a copy, a toast, a scroll).

use rune::{Drag, El, KeyEvent, Phase, Tx};

use super::pointer::PointerEvent;
use super::select::Cell;
use super::state::{UiEffect, hit};
use super::{HEADER_H, Labels, track};
use crate::App;
use crate::office::registry;

/// The grid's pointer gesture, on its scroll view (points from its top-left, as on screen): a
/// press selects a cell, a row, a column or everything (Shift extends), a drag moves the far
/// corner and scrolls the sheet toward a pointer past the cells' edge, and the press takes the
/// keyboard.
pub fn pointer(grid: El, id: u64, key: &str) -> El {
    let key = key.to_owned();
    grid.on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| {
        let (changed, effect) = match d.phase {
            Phase::Down => {
                tx.focus(key.clone(), false);
                let hit = locate(id, d.x, d.y, false).map(|(cell, spot)| hit(cell, spot));
                // Ctrl or Cmd changes nothing: a click follows a link either way (`press`).
                registry::pointer(id, PointerEvent::Down { shift: tx.mods().shift, hit })
            }
            Phase::Move => {
                chase(id, d.x, d.y, tx);
                let focus = registry::hit_target(id).map_or([0, 0], |(_, _, s)| s.focus);
                registry::pointer(id, PointerEvent::Moved { cell: locate(id, d.x, d.y, true).map(|(c, _)| c), focus })
            }
            Phase::Up if d.cancelled() => registry::pointer(id, PointerEvent::Cancelled),
            Phase::Up => registry::pointer(id, PointerEvent::Up { cell: locate(id, d.x, d.y, false).map(|(c, _)| c) }),
        };
        if changed {
            tx.instant();
        }
        if let Some(effect) = effect {
            run(s, effect, tx);
        }
    })
}

/// The grid's keys: Ctrl+C copies, Excel's keys move the selection.
pub fn keys(grid: El, id: u64, labels: Labels) -> El {
    grid.on_key(move |s: &mut App, k: &KeyEvent, tx: &mut Tx| {
        if matches!(k.key, rune::Key::Command(c) if c.eq_ignore_ascii_case(&'c')) && !k.mods.alt {
            if let Some(effect) = registry::copy(id, &labels.too_large, &labels.failed) {
                run(s, effect, tx);
            }
            tx.focus(super::scroll_key(id), true);
            return true;
        }
        let (taken, effect) = registry::key(id, &k.key, k.mods);
        if let Some(effect) = effect {
            run(s, effect, tx);
        }
        // Any key on the grid, Shift held for a Shift-click too, shows its keyboard ring, as
        // Chromium's `:focus-visible` turns on once a key is pressed in an element the pointer
        // focused. Tab leaves the grid, so it doesn't.
        if !matches!(k.key, rune::Key::Tab) {
            tx.focus(super::scroll_key(id), true);
        }
        if taken {
            tx.instant();
        }
        taken
    })
}

/// Does what a grid asked of the window.
pub fn run(app: &mut App, effect: UiEffect, tx: &mut Tx) {
    match effect {
        UiEffect::Copy { text, html } => tx.copy_rich(rune::ClipboardContent::new(text).with_html(html)),
        UiEffect::Open(url) => {
            if app.live() {
                tx.open_url(&url).ok();
            }
        }
        UiEffect::Toast(title) => crate::status_toast::say(app, title, ""),
        UiEffect::Scroll { key, x, y } => tx.scroll_to_instant(key, x, y),
    }
}

/// The cell under a point of the scroll view, and where the point is past the headers
/// (`locate`). While dragging (`held`), a point past the cells holds to the nearest one.
fn locate(id: u64, x: f32, y: f32, held: bool) -> Option<(Cell, [f32; 2])> {
    let (frame, view, _) = registry::hit_target(id)?;
    let spot = [x - view.header_w, y - HEADER_H];
    let size = [view.size[0] - view.header_w, view.size[1] - HEADER_H];
    if !held && (spot[0] >= size[0] || spot[1] >= size[1]) {
        return None;
    }
    let g = &frame.grid;
    let along = |spot: f32, size: f32, scroll: f32, origin: f64, offsets: &[f32]| -> Option<usize> {
        let last = *offsets.last()?;
        let at = (spot.clamp(0.0, (size - 1.0).max(0.0)) + scroll - origin as f32).clamp(0.0, (last - 1.0).max(0.0));
        let tracks = offsets.len().checked_sub(1).filter(|t| *t > 0)?;
        (0..tracks).rev().find(|&p| offsets[p] <= at)
    };
    let col = along(spot[0], size[0], view.scroll[0], frame.viewport[0], &g.col_offsets)?;
    let row = along(spot[1], size[1], view.scroll[1], frame.viewport[1], &g.row_offsets)?;
    Some(([track(g.start_row, g.row_indices.as_deref(), row), track(g.start_col, g.col_indices.as_deref(), col)], spot))
}

/// A drag past the cells' edge scrolls the sheet toward the pointer, by how far past the edge it
/// is (`chase`); along the headers only along them.
fn chase(id: u64, x: f32, y: f32, tx: &mut Tx) {
    let Some((_, view, _)) = registry::hit_target(id) else { return };
    let past = |point: f32, start: f32, end: f32| (point - start).min(0.0) + (point - end).max(0.0);
    let axis = registry::drag_axis(id);
    let dx = if axis == Some(super::pointer::Axis::Rows) { 0.0 } else { past(x, view.header_w, view.size[0]) };
    let dy = if axis == Some(super::pointer::Axis::Columns) { 0.0 } else { past(y, HEADER_H, view.size[1]) };
    if dx != 0.0 || dy != 0.0 {
        tx.scroll_to_instant(super::scroll_key(id), (view.scroll[0] + dx).max(0.0), (view.scroll[1] + dy).max(0.0));
    }
}

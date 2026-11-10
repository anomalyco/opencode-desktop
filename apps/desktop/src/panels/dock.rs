//! The bottom pane across the session screen (`SessionPane::Bottom`), and the inset it reserves.

use rune::{Cursor, Cx, Drag, El, Phase, Tx, col};

use super::SessionPane;
use super::state::BOTTOM_MIN_H;
use super::view::pane_box;
use crate::palette::{Elevate as _, Pal};
use crate::{App, Route};

/// Extra bottom inset reserved inside `window` when the bottom pane is open, so `panel` and
/// `side-column` sit cleanly above it without changing `window/panel`'s element path.
pub fn bottom_reserve_height(s: &App, cx: &mut Cx<App>) -> f32 {
    if !matches!(s.route, Route::Session { .. }) || s.settings.open {
        return 0.0;
    }
    if let Some(reg) = s.resolve_side_region() {
        let has_bottom = !reg.pane_entries(SessionPane::Bottom).is_empty()
            || s.side
                .adding
                .iter()
                .any(|n| n.session_id == reg.session_id && n.pane == SessionPane::Bottom);
        if has_bottom && s.bottom_open() {
            return bottom_pane_height(s, cx) + 8.0;
        }
    }
    0.0
}

/// The bottom pane's height: the session's own once dragged, else Desktop's 280.
fn bottom_pane_height(s: &App, _cx: &mut Cx<App>) -> f32 {
    s.bottom_height()
}

/// The tallest a drag takes the bottom pane: 70% of the window (`window.innerHeight * 0.7`).
fn bottom_pane_max(s: &App, cx: &mut Cx<App>) -> f32 {
    (cx.layout_size("zoom").map_or(820.0, |[_, h]| h / s.zoom) * 0.7).round()
}

/// The bottom pane across the session screen, while any tab is in it.
pub fn bottom_dock(s: &App, p: &Pal, cx: &mut Cx<App>) -> Option<El> {
    if !matches!(s.route, Route::Session { .. }) || s.settings.open {
        return None;
    }

    let reg = s.resolve_side_region()?;
    let has_bottom = !reg.pane_entries(SessionPane::Bottom).is_empty()
        || s.side
            .adding
            .iter()
            .any(|n| n.session_id == reg.session_id && n.pane == SessionPane::Bottom);
    if !has_bottom {
        return None;
    }
    let open = s.bottom_open();

    let h = bottom_pane_height(s, cx);
    let max_h = bottom_pane_max(s, cx);
    let dragging = s.side.bottom_drag_origin.is_some();
    let handle = super::view::resize_pill(p, dragging, false)
        .key("bottom-resize")
        .w_full()
        .h(8.0)
        .cursor(Cursor::ResizeRow)
        .on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| {
            tx.instant();
            let Some(sid) = s.active_session_id().map(str::to_owned) else { return };
            match d.phase {
                Phase::Down => s.side.bottom_drag_origin = Some((h, s.session_side_mut(&sid).bottom_height)),
                Phase::Move => {
                    if let Some((start, _)) = s.side.bottom_drag_origin {
                        s.session_side_mut(&sid).bottom_height = Some((start - d.dy / s.zoom).clamp(BOTTOM_MIN_H, max_h.max(BOTTOM_MIN_H)));
                    }
                }
                Phase::Up => {
                    if d.cancelled()
                        && let Some((_, stored)) = s.side.bottom_drag_origin
                    {
                        s.session_side_mut(&sid).bottom_height = stored;
                    }
                    s.side.bottom_drag_origin = None;
                }
            }
        });

    let card = col().key("bottom-panel").w_full().h(h).radius(10.0).fill(p.bg_base).elevate(p.raised)
        .on_press(|s: &mut App| {
            s.side.focused = true;
            s.side.active_pane_focus = SessionPane::Bottom;
            s.composer.focused = false;
        })
        .child(
            col()
                .key("bottom-body")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .clip_rounded(10.0)
                .child(pane_box(s, p, &reg, SessionPane::Bottom, (800.0, h), cx.env().scale, false).grow(1.0).min_h(0.0)),
        );

    // Hidden rather than unmounted, so its terminals keep their screens (`screen.tsx:544`); its
    // height flies shut and open on the side region's spring.
    let outer = col()
        .key("bottom-dock")
        .w_full()
        .h(if open { h + 16.0 } else { 0.0 })
        .clip_soft(0.0)
        .motion(super::view::SIDE_SPRING)
        .pl(8.0)
        .pr(8.0)
        .child(col().w_full().pb(8.0).child(handle).child(card));
    Some(if open { outer } else { outer.inert() })
}

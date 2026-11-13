//! The bottom pane across the session screen (`SessionPane::Bottom`), and the inset it reserves.

use rune::{Cursor, Cx, Drag, El, Phase, Tx, col};

use super::SessionPane;
use super::resize::{ResizeEffect, ResizeEvent};
use super::state::{BOTTOM_MIN_H, ResolvedRegion};
use super::view::pane_box;
use crate::palette::{Elevate as _, Pal};
use crate::{App, Route};

/// Extra bottom inset reserved inside `window` when the bottom pane is open, so `panel` and
/// `side-column` sit cleanly above it without changing `window/panel`'s element path. `reg` is the
/// routed session's side region, resolved once per view for this and `bottom_dock`.
pub fn bottom_reserve_height(s: &App, reg: Option<&ResolvedRegion>, cx: &mut Cx<App>) -> f32 {
    if !matches!(*s.route, Route::Session { .. }) || s.settings.open {
        return 0.0;
    }
    if let Some(reg) = reg {
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
    (cx.layout_size("zoom").map_or(820.0 / *s.zoom, |[_, h]| h) * 0.7).round()
}

/// The bottom pane across the session screen, while any tab is in it.
pub fn bottom_dock(s: &App, reg: Option<&ResolvedRegion>, p: &Pal, cx: &mut Cx<App>) -> Option<El> {
    if !matches!(*s.route, Route::Session { .. }) || s.settings.open {
        return None;
    }

    let reg = reg?;
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
    let handle = super::view::resize_pill(p, s.side.bottom_resize.dragging(), false)
        .key("bottom-resize")
        .w_full()
        .h(8.0)
        .cursor(Cursor::ResizeRow)
        .on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| {
            tx.instant();
            let Some(sid) = s.active_session_id().map(str::to_owned) else { return };
            let event = match d.phase {
                Phase::Down => ResizeEvent::Down { size: h, stored: s.session_side_mut(&sid).bottom_height, clicks: d.clicks },
                Phase::Move => ResizeEvent::Move { by: -d.dy },
                Phase::Up => ResizeEvent::Up { cancelled: d.cancelled() },
            };
            match s.side.bottom_resize.step(event) {
                ResizeEffect::Size(h) => s.session_side_mut(&sid).bottom_height = Some(h.clamp(BOTTOM_MIN_H, max_h.max(BOTTOM_MIN_H))),
                ResizeEffect::Restore(stored) => s.session_side_mut(&sid).bottom_height = stored,
                ResizeEffect::None => {}
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
                // The dock spans the window past the sidebar, 8 in from each side.
                .child(pane_box(s, p, reg, SessionPane::Bottom, (s.side.laid_w.get() - s.sidebar_shown_w() - 16.0, h), cx.env().scale, false).grow(1.0).min_h(0.0)),
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

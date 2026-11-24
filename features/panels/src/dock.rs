//! The bottom pane across the session screen (`SessionPane::Bottom`), and the inset it reserves.

use rune::{Cursor, Cx, Drag, El, Phase, Tx, col};
use opencode_ui::palette::{Elevate as _, Pal};

use crate::SessionPane;
use crate::host::PanelsHost;
use crate::resize::{ResizeEffect, ResizeEvent};
use crate::state::{BOTTOM_MIN_H, ResolvedRegion, bottom_open, session_side_mut};
use crate::tabs::session_tab_pane;
use crate::view::{SIDE_SPRING, pane_box, resize_pill};
use crate::width::bottom_height;

/// Pinned tabs always live at the side (`resolve_side_region_now`). A bottom tab must
/// therefore be stored in that pane, or be a New tab in it. Only ask providers then;
/// an empty dock doesn't need the Review file count or a terminal's title.
pub fn bottom_has_candidate(host: &dyn PanelsHost) -> bool {
    let Some(id) = host.panel_session_id() else { return false };
    let side = host.side();
    side.adding.iter().any(|n| n.session_id == id && n.pane == SessionPane::Bottom)
        || side.sessions.get(id).is_some_and(|s| {
            let tabs = &s.tab_state.tabs;
            tabs.all.iter().any(|k| session_tab_pane(tabs, k) == SessionPane::Bottom)
        })
}

/// Extra bottom inset reserved inside `window` when the bottom pane is open, so `panel` and
/// `side-column` sit cleanly above it without changing `window/panel`'s element path. `reg` is the
/// routed session's side region, resolved once per view for this and [`bottom_dock`].
pub fn bottom_reserve_height(host: &dyn PanelsHost, reg: Option<&ResolvedRegion>) -> f32 {
    if host.panel_session_id().is_none() || host.settings_open() {
        return 0.0;
    }
    if let Some(reg) = reg {
        let has_bottom = !reg.pane_entries(SessionPane::Bottom).is_empty()
            || host
                .side()
                .adding
                .iter()
                .any(|n| n.session_id == reg.session_id && n.pane == SessionPane::Bottom);
        if has_bottom && bottom_open(host) {
            return bottom_height(host) + 8.0;
        }
    }
    0.0
}

/// The tallest a drag takes the bottom pane: 70% of the window (`window.innerHeight * 0.7`).
fn bottom_pane_max<H: PanelsHost + 'static>(host: &H, cx: &Cx<H>) -> f32 {
    (cx.layout_size("zoom").map_or(820.0 / host.zoom(), |[_, h]| h) * 0.7).round()
}

/// The bottom pane across the session screen, while any tab is in it.
pub fn bottom_dock<H: PanelsHost + 'static>(
    host: &H,
    reg: Option<&ResolvedRegion>,
    p: &Pal,
    cx: &mut Cx<H>,
) -> Option<El> {
    if host.panel_session_id().is_none() || host.settings_open() {
        return None;
    }

    let reg = reg?;
    let has_bottom = !reg.pane_entries(SessionPane::Bottom).is_empty()
        || host
            .side()
            .adding
            .iter()
            .any(|n| n.session_id == reg.session_id && n.pane == SessionPane::Bottom);
    if !has_bottom {
        return None;
    }
    let open = bottom_open(host);

    let h = bottom_height(host);
    let max_h = bottom_pane_max(host, cx);
    let handle = resize_pill(p, host.side().bottom_resize.dragging(), false)
        .key("bottom-resize")
        .w_full()
        .h(8.0)
        .cursor(Cursor::ResizeRow)
        .on_drag(move |s: &mut H, d: &mut Drag, tx: &mut Tx| on_bottom_resize_drag(s, h, max_h, d, tx));

    let pane_w = host.side().laid_w.get() - host.sidebar_shown_w() - 16.0;
    let body = col()
        .key("bottom-body")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .clip_rounded(10.0)
        // The dock spans the window past the sidebar, 8 in from each side.
        .child(pane_box(host, p, reg, SessionPane::Bottom, (pane_w, h), cx.env().scale, false).grow(1.0).min_h(0.0));

    let card = col()
        .key("bottom-panel")
        .w_full()
        .h(h)
        .radius(10.0)
        .fill(p.bg_base)
        .elevate(p.raised)
        .on_press(|s: &mut H| focus_bottom_pane(s))
        .child(body);

    // Hidden rather than unmounted, so its terminals keep their screens (`screen.tsx:544`); its
    // height flies shut and open on the side region's spring.
    let outer = col()
        .key("bottom-dock")
        .w_full()
        .h(if open { h + 16.0 } else { 0.0 })
        .clip_soft(0.0)
        .motion(SIDE_SPRING)
        .pl(8.0)
        .pr(8.0)
        .child(col().w_full().pb(8.0).child(handle).child(card));
    Some(if open { outer } else { outer.inert() })
}

fn on_bottom_resize_drag(host: &mut dyn PanelsHost, h: f32, max_h: f32, d: &Drag, tx: &mut Tx) {
    tx.instant();
    let Some(sid) = host.panel_session_id().map(str::to_owned) else { return };
    let stored = host.side().sessions.get(&sid).and_then(|st| st.bottom_height);
    let event = match d.phase {
        Phase::Down => ResizeEvent::Down { size: h, stored, clicks: d.clicks },
        Phase::Move => ResizeEvent::Move { by: -d.dy },
        Phase::Up => ResizeEvent::Up { cancelled: d.cancelled() },
    };
    match host.side_mut().bottom_resize.step(event) {
        ResizeEffect::Size(next_h) => {
            session_side_mut(host, &sid).bottom_height = Some(next_h.clamp(BOTTOM_MIN_H, max_h.max(BOTTOM_MIN_H)));
        }
        ResizeEffect::Restore(stored) => session_side_mut(host, &sid).bottom_height = stored,
        ResizeEffect::None => {}
    }
}

fn focus_bottom_pane(host: &mut dyn PanelsHost) {
    {
        let side = host.side_mut();
        side.focus.focus();
        side.active_pane_focus = SessionPane::Bottom;
    }
    host.set_composer_focused(false);
}

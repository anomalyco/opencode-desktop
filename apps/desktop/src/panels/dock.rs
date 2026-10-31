//! `PanelRegion::Dock` card, vertical resize handle, and `DockPlacement::Bottom` dock rendering.

use rune::{Anim, Cursor, Cx, Drag, El, Phase, Tx, col, el};

use super::state::{
    BOTTOM_MAX_H, BOTTOM_MIN_H, DOCK_COLLAPSE_H, DOCK_MAX_H, DOCK_MIN_H, ResolvedEntry,
};
use super::view::pane_box;
use super::{PanelRegion, PanelViewCtx, SessionPane};
use crate::palette::Pal;
use crate::{App, Route};

const INSTANT: Anim = Anim::INSTANT;

/// Extra bottom inset reserved inside `window` when `bottom_dock` is open, so `panel` and
/// `side-column` sit cleanly above it without changing `window/panel`'s element path.
pub fn bottom_reserve_height(s: &App, cx: &mut Cx<App>) -> f32 {
    if !matches!(s.route, Route::Session(_)) || s.settings.open {
        return 0.0;
    }
    if s.dock_bottom_open() && s.resolve_dock_entry().is_some() {
        let win_h = cx.layout_size("window").map_or(820.0 / s.zoom, |[_, h]| h);
        let max_h = (win_h * 0.6).clamp(DOCK_MIN_H, DOCK_MAX_H);
        return s.side.dock_height.clamp(DOCK_MIN_H, max_h) + 8.0;
    }
    if let Some(reg) = s.resolve_side_region() {
        let has_bottom = !reg.pane_entries(SessionPane::Bottom).is_empty()
            || s.side
                .adding
                .iter()
                .any(|n| n.session_id == reg.session_id && n.pane == SessionPane::Bottom);
        if has_bottom && s.bottom_open() {
            return s.side.bottom_height + 8.0;
        }
    }
    0.0
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_dock_card(
    s: &App,
    p: &Pal,
    session_id: &str,
    directory: &str,
    project_id: &str,
    entry: &ResolvedEntry,
    pane_size: (f32, f32),
    env_scale: f32,
    reserve_top_right: bool,
) -> El {
    let vctx = PanelViewCtx {
        session_id,
        directory,
        project_id,
        region: PanelRegion::Dock,
        pane: SessionPane::Side,
        pane_size,
        env_scale,
        tab: &entry.tab,
        visible: s.dock_open(),
        focused: s.side.dock_focused,
        reserve_top_right,
    };
    let content = (entry.provider.render)(s, p, &vctx);
    col()
        .radius(10.0)
        .fill(p.bg_base)
        .shadow(2.0, 4.0, p.shadow)
        .shadow(1.0, 2.0, p.shadow)
        .on_press(|s: &mut App| {
            s.side.dock_focused = true;
            s.side.focused = false;
            s.composer.focused = false;
        })
        .child(
            col()
                .key("dock-body")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .clip_rounded(10.0)
                .instant_changes()
                .child(content),
        )
        .child(
            el().key("dock-ring")
                .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .radius(10.0)
                .border(0.5, p.ring)
                .inert(),
        )
}

pub(super) fn dock_resize_handle(s: &App, p: &Pal, key: &'static str, max_h: f32) -> El {
    let dragging = s.side.dock_drag_origin.is_some();
    el().key(key)
        .w_full()
        .h(8.0)
        .center()
        .cursor(Cursor::ResizeRow)
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| {
            tx.instant();
            match d.phase {
                Phase::Down => s.side.dock_drag_origin = Some(s.side.dock_height),
                Phase::Move => {
                    if let Some(orig) = s.side.dock_drag_origin {
                        let raw = orig - d.dy / s.zoom;
                        if raw < DOCK_COLLAPSE_H {
                            s.side.dock_drag_origin = None;
                            s.close_dock(tx);
                        } else {
                            s.side.dock_height = raw.clamp(DOCK_MIN_H, max_h);
                        }
                    }
                }
                Phase::Up => {
                    if d.cancelled()
                        && let Some(orig) = s.side.dock_drag_origin
                    {
                        s.side.dock_height = orig;
                    }
                    s.side.dock_drag_origin = None;
                }
            }
        })
        .child(
            el().size(40.0, 4.0)
                .radius(2.0)
                .fill(if dragging { p.icon } else { p.icon_muted })
                .group_opacity(if dragging { 1.0 } else { 0.0 }, 1.0)
                .transition(rune::Transition::none()),
        )
}

/// Bottom dock across the session screen: renders `PanelRegion::Dock` when `terminalPlacement == "bottom"`
/// (or `SessionPane::Bottom` if tabs were moved there).
pub fn bottom_dock(s: &App, p: &Pal, cx: &mut Cx<App>) -> Option<El> {
    if !matches!(s.route, Route::Session(_)) || s.settings.open {
        return None;
    }

    if s.dock_placement() == super::DockPlacement::Bottom
        && (s.dock_bottom_open() || s.side.dock_mounted)
        && let Some((sid, dir, proj, dentry)) = s.resolve_dock_entry()
    {
        let open = s.dock_bottom_open();
        let env_scale = cx.env().scale;
        let [win_w, win_h] = cx
            .layout_size("window")
            .unwrap_or([1280.0 / s.zoom, 820.0 / s.zoom]);
        let left_w = if s.sidebar_open { s.sidebar_w } else { 0.0 };
        let max_h = (win_h * 0.6).clamp(DOCK_MIN_H, DOCK_MAX_H);
        let h = s.side.dock_height.clamp(DOCK_MIN_H, max_h);
        let slot_h = if open { h + 16.0 } else { 0.0 };
        let handle = dock_resize_handle(s, p, "dock-bottom-resize", max_h);
        let card = render_dock_card(
            s,
            p,
            &sid,
            &dir,
            &proj,
            &dentry,
            ((win_w - left_w - 16.0).max(200.0), h),
            env_scale,
            false,
        )
        .key("dock-bottom-panel")
        .w_full()
        .h(h);
        let mut outer = col()
            .key("bottom-dock")
            .role("region")
            .w_full()
            .h(slot_h)
            .clip_soft(0.0)
            .pl(8.0)
            .pr(8.0)
            .child(col().w_full().pb(8.0).child(handle).child(card));
        if !open {
            outer = outer.inert();
        }
        return Some(outer);
    }

    let reg = s.resolve_side_region()?;
    let has_bottom = !reg.pane_entries(SessionPane::Bottom).is_empty()
        || s.side
            .adding
            .iter()
            .any(|n| n.session_id == reg.session_id && n.pane == SessionPane::Bottom);
    if !has_bottom || !s.bottom_open() {
        return None;
    }

    let h = s.side.bottom_height;
    let dragging = s.side.bottom_drag_origin.is_some();
    let handle = el()
        .key("bottom-resize")
        .w_full()
        .h(8.0)
        .center()
        .cursor(Cursor::ResizeRow)
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .on_drag(|s: &mut App, d: &mut Drag, tx: &mut Tx| {
            tx.instant();
            match d.phase {
                Phase::Down => s.side.bottom_drag_origin = Some(s.side.bottom_height),
                Phase::Move => {
                    if let Some(orig) = s.side.bottom_drag_origin {
                        s.side.bottom_height =
                            (orig - d.dy / s.zoom).clamp(BOTTOM_MIN_H, BOTTOM_MAX_H);
                    }
                }
                Phase::Up => {
                    if d.cancelled()
                        && let Some(orig) = s.side.bottom_drag_origin
                    {
                        s.side.bottom_height = orig;
                    }
                    s.side.bottom_drag_origin = None;
                }
            }
        })
        .child(
            el().size(40.0, 4.0)
                .radius(2.0)
                .fill(if dragging { p.icon } else { p.icon_muted })
                .group_opacity(if dragging { 1.0 } else { 0.0 }, 1.0)
                .transition(rune::Transition::none()),
        );

    let card = col()
        .key("bottom-panel")
        .w_full()
        .h(h)
        .radius(10.0)
        .fill(p.bg_base)
        .shadow(2.0, 4.0, p.shadow)
        .shadow(1.0, 2.0, p.shadow)
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
        )
        .child(
            el().key("bottom-ring")
                .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .radius(10.0)
                .border(0.5, p.ring)
                .inert(),
        );

    Some(col().key("bottom-dock").w_full().pl(8.0).pr(8.0).pb(8.0).child(handle).child(card))
}

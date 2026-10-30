//! Side region, split pane, bottom dock, tab strip, New Tab launchers, and `Recently closed` UI
//! (`packages/app/src/runtime/plugin/side-region.tsx`, `closed-tabs-view.tsx`).

use rune::{
    Anim, AnimExt, Color, Curve, Cursor, Cx, Drag, Easing, El, Icon, Phase, PointerPress,
    ScrollSpec, Spring, Tx, col, el, icon, row, text,
};
use rune_theme_opencode::icons as ic;

pub use super::dock::{bottom_dock, bottom_reserve_height};
use super::dock::{dock_resize_handle, render_dock_card};
use super::menus::new_tab_view;
pub use super::menus::context_menus;
use super::state::{
    DOCK_MAX_H, DOCK_MIN_H, ResolvedEntry, ResolvedRegion, SIDE_DEFAULT_W, SIDE_MAX_W, SIDE_MIN_W,
    SPLIT_MIN_H,
};
use super::tabs::SessionTabs;
use super::{CloseButtonMode, PanelRegion, PanelViewCtx, SessionPane};
use crate::palette::Pal;
use crate::view::{UI, UI_MED, UI_SMALL};
use crate::{App, Route};

/// Shared panel spring matching `view/sidebar.rs::SIDEBAR_SPRING` and `panel-motion.ts`.
pub const SIDE_SPRING: Spring = Spring::new(0.3, 0.0);
const INSTANT: Anim = Anim::INSTANT;
const FADE_FLOOR: f32 = 0.5;

static LAYOUT_RIGHT: Icon = Icon {
    name: "layout-right",
    view: 20.0,
    body: r#"<g transform="matrix(-1 0 0 1 20 0)"><path d="M2.917 2.916H17.084V17.083H2.917ZM7.917 2.916V17.083" stroke="currentColor" stroke-linecap="square"/></g>"#,
};

static LAYOUT_RIGHT_FULL: Icon = Icon {
    name: "layout-right-full",
    view: 20.0,
    body: r#"<g transform="matrix(-1 0 0 1 20 0)"><path d="M2.917 2.916H7.917V17.083H2.917Z" fill="currentColor"/><path d="M2.917 2.916H17.084V17.083H2.917ZM7.917 2.916V17.083" stroke="currentColor" stroke-linecap="square"/></g>"#,
};

fn pane_slug(pane: SessionPane) -> &'static str {
    match pane {
        SessionPane::Side => "side",
        SessionPane::Split => "split",
        SessionPane::Bottom => "bottom",
    }
}

/// Right-hand column (`Side` tabs over `Dock` when `terminalPlacement == "side"`), animated on `SIDE_SPRING`.
pub fn side_region(s: &App, p: &Pal, zoomed_w: f32, bottom_reserve: f32, cx: &mut Cx<App>) -> El {
    let on_session = matches!(s.route, Route::Session(_)) && !s.settings.open;
    let reg = if on_session { s.resolve_side_region() } else { None };
    let dock_entry = if on_session { s.resolve_dock_entry() } else { None };
    let wide = reg.as_ref().is_some_and(|r| r.wide);
    let left_w = if s.sidebar_open { s.sidebar_w } else { 0.0 };
    let w = s.side.effective_width(zoomed_w, left_w, wide);

    let tabs_open = on_session && s.side_open();
    let dock_side_open = on_session && s.dock_side_open() && dock_entry.is_some();
    let col_open = tabs_open || dock_side_open;
    let slot_w = if col_open { w + 8.0 } else { 0.0 };

    let mut outer = el()
        .key("side-region")
        .role("complementary")
        .w(slot_w)
        .h_full()
        .clip_soft(0.0)
        .motion(SIDE_SPRING);

    if let Some(reg) = reg.as_ref() {
        let env_scale = cx.env().scale;
        let col_h = if col_open || s.side.mounted {
            cx.layout_size("side-column")
                .map_or(720.0 / s.zoom, |[_, h]| h / s.zoom)
        } else {
            720.0 / s.zoom
        };
        let max_dock_h = (col_h * 0.6).clamp(DOCK_MIN_H, DOCK_MAX_H);
        let dock_h = s.side.dock_height.clamp(DOCK_MIN_H, max_dock_h);
        let stacked = tabs_open && dock_side_open;

        let show_solo_dock = !tabs_open
            && s.dock_placement() == super::DockPlacement::Side
            && (dock_side_open || (s.side.dock_last_solo && dock_entry.is_some()));
        let col_bottom = if col_open { 8.0 + bottom_reserve } else { 8.0 };

        let mut column = col()
            .key("side-column")
            .pin(Some(0.0), Some(8.0), None, Some(col_bottom))
            .w(w)
            .opacity(if col_open { 1.0 } else { FADE_FLOOR })
            .opacity_motion(SIDE_SPRING);

        if tabs_open || !show_solo_dock {
            let side_h = if stacked { (col_h - dock_h - 8.0).max(120.0) } else { col_h };
            let mut card = region_card(s, p, reg, w, side_h, env_scale)
                .key("side-panel")
                .grow(1.0)
                .min_h(0.0);
            if !tabs_open {
                card = card.inert();
            }
            column = column.child(card);
        }

        if show_solo_dock
            && let Some((ref sid, ref dir, ref proj, ref dentry)) = dock_entry
        {
            let mut dock_card = render_dock_card(
                s,
                p,
                sid,
                dir,
                proj,
                dentry,
                (w, col_h),
                env_scale,
                true,
            )
            .key("dock-panel")
            .w(w)
            .grow(1.0)
            .min_h(0.0);
            if !dock_side_open {
                dock_card = dock_card.inert();
            }
            column = column.child(dock_card);
        } else if s.dock_placement() == super::DockPlacement::Side
            && (dock_side_open || s.side.dock_mounted)
            && let Some((ref sid, ref dir, ref proj, ref dentry)) = dock_entry
        {
            let slot_open = tabs_open && dock_side_open;
            let slot_h = if slot_open { dock_h + 8.0 } else { 0.0 };
            let mut slot = col()
                .key("dock-side-slot")
                .role("region")
                .w(w)
                .h(slot_h)
                .clip_soft(0.0)
                .motion(SIDE_SPRING)
                .child(dock_resize_handle(s, p, "dock-side-resize", max_dock_h))
                .child(
                    render_dock_card(
                        s,
                        p,
                        sid,
                        dir,
                        proj,
                        dentry,
                        (w, dock_h),
                        env_scale,
                        false,
                    )
                    .key("dock-panel")
                    .w(w)
                    .h(dock_h),
                );
            if !slot_open {
                slot = slot.inert();
            }
            column = column.child(slot);
        }

        if !col_open {
            column = column.inert();
        }
        outer = outer.child(column);
    }
    outer
}

/// Standalone card for storybook stages (`side_stories.rs`).
pub fn story_card(s: &App, p: &Pal, width: f32, height: f32) -> El {
    let Some(reg) = s.resolve_side_region() else {
        return el().key("side-panel").size(width, height);
    };
    region_card(s, p, &reg, width, height, 2.0)
        .key("side-panel")
        .size(width, height)
}

fn region_card(s: &App, p: &Pal, reg: &ResolvedRegion, width: f32, col_h: f32, env_scale: f32) -> El {
    let has_split = !reg.pane_entries(SessionPane::Split).is_empty()
        || s.side
            .adding
            .iter()
            .any(|n| n.session_id == reg.session_id && n.pane == SessionPane::Split);
    let has_side = !reg.pane_entries(SessionPane::Side).is_empty()
        || s.side
            .adding
            .iter()
            .any(|n| n.session_id == reg.session_id && n.pane == SessionPane::Side);

    let panes: &[SessionPane] = match (has_side, has_split) {
        (_, false) => &[SessionPane::Side],
        (false, true) => &[SessionPane::Split],
        (true, true) => &[SessionPane::Side, SessionPane::Split],
    };

    let split_h = (col_h * (1.0 - s.side.split_ratio)).clamp(SPLIT_MIN_H, (col_h - SPLIT_MIN_H).max(SPLIT_MIN_H));

    let mut body = col()
        .key("side-body")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .clip_rounded(10.0)
        .instant_changes()
        .on_press(|s: &mut App| {
            s.side.focused = true;
            s.side.dock_focused = false;
            s.composer.focused = false;
            s.side.tab_menu = None;
            s.side.add_menu = None;
        });

    for (idx, &pane) in panes.iter().enumerate() {
        if idx > 0 {
            body = body.child(split_divider(s, p, col_h));
        }
        let ph = if panes.len() == 1 {
            col_h
        } else if idx == 0 {
            (col_h - split_h - 6.0).max(SPLIT_MIN_H)
        } else {
            split_h
        };
        let pane_el = pane_box(s, p, reg, pane, (width, ph), env_scale, idx == 0);
        let sized = if idx == 0 {
            pane_el.grow(1.0).min_h(0.0)
        } else {
            pane_el.h(split_h)
        };
        body = body.child(sized);
    }

    col()
        .w(width)
        .radius(10.0)
        .fill(p.bg_base)
        .shadow(2.0, 4.0, p.shadow)
        .shadow(1.0, 2.0, p.shadow)
        .child(body)
        .child(
            el().key("side-ring")
                .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .radius(10.0)
                .border(0.5, p.ring)
                .inert(),
        )
}

fn split_divider(s: &App, p: &Pal, col_h: f32) -> El {
    let dragging = s.side.split_drag_origin.is_some();
    el().key("side-split-resize")
        .w_full()
        .h(6.0)
        .center()
        .cursor(Cursor::ResizeRow)
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| {
            tx.instant();
            match d.phase {
                Phase::Down => s.side.split_drag_origin = Some(s.side.split_ratio),
                Phase::Move => {
                    if let Some(orig) = s.side.split_drag_origin
                        && col_h > 1.0
                    {
                        s.side.split_ratio = (orig + (d.dy / s.zoom) / col_h).clamp(0.2, 0.8);
                    }
                }
                Phase::Up => {
                    if d.cancelled()
                        && let Some(orig) = s.side.split_drag_origin
                    {
                        s.side.split_ratio = orig;
                    }
                    s.side.split_drag_origin = None;
                }
            }
        })
        .child(el().w_full().h(0.5).fill(if dragging { p.border } else { p.border_muted }))
}

pub(super) fn pane_box(
    s: &App,
    p: &Pal,
    reg: &ResolvedRegion,
    pane: SessionPane,
    pane_size: (f32, f32),
    env_scale: f32,
    reserve_toggle: bool,
) -> El {
    let empty_tabs = SessionTabs::default();
    let stored_state = s.side.sessions.get(&reg.session_id);
    let stored = stored_state.map_or(&empty_tabs, |st| &st.tab_state.tabs);
    let preview_key = stored_state.and_then(|st| st.tab_state.preview.as_deref());
    let pane_active = reg.pane_active(pane, stored);
    let shown_new = s.shown_new_tab(&reg.session_id, pane, pane_active.as_deref());
    let live_new = s.live_new_tabs(&reg.session_id, pane, pane_active.as_deref());
    let entries = reg.pane_entries(pane);

    let bar = tab_bar(
        p,
        pane,
        &entries,
        pane_active.as_deref(),
        preview_key,
        shown_new.map(|n| n.id.as_str()),
        &live_new,
        reserve_toggle && s.stage.is_none(),
    );

    let content = if shown_new.is_some() {
        new_tab_view(s, p, reg, pane)
    } else if let Some(act_key) = pane_active.as_deref()
        && let Some(selected) = reg.entry(act_key)
    {
        let visible = match pane {
            SessionPane::Bottom => s.bottom_open(),
            _ => s.side_open() || s.stage.is_some(),
        };
        let bar_h = if pane == SessionPane::Side { 48.5 } else { 38.5 };
        let vctx = PanelViewCtx {
            session_id: &reg.session_id,
            directory: &reg.directory,
            project_id: &reg.project_id,
            region: PanelRegion::Side,
            pane,
            pane_size: (pane_size.0, (pane_size.1 - bar_h).max(1.0)),
            env_scale,
            tab: &selected.tab,
            visible,
            focused: s.side.focused && s.side.active_pane_focus == pane,
            reserve_top_right: false,
        };
        let panel_el = (selected.provider.render)(s, p, &vctx);
        let content_key = match selected.tab.group {
            Some(g) => format!("panel-{}-{g}", selected.plugin),
            None => format!("panel-{}", selected.key),
        };
        col()
            .key(content_key)
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .child(panel_el)
    } else {
        empty_pane_view(p)
    };

    col()
        .key(format!("pane-{}", pane_slug(pane)))
        .w_full()
        .min_w(0.0)
        .min_h(0.0)
        .on_press(move |s: &mut App| {
            s.side.focused = true;
            s.side.dock_focused = false;
            s.side.active_pane_focus = pane;
            s.composer.focused = false;
        })
        .child(bar)
        .child(el().w_full().h(0.5).fill(p.border_muted))
        .child(content)
}

#[allow(clippy::too_many_arguments)]
fn tab_bar(
    p: &Pal,
    pane: SessionPane,
    entries: &[&ResolvedEntry],
    pane_active: Option<&str>,
    preview_key: Option<&str>,
    shown_new_id: Option<&str>,
    live_new: &[&super::state::NewTabEntry],
    reserve_toggle: bool,
) -> El {
    let mut strip = row()
        .key(format!("side-tabs-{}", pane_slug(pane)))
        .gap(4.0)
        .center_y()
        .grow(1.0)
        .min_w(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .clip_fade(0.0, 0.0, 12.0, 0.0);

    for entry in entries {
        let key = entry.key.clone();
        let active = shown_new_id.is_none() && pane_active == Some(key.as_str());
        let is_preview = preview_key == Some(key.as_str());
        strip = strip.child(tab_trigger(p, entry, active, is_preview));
    }

    for nt in live_new {
        let nid = nt.id.clone();
        let close_nid = nt.id.clone();
        let active = shown_new_id == Some(nid.as_str());
        let trigger = row()
            .key(format!("side-new-{}", nid))
            .role("tab")
            .label("New tab")
            .h(28.0)
            .pl(8.0)
            .pr(4.0)
            .gap(6.0)
            .center_y()
            .radius(6.0)
            .hover_group()
            .fill(if active { p.pressed } else { Color::TRANSPARENT })
            .hover(p.hover)
            .hover_motion(INSTANT, INSTANT)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.side.chosen_new_tab.insert(pane, nid.clone());
                s.side.focused = true;
                s.side.active_pane_focus = pane;
            })
            .child(
                text("New tab")
                    .font(if active { UI_MED } else { UI })
                    .color(if active { p.text } else { p.muted })
                    .truncate_fade(),
            )
            .child(
                el().key(format!("side-new-close-{}", close_nid))
                    .role("button")
                    .label("Close tab")
                    .size(18.0, 18.0)
                    .radius(4.0)
                    .center()
                    .hover(p.hover)
                    .hover_motion(INSTANT, INSTANT)
                    .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                        tx.instant();
                        s.dismiss_side_new_tab(&close_nid);
                    })
                    .child(
                        icon(&ic::XMARK_SMALL, 12.0)
                            .color(if active { p.faint } else { Color::TRANSPARENT })
                            .group_color(p.icon_muted),
                    ),
            );
        strip = strip.child(trigger);
    }

    // `+` button right after the tabs, as in Desktop's `side-region.tsx`.
    strip = strip.child(
        el().key(format!("side-add-{}", pane_slug(pane)))
            .role("button")
            .label("New tab")
            .size(28.0, 28.0)
            .radius(6.0)
            .center()
            .hover(p.hover)
            .press(p.pressed)
            .hover_motion(INSTANT, INSTANT)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.open_side_new_tab(pane);
            })
            .on_secondary_press(move |s: &mut App, ev: PointerPress, tx: &mut Tx| {
                tx.instant();
                s.side.add_menu = Some((pane, ev.x / s.zoom, ev.y / s.zoom));
            })
            .child(icon(&ic::PLUS, 14.0).color(p.icon_muted)),
    );

    let mut bar = row()
        .key(format!("side-tabbar-{}", pane_slug(pane)))
        .w_full()
        .h(if pane == SessionPane::Side { 48.0 } else { 38.0 })
        .pl(10.0)
        .pr(10.0)
        .gap(4.0)
        .center_y()
        .window_drag()
        .child(strip);

    if pane == SessionPane::Bottom {
        bar = bar.child(
            el().key("bottom-hide")
                .role("button")
                .label("Hide bottom panel (⌘J)")
                .size(28.0, 28.0)
                .radius(6.0)
                .center()
                .hover(p.hover)
                .press(p.pressed)
                .hover_motion(INSTANT, INSTANT)
                .on_press_tx(|s: &mut App, tx: &mut Tx| s.toggle_bottom_region(tx))
                .child(icon(&ic::CHEVRON_DOWN, 14.0).color(p.icon_muted)),
        );
    } else if reserve_toggle {
        // Reserves space at the end of the top tab bar for the floating `side-toggle` button (`size-7`).
        bar = bar.child(el().key("side-toggle-slot").size(30.0, 28.0));
    }

    bar
}

fn tab_trigger(p: &Pal, entry: &ResolvedEntry, active: bool, is_preview: bool) -> El {
    let key = entry.key.clone();
    let select_key = entry.key.clone();
    let promote_key = entry.key.clone();
    let menu_key = entry.key.clone();
    let close_key = entry.key.clone();
    let safe_id = key.replace(':', "-").replace('/', "_");

    let label_color = if entry.tab.missing {
        p.faint
    } else if active {
        p.text
    } else if is_preview {
        p.muted
    } else {
        p.muted
    };

    let mut pill = row()
        .key(format!("side-tab-{safe_id}"))
        .role("tab")
        .label(entry.tab.title.clone())
        .h(28.0)
        .max_w(188.0)
        .pl(8.0)
        .pr(if entry.tab.pinned || entry.tab.closable == CloseButtonMode::None { 8.0 } else { 4.0 })
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover_group()
        .fill(if active { p.pressed } else { Color::TRANSPARENT })
        .hover(p.hover)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            s.select_side_tab(&select_key, tx);
        })
        .on_double_press(move |s: &mut App| {
            s.promote_preview_tab(&promote_key);
        })
        .on_secondary_press(move |s: &mut App, ev: PointerPress, tx: &mut Tx| {
            tx.instant();
            s.side.tab_menu = Some((menu_key.clone(), ev.x / s.zoom, ev.y / s.zoom));
        });

    if let Some(glyph) = entry.tab.icon {
        pill = pill.child(
            icon(glyph, 14.0).color(if active { p.icon } else { p.icon_muted }),
        );
    }

    pill = pill.child(
        text(entry.tab.title.clone())
            .font(if active { UI_MED } else { UI })
            .color(label_color)
            .truncate_fade()
            .shrink(),
    );

    if let Some(badge) = &entry.tab.badge {
        pill = pill.child(text(badge.clone()).font(UI_SMALL).color(p.faint));
    }

    if !entry.tab.pinned && entry.tab.closable != CloseButtonMode::None {
        let always_visible = entry.tab.closable == CloseButtonMode::Standard || active;
        let close_btn = el()
            .key(format!("side-tab-close-{safe_id}"))
            .role("button")
            .label("Close tab")
            .size(18.0, 18.0)
            .radius(4.0)
            .center()
            .hover(p.hover)
            .hover_motion(INSTANT, INSTANT)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                s.close_side_tab(&close_key, tx);
            })
            .child(
                icon(&ic::XMARK_SMALL, 12.0)
                    .color(if always_visible { p.faint } else { Color::TRANSPARENT })
                    .group_color(p.icon_muted),
            );
        pill = pill.child(close_btn);
    }

    pill
}

fn empty_pane_view(p: &Pal) -> El {
    col()
        .key("side-empty")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .center()
        .padding(24.0)
        .gap(8.0)
        .child(text("Open a tab with + or ⌘T").font(UI).color(p.faint))
}

/// Floating top-right toggle button (`SessionReviewToggle` in `screen.tsx:408`), kept outside panel motion.
pub fn side_toggle(s: &App, p: &Pal) -> Option<El> {
    if !matches!(s.route, Route::Session(_)) || s.settings.open {
        return None;
    }
    let open = s.side_open();
    Some(
        el().key("side-toggle")
            .role("button")
            .label(if open { "Hide side panel (⌥⌘B)" } else { "Show side panel (⌥⌘B)" })
            .pin(None, Some(18.0), Some(18.0), None)
            .size(28.0, 28.0)
            .radius(6.0)
            .center()
            .hover(p.hover)
            .press(p.pressed)
            .hover_motion(INSTANT, INSTANT)
            .on_press_tx(|s: &mut App, tx: &mut Tx| s.toggle_side_region(tx))
            .child(
                icon(&LAYOUT_RIGHT_FULL, 20.0)
                    .color(p.icon_muted)
                    .absolute(4.0, 4.0)
                    .opacity(if open { 1.0 } else { 0.0 })
                    .opacity_motion(SIDE_SPRING),
            )
            .child(
                icon(&LAYOUT_RIGHT, 20.0)
                    .color(p.icon_muted)
                    .absolute(4.0, 4.0)
                    .opacity(if open { 0.0 } else { 1.0 })
                    .opacity_motion(SIDE_SPRING),
            ),
    )
}

/// Horizontal resize handle across the 8pt gap between `panel` and `side-region`.
/// Dragging tracks the pointer 1:1 (`tx.instant()`); double-clicking resets to default width on `SIDE_SPRING`.
pub fn resize_handle(s: &App, p: &Pal, zoomed_w: f32) -> Option<El> {
    if !matches!(s.route, Route::Session(_)) || s.settings.open || !s.right_column_open() || zoomed_w <= 0.0 {
        return None;
    }
    let wide = s.resolve_side_region().is_some_and(|r| r.wide);
    let left_w = if s.sidebar_open { s.sidebar_w } else { 0.0 };
    let w = s.side.effective_width(zoomed_w, left_w, wide);
    let x = (zoomed_w - w - 16.0).max(0.0);

    let enter = Curve::tween(0.15, Easing::cubic(0.2, 0.8, 0.2, 1.0)).delay(0.2);
    let exit = Curve::tween(0.11, Easing::cubic(0.4, 0.0, 1.0, 1.0));
    let dragging = s.side.drag_origin.is_some();

    Some(
        el().key("side-resize")
            .absolute(x, 0.0)
            .w(8.0)
            .h_full()
            .center()
            .cursor(Cursor::ResizeColumn)
            .hover_group()
            .hover_motion(enter, exit)
            .on_drag(|s: &mut App, d: &mut Drag, tx: &mut Tx| {
                tx.instant();
                match d.phase {
                    Phase::Down => s.side.drag_origin = Some(s.side.width),
                    Phase::Move => {
                        if let Some(orig) = s.side.drag_origin {
                            s.side.resized = true;
                            s.side.width = (orig - d.dx / s.zoom).clamp(SIDE_MIN_W, SIDE_MAX_W);
                        }
                    }
                    Phase::Up => {
                        if d.cancelled()
                            && let Some(orig) = s.side.drag_origin
                        {
                            s.side.width = orig;
                        }
                        s.side.drag_origin = None;
                    }
                }
            })
            .on_double_press(|s: &mut App| {
                // Smooth spring return to default width on double-click (`PaneState::reset_width`).
                s.side.resized = false;
                s.side.width = SIDE_DEFAULT_W;
            })
            .child(
                el().key("side-resize-pill")
                    .size(4.0, 40.0)
                    .radius(2.0)
                    .fill(if dragging { p.icon } else { p.icon_muted })
                    .group_opacity(if dragging { 1.0 } else { 0.0 }, 1.0),
            ),
    )
}

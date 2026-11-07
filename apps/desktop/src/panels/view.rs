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
use super::state::{DOCK_MAX_H, DOCK_MIN_H, ResolvedEntry, ResolvedRegion, SPLIT_MIN_H};
use super::width::SESSION_MIN_W;
use super::tabs::SessionTabs;
use super::{CloseButtonMode, PanelRegion, PanelViewCtx, SessionPane};
use crate::palette::{Elevate as _, Pal};
use crate::view::{UI, UI_SMALL};
use crate::{App, Route};

/// Shared panel spring matching `view/sidebar.rs::SIDEBAR_SPRING` and `panel-motion.ts`.
pub const SIDE_SPRING: Spring = Spring::new(0.3, 0.0);
const INSTANT: Anim = Anim::INSTANT;
const FADE_FLOOR: f32 = 0.5;

/// The review toggle's own glyph (`session-header-actions.tsx`), mirrored so its pane sits on
/// the right, as Desktop draws it at 16 px.
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
    super::drag::note_zones(s);
    // Until the window is first measured, the last width it had.
    if zoomed_w > 0.0 {
        s.side.laid_w.set(zoomed_w);
    }
    let w = s.side_width(s.side.laid_w.get());

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
                .map_or(720.0 / s.zoom, |[_, h]| h)
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
            .pin(Some(0.0), Some(crate::view::chrome().top()), None, Some(col_bottom))
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

    let split_h = (col_h * (1.0 - s.split_ratio())).round();

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
        let ph = if panes.len() == 1 {
            col_h
        } else if idx == 0 {
            (col_h - split_h).max(0.0)
        } else {
            split_h
        };
        let pane_el = pane_box(s, p, reg, pane, (width, ph), env_scale, idx == 0);
        // The lower pane takes its height from the split, under a 1 px `border-base` rule.
        let sized = if idx == 0 {
            pane_el.grow(1.0).min_h(0.0)
        } else {
            col().w_full().h(split_h).child(el().w_full().h(1.0).fill(p.border)).child(pane_el.grow(1.0).min_h(0.0))
        };
        body = body.child(sized);
    }
    if panes.len() > 1 {
        body = body.child(split_handle(s, p, col_h, split_h));
    }

    // `side-region.tsx`: bg-base, 10 pt corners, `--v2-elevation-raised`.
    col().w(width).radius(10.0).fill(p.bg_base).elevate(p.raised).child(body)
}

/// The split's `ResizeHandle` (`direction="vertical" edge="start"`): 8 tall across the lower
/// pane's top edge, sizing it between `SPLIT_MIN_H` and the column less `SPLIT_MIN_H`.
fn split_handle(s: &App, p: &Pal, col_h: f32, split_h: f32) -> El {
    let dragging = s.side.split_drag_origin.is_some();
    resize_pill(p, dragging, false)
        .key("side-split-resize")
        .absolute(0.0, col_h - split_h - 4.0)
        .w_full()
        .h(8.0)
        .cursor(Cursor::ResizeRow)
        .on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| {
            tx.instant();
            let Some(sid) = s.active_session_id().map(str::to_owned) else { return };
            match d.phase {
                Phase::Down => s.side.split_drag_origin = Some((split_h, s.session_side_mut(&sid).split_ratio)),
                Phase::Move => {
                    if let Some((start, _)) = s.side.split_drag_origin
                        && col_h > 1.0
                    {
                        let h = (start - d.dy / s.zoom).clamp(SPLIT_MIN_H, SPLIT_MIN_H.max(col_h - SPLIT_MIN_H));
                        s.session_side_mut(&sid).split_ratio = Some(1.0 - h / col_h);
                    }
                }
                Phase::Up => {
                    if d.cancelled()
                        && let Some((_, stored)) = s.side.split_drag_origin
                    {
                        s.session_side_mut(&sid).split_ratio = stored;
                    }
                    s.side.split_drag_origin = None;
                }
            }
        })
}

/// A `ResizeHandle`'s hit area with its pill centred (4 by 40 across its length): `icon-muted`
/// after a 200 ms hover, `icon-base` while it drags.
pub(super) fn resize_pill(p: &Pal, dragging: bool, horizontal: bool) -> El {
    let enter = Curve::tween(0.15, Easing::cubic(0.2, 0.8, 0.2, 1.0)).delay(0.2);
    let exit = Curve::tween(0.11, Easing::cubic(0.4, 0.0, 1.0, 1.0));
    let (w, h) = if horizontal { (4.0, 40.0) } else { (40.0, 4.0) };
    el().center()
        .hover_group()
        .hover_motion(enter, exit)
        .child(
            el().size(w, h)
                .radius(2.0)
                .fill(if dragging { p.icon } else { p.icon_muted })
                .group_opacity(if dragging { 1.0 } else { 0.0 }, 1.0),
        )
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
        s,
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
        let bar_h = 48.5;
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
    s: &App,
    p: &Pal,
    pane: SessionPane,
    entries: &[&ResolvedEntry],
    pane_active: Option<&str>,
    _preview_key: Option<&str>,
    shown_new_id: Option<&str>,
    live_new: &[&super::state::NewTabEntry],
    reserve_toggle: bool,
) -> El {
    // Desktop's panel strip (`tabs.css`, `#review-panel` / `#bottom-panel`): 28 tall tabs 8 apart
    // from a 12 inset, the "+" flush after the last, fading at the strip's end.
    let mut strip = row()
        .key(format!("side-tabs-{}", pane_slug(pane)))
        .gap(8.0)
        .center_y()
        .grow(1.0)
        .min_w(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .clip_fade(0.0, 0.0, 8.0, 0.0);

    // While the keyboard is the chat's, the selected tab rests at the strip's muted ink.
    let focused = s.side.focused && s.side.active_pane_focus == pane;
    for entry in entries {
        let key = entry.key.clone();
        let active = shown_new_id.is_none() && pane_active == Some(key.as_str());
        let dragging = s.side.pane_drag.dragging() == Some(key.as_str());
        strip = strip.child(tab_trigger(s, p, entry, active, focused, dragging));
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
            .min_w(72.0)
            .pl(10.0)
            .pr(4.0)
            .gap(6.0)
            .center_y()
            .radius(6.0)
            .hover_group()
            .fill(if active { p.layer2 } else { Color::TRANSPARENT })
            .hover(if active { p.layer2 } else { p.hover })
            .hover_motion(INSTANT, INSTANT)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.side.chosen_new_tab.insert(pane, nid.clone());
                s.side.focused = true;
                s.side.active_pane_focus = pane;
            })
            .child(
                text("New tab")
                    .font(UI)
                    .color(if active && focused { p.text } else { p.muted })
                    .truncate_fade(),
            )
            .child(close_button(p, format!("side-new-close-{close_nid}"), active, move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.dismiss_side_new_tab(&close_nid);
            }));
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
            .child(icon(&ic::PLUS, 16.0).color(p.icon_muted)),
    );

    // Every pane's strip is `--tabs-bar-height` tall; its controls end at the header's inset.
    let mut bar = row()
        .key(format!("side-tabbar-{}", pane_slug(pane)))
        .w_full()
        .h(48.0)
        .pl(12.0)
        .pr(12.0)
        .gap(8.0)
        .center_y()
        .window_drag()
        .child(strip);

    if pane == SessionPane::Bottom {
        bar = bar.child(
            el().key("bottom-hide")
                .role("button")
                .label(crate::keymap::Bind::BottomToggle.tip("Hide bottom panel"))
                .size(28.0, 28.0)
                .radius(6.0)
                .center()
                .hover(p.hover)
                .press(p.pressed)
                .hover_motion(INSTANT, INSTANT)
                .on_press_tx(|s: &mut App, tx: &mut Tx| s.toggle_bottom_region(tx))
                .child(icon(&ic::CHEVRON_DOWN, 16.0).color(p.icon_muted)),
        );
    } else if reserve_toggle {
        // Reserves space at the end of the top tab bar for the floating `side-toggle` button (`size-7`).
        bar = bar.child(el().key("side-toggle-slot").size(28.0, 28.0));
    }

    bar
}

/// A tab key as an element key.
pub(super) fn safe_id(key: &str) -> String {
    key.replace(':', "-").replace('/', "_")
}

/// One tab of a pane's strip (`PanelTrigger`): 28 tall, 10 in, 72 at the narrowest, the
/// selected one on `layer-02`; 13/440 labels in the muted ink, the selected one's in the base
/// ink while its pane has the keyboard; the 20 pt close button 4 from its end. A press selects
/// it, a drag moves it between panes, a right click opens its menu.
fn tab_trigger(s: &App, p: &Pal, entry: &ResolvedEntry, active: bool, focused: bool, dragging: bool) -> El {
    let select_key = entry.key.clone();
    let promote_key = entry.key.clone();
    let menu_key = entry.key.clone();
    let close_key = entry.key.clone();
    let drag_key = entry.key.clone();
    let drag_title = entry.tab.title.clone();
    let safe_id = safe_id(&entry.key);

    let label_color = if entry.tab.missing {
        p.faint
    } else if active && focused {
        p.text
    } else {
        p.muted
    };

    let mut pill = row()
        .key(format!("side-tab-{safe_id}"))
        .role("tab")
        .label(entry.tab.title.clone())
        .h(28.0)
        .min_w(72.0)
        .max_w(320.0)
        .shrink()
        .pl(10.0)
        .pr(if entry.tab.can_close() { 4.0 } else { 10.0 })
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover_group()
        .fill(if active { p.layer2 } else { Color::TRANSPARENT })
        .hover(if active { p.layer2 } else { p.hover })
        .hover_motion(INSTANT, INSTANT)
        .opacity(if dragging { 0.4 } else { 1.0 })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            // The release that ends a drag isn't a click on the tab under it.
            if !s.side.pane_drag.take_suppressed() {
                s.select_side_tab(&select_key, tx);
            }
        })
        .on_double_press(move |s: &mut App| {
            s.promote_preview_tab(&promote_key);
        })
        .on_secondary_press(move |s: &mut App, ev: PointerPress, tx: &mut Tx| {
            tx.instant();
            s.side.tab_menu = Some((menu_key.clone(), ev.x / s.zoom, ev.y / s.zoom));
        });
    if !entry.tab.pinned && entry.tab.draggable {
        pill = pill.on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| s.pane_drag_gesture(&drag_key, &drag_title, d, tx));
    }

    // A panel's own label (`PanelTab.label`) takes the icon's and the title's place.
    if let Some(label) = entry.provider.tab_lead.and_then(|lead| lead(s, p, &entry.tab, active)) {
        pill = pill.child(label.shrink().min_w(0.0));
    } else {
        match entry.tab.icon {
            // A file's badge in its own colours while selected or under the pointer, else in
            // the label's one colour (`FileVisual`'s `tab-fileicon-color` and `-mono`, `tabs.css`).
            Some(glyph) if entry.tab.file.is_some() => {
                let badge = icon(glyph, 16.0).key("icon").color(label_color);
                pill = pill.child(if active {
                    badge
                } else {
                    el().key("icon").size(16.0, 16.0)
                        .child(badge.key("icon-color").absolute(0.0, 0.0).group_opacity(0.0, 1.0))
                        .child(icon(glyph, 16.0).key("icon-mono").absolute(0.0, 0.0).icon_one_color().color(label_color).group_opacity(1.0, 0.0))
                });
            }
            Some(glyph) => pill = pill.child(icon(glyph, 16.0).color(if active { p.icon } else { p.icon_muted })),
            None => {}
        }
        pill = pill.child(
            text(entry.tab.title.clone())
                .font(UI)
                .color(label_color)
                .truncate_fade()
                .shrink(),
        );
    }

    if let Some(badge) = &entry.tab.badge {
        pill = pill.child(text(badge.clone()).font(UI_SMALL).color(p.faint));
    }

    if entry.tab.can_close() {
        let always_visible = entry.tab.closable == CloseButtonMode::Standard || active;
        pill = pill.child(close_button(p, format!("side-tab-close-{safe_id}"), always_visible, move |s: &mut App, tx: &mut Tx| {
            s.close_side_tab(&close_key, tx);
        }));
    }

    pill
}

/// A tab's close button (`tabs-trigger-close-button`): 20 pt, the 16 pt small cross, shown on
/// the selected tab and on hover.
fn close_button(p: &Pal, key: String, shown: bool, close: impl Fn(&mut App, &mut Tx) + 'static) -> El {
    el().key(key)
        .role("button")
        .label("Close tab")
        .size(20.0, 20.0)
        .radius(4.0)
        .center()
        .hover(p.hover)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(close)
        .child(icon(&ic::XMARK_SMALL, 16.0).color(if shown { p.icon_muted } else { Color::TRANSPARENT }).group_color(p.icon_muted))
}

/// A pane that shows nothing (`side-region.tsx`'s empty state): OpenCode's mark, 56 wide at 10%,
/// over "Select a file to open", raised a little off centre.
fn empty_pane_view(p: &Pal) -> El {
    // `Mark` on its 16 by 20 grid at 3.5×: the O in `icon-strong`, its shadow in `icon-weak`.
    let u = 3.5;
    let bar = |x: f32, y: f32, w: f32, h: f32, c: Color| el().absolute(x * u, y * u).size(w * u, h * u).fill(c);
    let mark = el()
        .key("side-empty-mark")
        .size(16.0 * u, 20.0 * u)
        .opacity(0.1)
        .child(bar(4.0, 8.0, 8.0, 8.0, p.icon_faint))
        .child(bar(0.0, 0.0, 16.0, 4.0, p.icon))
        .child(bar(0.0, 16.0, 16.0, 4.0, p.icon))
        .child(bar(0.0, 4.0, 4.0, 12.0, p.icon))
        .child(bar(12.0, 4.0, 4.0, 12.0, p.icon));
    col()
        .key("side-empty")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .center()
        .px(24.0)
        // `pt-2`, `-mt-4` and `pb-42`: the centre sits 8 above where `pb-42` alone puts it.
        .pb(168.0 + 16.0)
        .gap(24.0)
        .child(mark)
        .child(text("Select a file to open").font(rune::TextStyle::new(14.0, 400.0).lh(20.0)).color(p.muted).max_w(224.0))
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
            // `SessionHeaderActions`: "Toggle review" and its keybind in the tooltip under it.
            .label("Toggle review")
            .expanded(open)
            .help("Toggle review")
            .help_keys(crate::keymap::Bind::SideToggle.keycaps())
            // On the session header's centre line, 12pt in from the panel's edge.
            .pin(None, Some(crate::view::chrome().top() + 10.0), Some(20.0), None)
            .size(28.0, 28.0)
            .radius(6.0)
            .center()
            .hover(p.hover)
            .press(p.pressed)
            .hover_motion(INSTANT, INSTANT)
            .on_press_tx(|s: &mut App, tx: &mut Tx| s.toggle_side_region(tx))
            .child(
                icon(&LAYOUT_RIGHT_FULL, 16.0)
                    .color(p.icon_muted)
                    .absolute(6.0, 6.0)
                    .opacity(if open { 1.0 } else { 0.0 })
                    .opacity_motion(SIDE_SPRING),
            )
            .child(
                icon(&LAYOUT_RIGHT, 16.0)
                    .color(p.icon_muted)
                    .absolute(6.0, 6.0)
                    .opacity(if open { 0.0 } else { 1.0 })
                    .opacity_motion(SIDE_SPRING),
            ),
    )
}

/// The chat panel's `ResizeHandle` (`screen.tsx`): 8 wide across the gap after the chat panel.
/// A drag sets the session's chat width, 1:1 with the pointer, between `SESSION_MIN_W` and what
/// leaves the side region `SIDE_PANE_MIN_W`; the side region takes the rest. A double press
/// does nothing, as Desktop's handle ignores one.
pub fn resize_handle(s: &App, p: &Pal, zoomed_w: f32) -> Option<El> {
    if !matches!(s.route, Route::Session(_)) || s.settings.open || !s.right_column_open() || zoomed_w <= 0.0 {
        return None;
    }
    let x = (zoomed_w - s.side_width(zoomed_w) - 16.0).max(0.0);
    let chat = s.chat_width(zoomed_w);
    let max = s.chat_width_max(zoomed_w);
    let dragging = s.side.drag_origin.is_some();

    Some(
        resize_pill(p, dragging, true)
            .key("side-resize")
            .pin(Some(x), Some(crate::view::chrome().top()), None, Some(8.0))
            .w(8.0)
            .cursor(Cursor::ResizeColumn)
            .on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| {
                tx.instant();
                let Some(sid) = s.active_session_id().map(str::to_owned) else { return };
                match d.phase {
                    Phase::Down => s.side.drag_origin = Some((chat, s.session_side_mut(&sid).session_width)),
                    Phase::Move => {
                        if let Some((start, _)) = s.side.drag_origin {
                            s.session_side_mut(&sid).session_width = Some((start + d.dx / s.zoom).clamp(SESSION_MIN_W, max.max(SESSION_MIN_W)));
                        }
                    }
                    Phase::Up => {
                        if d.cancelled()
                            && let Some((_, stored)) = s.side.drag_origin
                        {
                            s.session_side_mut(&sid).session_width = stored;
                        }
                        s.side.drag_origin = None;
                    }
                }
            }),
    )
}

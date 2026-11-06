use super::*;
use crate::session_menu::{Spot, Target};

// ---------------------------------------------------------------------------------------------
// Sidebar: Desktop's vertical tabs (`shell/titlebar/titlebar.tsx`, `tab-nav.tsx`).

/// Default, minimum and maximum widths (`shell.tsx`'s ResizeHandle).
pub const SIDEBAR_W: f32 = 260.0;
pub const SIDEBAR_MIN: f32 = 120.0;
pub const SIDEBAR_MAX: f32 = 640.0;
/// The sidebars' shared spring, "smooth" (`panel-motion.ts`).
pub(super) const SIDEBAR_SPRING: Spring = Spring::new(0.3, 0.0);
/// How far a closed panel's contents dim at the end of the flight.
pub(super) const FADE_FLOOR: f32 = 0.5;

/// Home, Search sessions and New session (`data-titlebar-tab-action`).
pub(super) fn action_row(p: &Pal, key: &str, glyph: &'static Icon, label: &str, hint: &str, pressed: bool) -> El {
    let ink = if pressed { p.text } else { p.faint };
    let mut r = row()
        .key(key)
        .role("button")
        .label(label)
        .h(28.0)
        .w_full()
        .pl(6.0)
        .pr(8.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .child(icon(glyph, 16.0).color(ink).group_color(p.text))
        .child(text(label).font(UI).color(ink).group_color(p.text).truncate_fade());
    if pressed {
        r = r.fill(p.pressed);
    }
    if !hint.is_empty() {
        r = r.child(spacer()).child(text(hint).font(UI).color(p.faint).group_opacity(0.0, 1.0));
    }
    r
}

pub(crate) fn sidebar(s: &App, p: &Pal) -> El {
    let current = match &s.route {
        Route::Session(id) if !s.settings.open => Some(s.root_of(id)),
        _ => None,
    };
    // No gap: each tab's hit area takes half the space to its neighbours (see `tab_row`).
    let mut list = col().key("tabs").w_full().pb(16.0);
    for (i, t) in s.tabs.iter().enumerate() {
        list = list.child(tab_row(s, p, t, i, current == Some(t.id.as_str()), s.tabs.len()));
    }

    let w = s.sidebar_w;
    let chrome = chrome();
    let inner = col()
        .key("sidebar-inner")
        // Pinned to the aside's end edge: it slides out as the aside narrows.
        .absolute(if s.sidebar_open { 0.0 } else { -w }, 0.0)
        .w(w)
        .h_full()
        .pt(chrome.top())
        .pl(10.0)
        .pr(2.0)
        .pb(10.0)
        .opacity(if s.sidebar_open { 1.0 } else { FADE_FLOOR })
        .opacity_motion(SIDEBAR_SPRING)
        // Shares the session header's 48px centre line with the traffic lights; drags the window.
        // A titlebar row above holds the toggle instead (`titlebar.tsx` on Windows).
        .children(chrome.sidebar_header().then(|| {
            row()
                .key("sidebar-header")
                .w_full()
                .h(48.0)
                .mb(8.0)
                .gap(8.0)
                .center_y()
                .window_drag()
                // The lights' end, less the sidebar's 10pt padding.
                .child(el().key("lights").w((chrome.lights_end - 10.0).max(0.0)).h(28.0))
                .child(spacer())
                // The toggle floats above (see `sidebar_toggle`); its slot keeps the row's shape.
                .child(el().key("toggle-slot").size(28.0, 28.0))
        }));
    let inner = inner.child(
        col()
            .key("actions")
            .w_full()
            .gap(4.0)
            .child(action_row(p, "home", &ic::GRID_PLUS, "Home", "⌘B", s.route == Route::Home && !s.settings.open).on_press_tx(
                |s: &mut App, tx: &mut Tx| s.go_home(tx),
            ))
            .child(action_row(p, "search", &ic::MAGNIFYING_GLASS, "Search sessions", "⇧⌘F", false).on_press_tx(
                |s: &mut App, tx: &mut Tx| {
                    s.close_settings();
                    s.open_session_search(tx)
                },
            ))
            .child(action_row(p, "new", &ic::EDIT, "New session", "⌘T", false).on_press_tx(
                |s: &mut App, tx: &mut Tx| s.new_session(tx),
            )),
    )
    .child(el().key("tabs-gap").w_full().h(16.0))
    .child(
        col()
            .key("tabs-scroll")
            .role("list")
            .focus_group()
            .w_full()
            .grow(1.0)
            .shrink()
            .scroll(ScrollSpec { stick_bottom: false })
            .clip_fade(0.0, 0.0, 0.0, 16.0)
            .child(list),
    )
    .children(crate::updater::sidebar_pill(s, p));

    // The aside's width moves the panel's edge; its contents keep their width, pinned to that edge.
    el().key("sidebar")
        .role("navigation")
        .w(if s.sidebar_open { w } else { 0.0 })
        .h_full()
        
        .clip_soft(0.0)
        .motion(SIDEBAR_SPRING)
        .child(inner)
}

pub(super) const TAB_H: f32 = 28.0;
pub(super) const TAB_GAP: f32 = 4.0;
/// How far a tab steps aside for a dragged one.
pub(super) const TAB_PITCH: f32 = TAB_H + TAB_GAP;
/// The tabs a dragged one passes step aside, and a dropped one settles, on this: Beta's
/// sortable transition (250 ms, `cubic-bezier(0.25, 1, 0.5, 1)`) as a spring with no bounce.
pub(super) const REORDER_SPRING: Spring = Spring::new(0.25, 0.0);

/// Source-over of straight-alpha colours, for a lifted tab that hides the rows it passes.
pub(super) fn over(top: Color, bottom: Color) -> Color {
    let a = top.a + bottom.a * (1.0 - top.a);
    if a <= 0.0 {
        return Color::TRANSPARENT;
    }
    let mix = |t: f32, b: f32| (t * top.a + b * bottom.a * (1.0 - top.a)) / a;
    Color { r: mix(top.r, bottom.r), g: mix(top.g, bottom.g), b: mix(top.b, bottom.b), a }
}

/// A tab's hit area spans half the gap to each neighbour, so the pointer is always over one tab
/// (Beta's "fill vertical tab hover gaps"). Only the 28pt pill inside it paints.
pub(crate) fn tab_row(s: &App, p: &Pal, t: &crate::tabs::Tab, index: usize, active: bool, count: usize) -> El {
    let info = s.session(&t.id);
    let title = info.map(title_of).unwrap_or_else(|| if t.title.is_empty() { "Untitled session".into() } else { t.title.clone() });
    let dir = info.map_or(t.directory.as_str(), |i| i.directory.as_str());
    let project = info.and_then(|i| s.project_for_session(i)).or_else(|| s.project_at(dir));
    let proj = project_or_dir(project, dir);
    let status = s.tab_status(&t.id);
    // The title fades out before the status/close slot (`--tab-title-fade-offset`).
    let fade_offset = if active || status.is_some() { 24.0 } else { 4.0 };
    // 4px from the row's end; the row spans the sidebar less its 10 + 2 padding.
    let slot_x = s.sidebar_w - 12.0 - 24.0;
    let id = t.id.clone();
    let hover_id = t.id.clone();
    let nav_ids: Vec<_> = s.tabs.iter().map(|t| t.id.clone()).collect();
    let close_id = t.id.clone();
    let before = if index > 0 { TAB_GAP / 2.0 } else { 0.0 };
    let after = if index + 1 < count { TAB_GAP / 2.0 } else { 0.0 };
    let dragging = s.tab_drag.dragging() == Some(index);
    let target = Target::new(&t.id, Spot::Tab);
    let marked = s.menus.on.as_ref() == Some(&target);
    let rename_id = target.clone();
    let mut hit = col()
        .key(format!("t-{}", t.id))
        .role("button")
        .label(title.clone())
        .w_full()
        .h(before + TAB_H + after)
        .pt(before)
        .pb(after)
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
            tx.live_only();
            if inside { s.prefetch_session(&hover_id); }
        })
        .on_key(move |s: &mut App, ev, _: &mut Tx| s.prefetch_navigation(&nav_ids, index, ev))
        // Desktop's tab title: a double click renames it in place.
        .on_double_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.begin_rename(&rename_id);
        })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            // A drag's release lands on the lifted tab, which isn't a click (Beta's vertical
            // tabs don't open on a drag either).
            if !s.tab_drag.was_drag() {
                s.open(&id, tx);
            }
        })
        .on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| match d.phase {
            Phase::Down => s.tab_drag.press(),
            Phase::Move => {
                tx.animated();
                let count = s.tabs.len();
                s.tab_drag.follow(index, count, d.dy / s.zoom, TAB_PITCH);
            }
            Phase::Up => {
                let on_tab = (0.0..=d.w).contains(&d.x) && (0.0..=d.h).contains(&d.y);
                if let Some((from, to)) = s.tab_drag.end(d.cancelled(), on_tab) {
                    s.move_tab(from, to);
                }
            }
        });
    let mut r = row()
        .key("pill")
        .w_full()
        .h(TAB_H)
        .radius(6.0)
        .child(
            el().key("hover")
                .absolute(0.0, 0.0)
                .w_full()
                .h(TAB_H)
                .radius(6.0)
                .fill(p.hover)
                .group_opacity(0.0, if dragging { 0.0 } else { 1.0 }),
        )
        .child(
            row()
                .key("link")
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .h_full()
                .pl(6.0)
                .mr(fade_offset + 6.0)
                .gap(6.0)
                .center_y()
                .clip_fade(0.0, 0.0, 16.0, 0.0)
                .child(crate::icons::badge(s, p, project, &proj))
                .child(match crate::session_menu_view::field(s, p, &target, UI_MED) {
                    Some(field) => field,
                    None => text(title).font(UI_MED).color(if active { p.text } else { p.faint }),
                }),
        )
        .child(
            // Status and close share the slot; hovering the row swaps one for the other.
            el().key("status")
                .absolute(slot_x, 4.0)
                .size(20.0, 20.0)
                .center()
                .group_opacity(1.0, 0.0)
                .child(match status {
                    Some(TabStatus::Busy) => activity_ring(p),
                    Some(TabStatus::Attention) => el().size(7.0, 7.0).radius(3.5).fill(p.warning),
                    Some(TabStatus::Unread) => el().size(7.0, 7.0).radius(3.5).fill(p.bg_accent),
                    None => el(),
                }),
        )
        .child(
            el().key("close")
                .role("button")
                .label("Close tab")
                .absolute(slot_x, 4.0)
                .size(20.0, 20.0)
                .radius(4.0)
                .center()
                .fill(p.bg_deep)
                .hover(p.hover)
                .hover_motion(INSTANT, INSTANT)
                .group_opacity(0.0, 1.0)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.close_tab(&close_id, tx))
                .child(
                    el().key("close-ink")
                        .size(20.0, 20.0)
                        .radius(4.0)
                        .center()
                        .fill(if active { p.pressed } else { p.hover })
                        .child(icon(&ic::XMARK_SMALL, 16.0).color(p.icon_muted)),
                ),
        );
    if active {
        r = r.fill(p.pressed);
    } else if marked {
        r = r.fill(p.hover);
    }
    if dragging {
        // Beta lifts it with the pressed overlay alone; here it's opaque on the sidebar's deep
        // background so the rows it passes don't show through.
        r = r.fill(over(p.pressed, p.bg_deep));
    }
    // Its layout and its step aside share one spring, so a drop that swaps one for the other
    // leaves a passed tab still and slides the dropped one from where it was drawn.
    // Tab visits the open tab; the arrows rove from there, and the menu key opens its menu.
    hit = crate::session_menu::on(hit.focusable(true).focus_selected(active), Target::new(&t.id, Spot::Tab)).child(r).motion(REORDER_SPRING).offset(0.0, s.tab_drag.shift(index, TAB_PITCH));
    if dragging {
        // It follows the pointer exactly.
        hit = hit.raise().offset_motion(INSTANT);
    }
    hit
}

/// The sidebar edge's resize handle: an 8px hit area across the gap, with a pill that waits a
/// moment before showing on hover (`resize-handle.css`).
pub(super) fn resize_handle(s: &App, p: &Pal, w: f32) -> El {
    let enter = Curve::tween(0.15, Easing::cubic(0.2, 0.8, 0.2, 1.0)).delay(0.2);
    let exit = Curve::tween(0.11, Easing::cubic(0.4, 0.0, 1.0, 1.0));
    let dragging = s.sidebar_drag.is_some();
    el().key("sidebar-resize")
        // Below a titlebar row, which moves the window instead.
        .absolute(w, chrome().row_h)
        .w(8.0)
        .h_full()
        .center()
        .cursor(Cursor::ResizeColumn)
        .hover_group()
        .hover_motion(enter, exit)
        .on_drag(|s: &mut App, d: &mut Drag, tx: &mut Tx| {
            tx.instant();
            match d.phase {
                Phase::Down => s.sidebar_drag = Some(s.sidebar_w),
                Phase::Move => {
                    let from = s.sidebar_drag.unwrap_or(s.sidebar_w);
                    s.sidebar_w = (from + d.dx / s.zoom).clamp(SIDEBAR_MIN, SIDEBAR_MAX);
                }
                Phase::Up => {
                    if d.cancelled() && let Some(w) = s.sidebar_drag { s.sidebar_w = w; }
                    s.sidebar_drag = None;
                }
            }
        })
        .child(
            el().key("pill")
                .size(4.0, 40.0)
                .radius(2.0)
                .fill(if dragging { p.icon } else { p.icon_muted })
                .group_opacity(if dragging { 1.0 } else { 0.0 }, 1.0),
        )
}

/// One toggle that reads as the header's and the collapsed one: it rides the sidebar's edge
/// to the collapsed spot past the traffic lights while its glyph turns (`sidebar-toggle.tsx`).
pub(super) fn sidebar_toggle(p: &Pal, open: bool, w: f32) -> El {
    let (x, y) = chrome().toggle_at(open, w);
    el().key("sidebar-toggle")
        .role("button")
        .label("Toggle sidebar")
        .absolute(x, y)
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .motion(SIDEBAR_SPRING)
        .on_press(|s: &mut App| s.sidebar_open = !s.sidebar_open)
        .child(icon(&ic::LAYOUT_LEFT_FULL, 20.0).color(p.icon_muted).absolute(4.0, 4.0).opacity(if open { 1.0 } else { 0.0 }).opacity_motion(SIDEBAR_SPRING))
        .child(icon(&ic::LAYOUT_LEFT, 20.0).color(p.icon_muted).absolute(4.0, 4.0).opacity(if open { 0.0 } else { 1.0 }).opacity_motion(SIDEBAR_SPRING))
}

/// Room the session header keeps for the collapsed toggle.
pub(crate) fn collapsed_slot(s: &App) -> El {
    // Beta measures from the header's 10px padding; this header pads 14.
    el().key("toggle-space").w(chrome().toggle_space(s.sidebar_open)).h(28.0).motion(SIDEBAR_SPRING)
}

/// Desktop's activity spinner, "Soft Radar": a 5×5 dot matrix whose sweep head steps clockwise
/// around each square ring, leaving a fading trail (`spinner-radar.ts`), 14px, icon-muted.
pub(crate) use session_view::style::{activity_ring, activity_ring_sized};

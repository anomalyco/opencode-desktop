use rune_kit::shell::TabOrientation;

use super::*;
use crate::layout_state::{PeekEvent, SidebarPeekState};

// ---------------------------------------------------------------------------------------------
// Sidebar: Desktop's vertical tabs (`shell.tsx`, `titlebar.tsx`, `tab-strip.tsx`). Top tabs have
// none: the aside narrows to nothing and its tabs fly to the titlebar.

/// Default, minimum and maximum widths (`shell.tsx`'s `tabsWidth` and its ResizeHandle).
pub const SIDEBAR_W: f32 = 240.0;
pub const SIDEBAR_MIN: f32 = 136.0;
pub const SIDEBAR_MAX: f32 = 640.0;
/// The sidebar's start and end padding (`--sidebar-inset-start`, `--sidebar-inset-end`).
pub(super) const INSET_START: f32 = 11.0;
pub(super) const INSET_END: f32 = 3.0;
/// The sidebars' shared spring, "smooth" (`panel-motion.ts`).
pub(super) const SIDEBAR_SPRING: Spring = Spring::new(0.3, 0.0);
/// A peek slides in and out on this (`sidebar-peek.ts`'s 0.18 s, no bounce).
const PEEK_SPRING: Spring = Spring::new(0.18, 0.0);
/// Under Reduce Motion a peek fades in place (`0.15s`).
const PEEK_FADE: Spring = Spring::new(0.15, 0.0);
/// How far a closed panel's contents dim at the end of the flight.
pub(super) const FADE_FLOOR: f32 = 0.5;
/// A side tab's height plus the strip's 3pt gap: one row of the vertical strip.
#[cfg(test)]
pub(super) const TAB_PITCH: f32 = 28.0 + 3.0;
/// The peek's edge zone (`sidebar-peek.ts`): 24pt in from the window's start, from 56pt below
/// the sidebar's top, and the corridor around the toggle and the peek that holds it open.
const EDGE_W: f32 = 24.0;
const EDGE_TOP: f32 = 56.0;
const CORRIDOR: f32 = 15.0;

/// Home, Search sessions and New session (`data-titlebar-tab-action`): 28pt rows, `px-2 gap-2`,
/// faint until hovered or pressed, their keys showing on hover.
pub(super) fn action_row(p: &Pal, key: &str, glyph: &'static Icon, label: &str, hint: &str, pressed: bool) -> El {
    let ink = if pressed { p.text } else { p.faint };
    let mut r = row()
        .key(key)
        .role("button")
        .label(label)
        .h(28.0)
        .w_full()
        .px(8.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .hover(if pressed { p.pressed } else { p.hover })
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .child(icon(glyph, 16.0).color(ink).group_color(p.text))
        .child(text(label).font(UI).color(ink).group_color(p.text).truncate_fade());
    if pressed {
        r = r.fill(p.pressed);
    }
    if !hint.is_empty() {
        // `hidden group-hover:block`: no room is kept for it at rest, so the label never fades early.
        r = r.child(row().key("hint").pin(None, Some(0.0), Some(8.0), Some(0.0)).center_y().child(text(hint).font(UI).color(p.faint)).group_opacity(0.0, 1.0));
    }
    r
}

/// The docked sidebar: its aside's width moves the panel's edge while its contents keep their
/// width, pinned to that edge. Top tabs keep the aside at no width, empty.
pub(crate) fn sidebar(s: &App, p: &Pal) -> El {
    let aside = el().key("sidebar").role("navigation").h_full().clip_soft(0.0).motion(SIDEBAR_SPRING);
    if !s.tab_layout().has_sidebar() {
        return aside.w(0.0);
    }
    let w = s.sidebar_w;
    let open = s.sidebar_open;
    // A peek shows the contents over the panel instead; the aside keeps none meanwhile.
    if !open && s.layout.peek.shown() {
        return aside.w(0.0);
    }
    let inner = contents(s, p, chrome().top())
        .key("sidebar-inner")
        .absolute(if open { 0.0 } else { -w }, 0.0)
        .opacity(if open { 1.0 } else { FADE_FLOOR })
        .opacity_motion(SIDEBAR_SPRING);
    aside.w(if open { w } else { 0.0 }).child(inner)
}

/// What the sidebar holds, `w` wide from `top` down: the header line where the traffic lights
/// share it, Home, Search sessions and New session, then the tabs (or the inbox).
fn contents(s: &App, p: &Pal, top: f32) -> El {
    let chrome = chrome();
    let home = s.route == Route::Home && !s.settings.open;
    col()
        .w(s.sidebar_w)
        .h_full()
        .pt(top)
        .pl(INSET_START)
        .pr(INSET_END)
        .pb(10.0)
        // Shares the session header's 48px centre line with the traffic lights; drags the window.
        // A titlebar row above holds the toggle instead (`titlebar.tsx` on Windows).
        .children(chrome.sidebar_header().then(|| {
            row()
                .key("sidebar-header")
                .w_full()
                .h(48.0)
                .mb(2.0)
                .gap(8.0)
                .center_y()
                .window_drag()
                // The lights' end, less the sidebar's start padding.
                .child(el().key("lights").w((chrome.lights_end - INSET_START).max(0.0)).h(28.0))
                .child(spacer())
                // The toggle floats above (see `sidebar_toggle`); its slot keeps the row's shape.
                .child(el().key("toggle-slot").size(28.0, 28.0))
        }))
        .child(
            col()
                .key("actions")
                .w_full()
                .gap(3.0)
                .child(
                    action_row(p, "home", &ic::GRID_PLUS, "Home", &crate::keymap::Bind::Home.label(), home)
                        .matched(super::titlebar::NAV_HOME).matched_crop().motion(GLIDE)
                        .on_press_tx(|s: &mut App, tx: &mut Tx| {
                            s.toggle_home(tx);
                        }),
                )
                .child(
                    action_row(p, "search", &ic::MAGNIFYING_GLASS, "Search sessions", &crate::keymap::Bind::SessionSearch.label(), false)
                        .matched(super::titlebar::NAV_SEARCH).matched_crop().motion(GLIDE)
                        .on_press_tx(|s: &mut App, tx: &mut Tx| {
                            s.close_settings();
                            s.open_session_search(tx)
                        }),
                )
                .child(
                    action_row(p, "new", &ic::EDIT, "New session", &crate::keymap::Bind::TabNew.label(), false)
                        .matched(super::titlebar::NAV_NEW).matched_crop().motion(GLIDE)
                        .on_press_tx(|s: &mut App, tx: &mut Tx| s.new_session(tx))
                        // Right-click: "Recently closed" (`RecentlyClosedTabsMenu`).
                        .on_secondary_press(|s: &mut App, press: rune::PointerPress, tx: &mut Tx| crate::recently_closed::open(s, press, tx)),
                ),
        )
        .child(el().key("tabs-gap").w_full().h(16.0))
        .child(list(s, p))
        .children(crate::updater::sidebar_pill(s, p))
}

/// The sidebar's list: the side tabs, or the inbox.
fn list(s: &App, p: &Pal) -> El {
    match s.tab_layout() {
        // A switch crossfades the whole list, and the active session's tab flies into its card
        // (`shell-active-{id}`), which landing the list's changes at once would keep from flying.
        crate::layout_state::TabLayout::Inbox => super::inbox::inbox(s, p, (INSET_START, INSET_END)),
        // Experimental › Organize vertical sidebar, Projects: side tabs alone (`shell.tsx`).
        crate::layout_state::TabLayout::Vertical if s.settings.sidebar_organize == crate::settings::Organize::Projects => super::sidebar_organized::organized(s, p),
        _ => tab_strip(s, p, "tabs", TabOrientation::Vertical, None, None),
    }
}

/// The collapsed sidebar's hover peek (`sidebar-peek.ts`): the sidebar slides over the panel
/// from the window's start, lifted on the deep background, without moving the panel. The
/// pointer resting at the start edge or on the toggle brings it; straying past the corridor
/// around it and the toggle sends it back. None while the sidebar is docked or has no peek.
pub(crate) fn peek(s: &App, p: &Pal) -> Vec<El> {
    if !s.tab_layout().has_sidebar() || s.sidebar_open && !s.layout.peek.handed_off() {
        return vec![];
    }
    let chrome = chrome();
    let below = chrome.row_h;
    let edge = el()
        .key("peek-edge")
        .pin(Some(0.0), Some(chrome.top() + EDGE_TOP), None, Some(0.0))
        .w(EDGE_W)
        .on_hover_change(|s: &mut App, inside, tx: &mut Tx| {
            tx.live_only();
            let keep_open = s.popup.is_open();
            let can_start_edge = inside && can_peek(s);
            s.queue_peek(PeekEvent::Pointer { can_start_edge, keep_open }, tx);
        });
    let state = s.layout.peek;
    if !state.shown() {
        return vec![edge];
    }
    let w = s.sidebar_w;
    let reduced = s.reduced_motion;
    let leaving = matches!(state, SidebarPeekState::Leaving { .. });
    let (x, opacity) = match (leaving, reduced) {
        (true, false) => (-w, 1.0),
        (true, true) => (0.0, 0.0),
        _ => (0.0, 1.0),
    };
    let mut surface = contents(s, p, chrome.top() - below)
        .key("peek-surface")
        .fill(p.bg_deep)
        .offset(x, 0.0)
        .offset_motion(PEEK_SPRING)
        .opacity(opacity)
        .opacity_motion(if reduced { PEEK_FADE } else { PEEK_SPRING })
        .transition(if reduced { rune::Transition::opacity() } else { rune::Transition::offset(-w, 0.0) });
    if !state.handed_off() {
        surface = surface.shadow(12.0, 16.0, p.shadow).shadow(1.0, 2.0, p.shadow);
    }
    // The corridor: the peek and the room around it hold it open.
    let corridor = el()
        .key("peek-corridor")
        .pin(Some(0.0), Some(below), None, Some(0.0))
        .w(w + CORRIDOR)
        .on_hover_change(|s: &mut App, inside, tx: &mut Tx| {
            tx.live_only();
            let keep_open = inside || s.popup.is_open() || s.menus.rename.is_some();
            s.queue_peek(PeekEvent::Pointer { can_start_edge: false, keep_open }, tx);
        })
        .child(surface);
    vec![corridor]
}

/// Nothing holds the pointer elsewhere: no drag, resize or menu.
fn can_peek(s: &App) -> bool {
    !s.popup.is_open() && s.tab_drag.dragging().is_none() && s.sidebar_drag.is_none() && s.menus.confirm.is_none()
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

/// The sidebar toggle (`SidebarToggle`, ghost-muted large): the open glyph or the closed one,
/// with the unread badge while the closed sidebar hides an unread tab. Resting on it while the
/// sidebar is closed peeks it; a press docks it.
pub(super) fn toggle_button(s: &App, p: &Pal, open: bool) -> El {
    let unread = !open && s.tabs.iter().any(|t| Some(&t.id) != s.current_tab().as_ref() && s.tab_unread(&t.id));
    let mut b = el()
        .key("sidebar-toggle")
        .role("button")
        .label(if unread { "Toggle sidebar, unread sessions" } else { "Toggle sidebar" })
        .help("Toggle sidebar").help_keys(crate::keymap::Bind::Sidebar.keycaps())
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            s.toggle_sidebar(tx);
        })
        .on_hover_change(|s: &mut App, inside, tx: &mut Tx| {
            tx.live_only();
            let event = if inside { PeekEvent::ToggleEnter { can_peek: !s.sidebar_open && can_peek(s) } } else { PeekEvent::ToggleLeave };
            s.queue_peek(event, tx);
        })
        .child(icon(&ic::LAYOUT_LEFT_FULL, 16.0).color(p.icon_muted).absolute(6.0, 6.0).opacity(if open { 1.0 } else { 0.0 }).opacity_motion(SIDEBAR_SPRING))
        .child(icon(&ic::LAYOUT_LEFT, 16.0).color(p.icon_muted).absolute(6.0, 6.0).opacity(if open { 0.0 } else { 1.0 }).opacity_motion(SIDEBAR_SPRING));
    if unread {
        // `UnreadBadge`: a 7pt accent dot on the glyph's corner in a 1.5pt ring of the ground.
        let ring = 7.0 + 3.0;
        b = b.child(el().key("unread").absolute(6.0 + 16.0 - ring / 2.0 - 1.0, 6.0 - ring / 2.0 + 1.0).size(ring, ring).radius(ring / 2.0).fill(p.bg_deep).center().child(el().size(7.0, 7.0).radius(3.5).fill(p.bg_accent)));
    }
    b
}

/// One toggle that reads as the header's and the collapsed one: it rides the sidebar's edge
/// to the collapsed spot past the traffic lights while its glyph turns (`sidebar-toggle.tsx`).
pub(super) fn sidebar_toggle(s: &App, p: &Pal, open: bool, w: f32) -> El {
    let (x, y) = chrome().toggle_at(open, w);
    toggle_button(s, p, open).absolute(x, y).motion(SIDEBAR_SPRING)
}

/// Room the session header keeps for the collapsed toggle.
pub(crate) fn collapsed_slot(s: &App) -> El {
    // Beta measures from the header's 10px padding; this header pads 14.
    let open = s.sidebar_open || !s.tab_layout().has_sidebar();
    el().key("toggle-space").w(chrome().toggle_space(open)).h(28.0).motion(SIDEBAR_SPRING)
}

/// Desktop's activity spinner, "Soft Radar": a 5×5 dot matrix whose sweep head steps clockwise
/// around each square ring, leaving a fading trail (`spinner-radar.ts`), 14px, icon-muted.
pub(crate) use session_view::style::{activity_ring, activity_ring_sized};

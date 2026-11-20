//! Side region, split pane, bottom pane, tab strip, New Tab launchers, and `Recently closed` UI
//! (`packages/app/src/runtime/plugin/side-region.tsx`, `closed-tabs-view.tsx`).

mod pane;
mod strip;

use opencode_ui::keymap::Bind;
use rune::{Anim, AnimExt, Curve, Cursor, Cx, Drag, Easing, El, Icon, Phase, Spring, Tx, col, el, icon};
use opencode_ui::palette::{Elevate as _, Pal};

use crate::SessionPane;
use crate::drag::note_zones;
use crate::host::PanelsHost;
use crate::keys::closing_side_entry;
use crate::resize::{ResizeEffect, ResizeEvent};
use crate::state::{
    CloseTabGuardEvent, ResolvedRegion, SPLIT_MIN_H, resolve_side_region, session_side_mut, side_mounted, side_open,
    toggle_side_region,
};
use crate::width::{SESSION_MIN_W, chat_width, chat_width_max, side_width, split_ratio};

pub use crate::dock::{bottom_dock, bottom_reserve_height};
pub use crate::menus::context_menus;
pub use strip::TAB_UI;

pub(crate) use pane::pane_box;
pub(crate) use strip::safe_id;

/// Shared panel spring matching `view/sidebar.rs::SIDEBAR_SPRING` and `panel-motion.ts`.
pub const SIDE_SPRING: Spring = Spring::new(0.3, 0.0);
/// How long the side region's contents glide with its edge while the sidebar flies: SIDE_SPRING
/// within half a point of where it lands.
pub const SIDE_FLIGHT: f64 = 0.6;
/// How long a SIDE_SPRING flight takes to come wholly to rest, its last stretch far under a
/// thousandth of a point, so what changes once it lands changes out of sight.
pub const SIDE_REST: f64 = 1.0;

const INSTANT: Anim = Anim::INSTANT;
/// Desktop's `panelOpacity` (`panel-motion.ts`): a side card's contents fade only across the
/// last 15% of its travel, `min(1, (1 - closed) / 0.15)`. Over SIDE_SPRING (critically damped,
/// ω = 2π / 0.3 s, travel `1 - (1 + ωt)e^-ωt`) that is opacity 1 for the first 161 ms of a
/// close, then down to 0 by about 400 ms; an open reaches 1 after its first 15%, 32.5 ms in.
/// Baked as the share of the change done at each share of the time.
const CLOSE_FADE: Anim = Anim::new(Curve::tween(
    0.4,
    Easing::Piecewise(&[
        [0.0, 0.0],
        [0.4035, 0.0],
        [0.45, 0.267],
        [0.5, 0.475],
        [0.55, 0.627],
        [0.6, 0.736],
        [0.65, 0.814],
        [0.7, 0.87],
        [0.75, 0.909],
        [0.8, 0.937],
        [0.9, 0.97],
        [1.0, 1.0],
    ]),
));
const OPEN_FADE: Anim = Anim::new(Curve::tween(
    0.0325,
    Easing::Piecewise(&[
        [0.0, 0.0],
        [0.23, 0.074],
        [0.46, 0.267],
        [0.69, 0.545],
        [0.92, 0.875],
        [1.0, 1.0],
    ]),
));
/// How far the open side card's elevation shows past the side region's start edge.
const SIDE_BLEED: f32 = 4.0;

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

/// Right-hand column of `Side` tabs, animated on `SIDE_SPRING`.
///
/// A region: a keystroke elsewhere builds none of it, nor resolves its tabs (each panel's `list`).
/// What isn't `Tracked` is its inputs: the palette, the chrome, the laid width, the reserve, the
/// plugins' versions and whether the composer has the keyboard.
/// `main` is the main area's origin, which the column paints contained against (inside the region,
/// so a reused region's root is the one it built).
pub fn side_region<H: PanelsHost + 'static>(
    host: &H,
    p: &Pal,
    zoomed_w: f32,
    bottom_reserve: f32,
    main: [f32; 2],
    cx: &mut Cx<H>,
) -> El {
    // A drag's drop zones are read from the last layout every build, region or not.
    note_zones(host);
    // Until the window is first measured, the last width it had.
    if zoomed_w > 0.0 {
        host.side().laid_w.set(zoomed_w);
    }
    let laid_w = host.side().laid_w.get();
    let scale = cx.env().scale;
    let chrome = host.panel_chrome();
    let inputs = (
        (p.theme, p.generation, p.dark),
        opencode_i18n::locale().id(),
        (chrome.top.to_bits(), chrome.foot.to_bits(), chrome.stamp),
        laid_w.to_bits(),
        bottom_reserve.to_bits(),
        host.plugin_surfaces_version(),
        host.composer_focused(),
        main.map(f32::to_bits),
        scale.to_bits(),
    );
    rune::region_with("side-region", inputs, || {
        side_region_in(host, p, laid_w, bottom_reserve, main, scale, cx).paint_contained(main)
    })
}

fn side_region_in<H: PanelsHost + 'static>(
    host: &H,
    p: &Pal,
    laid_w: f32,
    bottom_reserve: f32,
    main: [f32; 2],
    scale: f32,
    cx: &mut Cx<H>,
) -> El {
    let on_session = host.active_session_id().is_some() && !host.settings_open();
    let w = side_width(host, laid_w);

    let col_open = on_session && side_open(host);
    let slot_w = if col_open { w + 8.0 } else { 0.0 };

    let outer = el()
        .key("side-region")
        .role("complementary")
        .w(slot_w)
        .h_full()
        .motion(SIDE_SPRING)
        // Desktop moves the side panel by layout (the chat's width springs, `session/screen.tsx`),
        // and Chromium paints a box layout moves on whole device pixels each frame, so the card's
        // ring and shadow keep their resting profile mid-flight instead of blending.
        .motion_snaps();
    // The slot clips what slides out of it, but lets the card's raised ring and shadow
    // ("--v2-elevation-raised", outside its box) show past its start edge while it is open or
    // still sliding shut, as Desktop's aside does; a slot at rest shut shows none.
    let shown = col_open || cx.layout_size("side-region").is_some_and(|[w, _]| w > 0.5);
    let bleed = if shown { SIDE_BLEED } else { 0.0 };
    let mut clip = el().key("side-region-clip").pin(Some(-bleed), Some(0.0), Some(0.0), Some(0.0)).clip_soft(0.0);

    // The column's opacity track exists before the first open. Its heavy body mounts only if
    // this session has shown it; another session's pane never pays for it here.
    // `session-side-panel-content` (`screen.tsx:521`, `will-change-[opacity]`) opens its own
    // composited layer inside `<main>`, so its origin snaps inside `<main>`'s snapped origin.
    let chrome = host.panel_chrome();
    let col_bottom = (if col_open { 8.0 + bottom_reserve } else { 8.0 }) + chrome.foot;
    let top = chrome.top;
    let layer_top = if scale > 0.0 {
        ((main[1] * scale + 0.5).floor() + ((top - main[1]) * scale + 0.5).floor()) / scale
    } else {
        top
    };
    let mut column = col()
        .key("side-column")
        .pin(Some(bleed), Some(layer_top), None, Some(col_bottom - (layer_top - top)))
        .w(w)
        .paint_contained([main[0], layer_top])
        .opacity(if col_open { 1.0 } else { 0.0 })
        .opacity_motion(if col_open { OPEN_FADE } else { CLOSE_FADE });
    if on_session {
        // Mount the card's frame with the column: its border follows the same track even on the
        // first open. At zero opacity no body, plugin output or row layout is needed.
        let mut card = card_frame(p, w)
            .key("side-panel")
            .role("region")
            .label_static(opencode_i18n::t!("session.panel.reviewAndFiles"))
            // No ring, as `#review-panel` (`tabIndex={-1}`, `outline-none`, `side-region.tsx:105-109`).
            .focusable(col_open)
            .tab_index(-1)
            .no_focus_ring()
            .grow(1.0)
            .min_h(0.0);
        if shown && side_mounted(host) && let Some(reg) = resolve_side_region(host) {
            let env_scale = cx.env().scale;
            let col_h = cx.layout_with("side-column", |r| r[3]).unwrap_or(720.0 / host.zoom());
            card = card.child(region_body(host, p, &reg, w, col_h, env_scale));
        }
        if !col_open {
            card = card.inert();
        }
        column = column.child(card);
    }
    clip = clip.child(if col_open { column } else { column.inert() });
    outer.child(clip)
}

/// Standalone card for storybook stages (`side_stories.rs`).
pub fn story_card<H: PanelsHost + 'static>(host: &H, p: &Pal, width: f32, height: f32) -> El {
    let Some(reg) = resolve_side_region(host) else {
        return el().key("side-panel").size(width, height);
    };
    card_frame(p, width)
        .child(region_body(host, p, &reg, width, height, 2.0))
        .key("side-panel")
        .size(width, height)
}

fn card_frame(p: &Pal, width: f32) -> El {
    col().w(width).radius(10.0).fill(p.bg_base).elevate(p.raised)
}

fn region_body<H: PanelsHost + 'static>(
    host: &H,
    p: &Pal,
    reg: &ResolvedRegion,
    width: f32,
    col_h: f32,
    env_scale: f32,
) -> El {
    // A tab switch lands the contents at once. While the sidebar's flight moves the region's edge
    // they glide with it instead (`App::toggle_sidebar`): what keeps to the start rides the edge,
    // what keeps to the end stays, as Desktop's holdLandingWidth "blocks" does (`screen.tsx:171`).
    let flying = host.side().edge_flight.is_flying();
    let panes = visible_side_panes(host, reg);
    let split_h = (col_h * (1.0 - split_ratio(host))).round();

    let mut body = col()
        .key("side-body")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .clip_rounded(10.0)
        .on_background_press(|s: &mut H| focus_side_body(s));

    for (idx, &pane) in panes.iter().enumerate() {
        let ph = if panes.len() == 1 {
            col_h
        } else if idx == 0 {
            (col_h - split_h).max(0.0)
        } else {
            split_h
        };
        let pane_el = pane_box(host, p, reg, pane, (width, ph), env_scale, idx == 0);
        // The lower pane takes its height from the split, under a 1 px `border-base` rule.
        let sized = if idx == 0 {
            pane_el.grow(1.0).min_h(0.0)
        } else {
            col()
                .w_full()
                .h(split_h)
                .child(el().w_full().h(1.0).fill(p.border))
                .child(pane_el.grow(1.0).min_h(0.0))
        };
        body = body.child(sized);
    }
    if panes.len() > 1 {
        body = body.child(split_handle(host, p, col_h, split_h));
    }

    if flying { body } else { body.instant_changes() }
}

fn visible_side_panes(host: &dyn PanelsHost, reg: &ResolvedRegion) -> &'static [SessionPane] {
    let has_split = !reg.pane_entries(SessionPane::Split).is_empty()
        || closing_side_entry(host, reg, SessionPane::Split).is_some()
        || host
            .side()
            .adding
            .iter()
            .any(|n| n.session_id == reg.session_id && n.pane == SessionPane::Split);
    let has_side = !reg.pane_entries(SessionPane::Side).is_empty()
        || host
            .side()
            .adding
            .iter()
            .any(|n| n.session_id == reg.session_id && n.pane == SessionPane::Side);

    match (has_side, has_split) {
        (_, false) => &[SessionPane::Side],
        (false, true) => &[SessionPane::Split],
        (true, true) => &[SessionPane::Side, SessionPane::Split],
    }
}

/// The split's `ResizeHandle` (`direction="vertical" edge="start"`): 8 tall across the lower
/// pane's top edge, sizing it between `SPLIT_MIN_H` and the column less `SPLIT_MIN_H`.
fn split_handle<H: PanelsHost + 'static>(host: &H, p: &Pal, col_h: f32, split_h: f32) -> El {
    resize_pill(p, host.side().split_resize.dragging(), false)
        .key("side-split-resize")
        .absolute(0.0, col_h - split_h - 4.0)
        .w_full()
        .h(8.0)
        .cursor(Cursor::ResizeRow)
        .on_drag(move |s: &mut H, d: &mut Drag, tx: &mut Tx| on_split_resize_drag(s, col_h, split_h, d, tx))
}

/// A `ResizeHandle`'s hit area with its pill centred (4 by 40 across its length): `icon-muted`
/// after a 200 ms hover, `icon-base` while it drags.
pub(crate) fn resize_pill(p: &Pal, dragging: bool, horizontal: bool) -> El {
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

/// Floating top-right toggle button (`SessionReviewToggle` in `screen.tsx:408`), kept outside panel motion.
pub fn side_toggle<H: PanelsHost + 'static>(host: &H, p: &Pal) -> Option<El> {
    Some(
        side_toggle_shell(host, p)?
            .on_press_tx(|s: &mut H, tx: &mut Tx| toggle_side_region(s, tx)),
    )
}

fn side_toggle_shell(host: &dyn PanelsHost, p: &Pal) -> Option<El> {
    // A narrow window has none (`screen.tsx:456`).
    let sid = host.active_session_id()?;
    if host.settings_open() || host.narrow() || !host.session_has_header(sid) {
        return None;
    }
    let open = side_open(host);
    Some(
        el().key("side-toggle")
            .role("button")
            // `SessionHeaderActions`: "Toggle review" and its keybind in the tooltip under it.
            .label_static(opencode_i18n::t!("command.review.toggle"))
            .expanded(open)
            .help(opencode_i18n::t!("command.review.toggle"))
            .help_keys(Bind::SideToggle.keycaps())
            // On the session header's centre line, 12pt in from the panel's edge.
            .pin(None, Some(host.panel_chrome().top + 10.0), Some(20.0), None)
            .size(28.0, 28.0)
            .radius(6.0)
            .center()
            .hover(p.hover)
            .press(p.pressed)
            .hover_motion(INSTANT, INSTANT)
            .child(
                icon(&LAYOUT_RIGHT_FULL, 16.0)
                    .color(p.icon_muted)
                    .mirror_in_rtl()
                    .absolute(6.0, 6.0)
                    .opacity(if open { 1.0 } else { 0.0 })
                    .opacity_motion(SIDE_SPRING),
            )
            .child(
                icon(&LAYOUT_RIGHT, 16.0)
                    .color(p.icon_muted)
                    .mirror_in_rtl()
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
pub fn resize_handle<H: PanelsHost + 'static>(host: &H, p: &Pal, zoomed_w: f32) -> Option<El> {
    if host.active_session_id().is_none() || host.settings_open() || !side_open(host) || zoomed_w <= 0.0 {
        return None;
    }
    let x = (zoomed_w - side_width(host, zoomed_w) - 16.0).max(0.0);
    let chat = chat_width(host, zoomed_w);
    let max = chat_width_max(host, zoomed_w);
    Some(
        resize_pill(p, host.side().chat_resize.dragging(), true)
            .key("side-resize")
            .pin(Some(x), Some(host.panel_chrome().top), None, Some(8.0))
            .w(8.0)
            .cursor(Cursor::ResizeColumn)
            .on_drag(move |s: &mut H, d: &mut Drag, tx: &mut Tx| on_chat_resize_drag(s, chat, max, d, tx)),
    )
}

fn focus_side_body(host: &mut dyn PanelsHost) {
    {
        let side = host.side_mut();
        side.focus.press();
        side.menu.close();
        side.close_guard.step(CloseTabGuardEvent::RegionOpened);
    }
    host.set_composer_focused(false);
}

fn on_split_resize_drag(host: &mut dyn PanelsHost, col_h: f32, split_h: f32, d: &Drag, tx: &mut Tx) {
    tx.instant();
    let Some(sid) = host.active_session_id().map(str::to_owned) else { return };
    let stored = host.side().sessions.get(&sid).and_then(|st| st.split_ratio);
    let event = match d.phase {
        Phase::Down => ResizeEvent::Down { size: split_h, stored, clicks: d.clicks },
        Phase::Move => ResizeEvent::Move { by: -d.dy },
        Phase::Up => ResizeEvent::Up { cancelled: d.cancelled() },
    };
    match host.side_mut().split_resize.step(event) {
        ResizeEffect::Size(h) if col_h > 1.0 => {
            let h = h.clamp(SPLIT_MIN_H, SPLIT_MIN_H.max(col_h - SPLIT_MIN_H));
            session_side_mut(host, &sid).split_ratio = Some(1.0 - h / col_h);
        }
        ResizeEffect::Restore(stored) => session_side_mut(host, &sid).split_ratio = stored,
        ResizeEffect::Size(_) | ResizeEffect::None => {}
    }
}

fn on_chat_resize_drag(host: &mut dyn PanelsHost, chat: f32, max: f32, d: &Drag, tx: &mut Tx) {
    tx.instant();
    let Some(sid) = host.active_session_id().map(str::to_owned) else { return };
    let stored = host.side().sessions.get(&sid).and_then(|st| st.session_width);
    let event = match d.phase {
        Phase::Down => ResizeEvent::Down { size: chat, stored, clicks: d.clicks },
        // The chat grows toward the side region: right, or left in a right-to-left window.
        Phase::Move => ResizeEvent::Move {
            by: if opencode_i18n::direction() == opencode_i18n::Direction::Rtl { -d.dx } else { d.dx },
        },
        Phase::Up => ResizeEvent::Up { cancelled: d.cancelled() },
    };
    match host.side_mut().chat_resize.step(event) {
        ResizeEffect::Size(w) => {
            session_side_mut(host, &sid).session_width = Some(w.clamp(SESSION_MIN_W, max.max(SESSION_MIN_W)));
        }
        ResizeEffect::Restore(stored) => session_side_mut(host, &sid).session_width = stored,
        ResizeEffect::None => {}
    }
}

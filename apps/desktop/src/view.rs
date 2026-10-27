//! Views: the shell (sidebar + panel), the home page and the session page. Values follow
//! OpenCode Desktop's v2 styles (`packages/app`, `packages/session-ui`, `packages/ui` tokens).

use std::sync::OnceLock;

use rune::{
    Anim, AnimExt, Color, Curve, Cursor, Cx, Cycle, Drag, Easing, El, GrowthSpec, Icon, Key, Phase, ScrollSpec,
    Align, Shadow, Side, Spring, TextStyle, Tx, col, edge, el, icon, row, spacer, text, text_input,
};

use rune_kit::{Kit, Markdown, MarkdownStyle};

use rune_theme_opencode::icons as ic;
use opencode_kit::model::{self, Assistant, InboxItem, Message, Part, Pending, SessionInfo, Transcript};
use crate::palette::{Pal, pal};
use crate::{App, Route, TabStatus};
use crate::pickers::Kind;
use crate::turns::{self, Block, Step};

// Type: Inter variable (weights 440/530/670 are real instances) and IBM Plex Mono.
pub(crate) const UI: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
pub(crate) const UI_MED: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
pub(crate) const UI_SMALL: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);
const AVATAR: TextStyle = TextStyle::new(11.0, 530.0).lh(16.0);
const MD: TextStyle = TextStyle::new(14.0, 400.0).lh(22.4);
pub(crate) const BUBBLE: TextStyle = TextStyle::new(14.0, 400.0).lh(21.0);
const H1: TextStyle = TextStyle::new(17.0, 600.0).lh(24.0);
const H2: TextStyle = TextStyle::new(15.0, 600.0).lh(24.0);
const H3: TextStyle = TextStyle::new(13.0, 600.0).lh(20.0);
const CODE: TextStyle = TextStyle::new(13.0, 400.0).mono().lh(20.0);
const PROMPT: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0);
const SHELL_PROMPT: TextStyle = TextStyle::new(13.0, 400.0).mono().lh(20.0);

/// `--container-session`: 744 including 24 px padding on each side.
pub(crate) const CONTENT_W: f32 = 696.0;

/// The window, drawn at the app's zoom: the shell is laid out at 1/zoom of the window and
/// scaled up about the centre, so text re-rasterizes crisp and hits follow what's drawn. The
/// outer, unscaled element is always mounted (it reports the window's size and keeps every
/// path below it stable across zoom changes).
pub fn app(s: &App, cx: &mut Cx<App>) -> El {
    let p = pal(s.dark(cx));
    crate::icons::prepare(s, cx);
    let mut inner = shell(s, cx);
    if s.pickers.open.is_some() {
        inner = inner.child(el().key("picker-dismiss").absolute(0.0, 0.0).w_full().h_full()
            .on_press_tx(|s: &mut App, tx: &mut Tx| {
                // Nothing gets focus back, so the search field can unmount in this frame.
                tx.instant();
                s.close_picker(false);
            }))
            .child(picker(s, p));
    }
    inner = inner.children(crate::agents_view::overlay(s, p));
    let composer_w = cx.layout("composer").map_or(0.0, |r| r[2]);
    inner = inner.children(crate::mentions::view(s, p, composer_w));
    inner = inner.children(crate::add_menu::commands_view(s, p, composer_w));
    inner = inner.children(crate::add_menu::menu(s, p));
    if s.switcher.open {
        inner = inner.children(crate::switcher::view(s, p, cx));
    }
    inner = inner.children(crate::attachments::viewer(s, p, cx));
    let (w, h) = cx.layout("zoom").map_or((0.0, 0.0), |r| (r[2], r[3]));
    let sized = if w > 0.0 && h > 0.0 { el().w(w / s.zoom).h(h / s.zoom) } else { el().w_full().h_full() };
    el().key("zoom")
        .w_full()
        .h_full()
        .center()
        // Hovers land and leave at once, like Desktop.
        .hover_motion_within(INSTANT, INSTANT)
        .child(sized.key("zoomed").scale(s.zoom).child(inner))
        .child(crate::knobs::layer(s))
}

fn shell(s: &App, cx: &mut Cx<App>) -> El {
    if !s.playground.is_empty() {
        return crate::icons::playground(s);
    }
    let p = pal(s.dark(cx));
    cx.reduced_motion(s.reduced_motion);
    let w = s.sidebar_w;
    let open = s.sidebar_open;
    let mut root = row()
        .key("window")
        .role("window")
        .fill(p.bg_deep)
        .w_full()
        .h_full()
        .child(sidebar(s, p))
        .child(
            col()
                .key("panel")
                .grow(1.0)
                .shrink()
                .ml(8.0)
                .mt(8.0)
                .mr(8.0)
                .mb(8.0)
                .radius(10.0)
                .fill(p.bg_base)
                .border(0.5, p.ring)
                .shadow(2.0, 4.0, p.shadow)
                .shadow(1.0, 2.0, p.shadow)
                .clip_soft(0.0)
                // Files dropped anywhere on the page attach, as in Desktop.
                .on_drop_files(crate::attachments::drop_files)
                .on_drop_hover(crate::attachments::drop_hover)
                .child(if s.story.is_some() { crate::stories::page(s, p) } else if s.settings.open { crate::settings::page(s, p) } else { match &s.route {
                    Route::Home => home(s, p),
                    Route::Session(id) => session_page(s, p, id),
                    Route::Draft => draft_page(s, p, cx.layout("panel").map_or(0.0, |r| r[2] / s.zoom)),
                } })
                .children(crate::attachments::drop_overlay(s, p, cx.layout("panel").map_or((0.0, 0.0), |r| (r[2] / s.zoom, r[3] / s.zoom)))),
        )
        // The empty inset above the panels moves the window, as the titlebar does.
        .child(el().key("drag-inset").absolute(0.0, 0.0).w_full().h(8.0).window_drag());
    if open {
        root = root.child(resize_handle(s, p, w));
    }
    root = root.child(sidebar_toggle(p, open, w));
    root
}

// ---------------------------------------------------------------------------------------------
// Pieces

pub(crate) fn avatar(p: &Pal, name: &str) -> El {
    let k = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) as usize % p.avatars.len();
    let (bg, border) = p.avatars[k];
    let letter = name.chars().find(|c| c.is_alphanumeric()).map_or("?".into(), |c| c.to_uppercase().to_string());
    el().size(16.0, 16.0)
        .radius(4.0)
        .fill(bg)
        .border(0.5, border)
        .center()
        .child(text(letter).font(AVATAR).color(p.avatar_fg))
}

fn roots(s: &App) -> impl Iterator<Item = &SessionInfo> {
    s.sessions.iter().filter(|x| x.parent.is_none())
}

// ---------------------------------------------------------------------------------------------
// Sidebar: Desktop's vertical tabs (`shell/titlebar/titlebar.tsx`, `tab-nav.tsx`).

/// Default, minimum and maximum widths (`shell.tsx`'s ResizeHandle).
pub const SIDEBAR_W: f32 = 260.0;
pub const SIDEBAR_MIN: f32 = 120.0;
pub const SIDEBAR_MAX: f32 = 640.0;
/// The sidebars' shared spring, "smooth" (`panel-motion.ts`).
const SIDEBAR_SPRING: Spring = Spring::new(0.3, 0.0);
/// Where the zoom button ends with the lights on the header's centre line (`macVerticalTrafficLights`).
const TRAFFIC_LIGHTS_END: f32 = 85.0;
/// The collapsed toggle's box: from the shell inset plus header padding (18) to past the lights.
const COLLAPSED_SLOT: f32 = TRAFFIC_LIGHTS_END + 16.0 - 18.0 + 32.0;
/// How far a closed panel's contents dim at the end of the flight.
const FADE_FLOOR: f32 = 0.5;

const INSTANT: Anim = Anim::INSTANT;

/// Home, Search sessions and New session (`data-titlebar-tab-action`).
fn action_row(p: &Pal, key: &str, glyph: &'static Icon, label: &str, hint: &str, pressed: bool) -> El {
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
        .child(text(label).font(UI).color(ink).group_color(p.text).truncate());
    if pressed {
        r = r.fill(p.pressed);
    }
    if !hint.is_empty() {
        r = r.child(spacer()).child(text(hint).font(UI).color(p.faint).group_opacity(0.0, 1.0));
    }
    r
}

fn sidebar(s: &App, p: &Pal) -> El {
    let current = match &s.route {
        Route::Session(id) => Some(s.root_of(id)),
        _ => None,
    };
    // No gap: each tab's hit area takes half the space to its neighbours (see `tab_row`).
    let mut list = col().key("tabs").w_full().pb(16.0);
    for (i, t) in s.tabs.iter().enumerate() {
        list = list.child(tab_row(s, p, t, i, current == Some(t.id.as_str()), s.tabs.len()));
    }

    let w = s.sidebar_w;
    let inner = col()
        .key("sidebar-inner")
        // Pinned to the aside's end edge: it slides out as the aside narrows.
        .absolute(if s.sidebar_open { 0.0 } else { -w }, 0.0)
        .w(w)
        .h_full()
        .pt(8.0)
        .pl(10.0)
        .pr(2.0)
        .pb(10.0)
        .opacity(if s.sidebar_open { 1.0 } else { FADE_FLOOR })
        .opacity_motion(SIDEBAR_SPRING)
        .child(
            // Shares the session header's 48px centre line with the traffic lights; drags the window.
            row()
                .key("sidebar-header")
                .w_full()
                .h(48.0)
                .mb(8.0)
                .gap(8.0)
                .center_y()
                .window_drag()
                .child(el().key("lights").w(TRAFFIC_LIGHTS_END - 10.0).h(28.0))
                .child(spacer())
                // The toggle floats above (see `sidebar_toggle`); its slot keeps the row's shape.
                .child(el().key("toggle-slot").size(28.0, 28.0)),
        );
    // The storybook (⌘⇧D) lists its stories in place of the tabs.
    let inner = match crate::stories::list(s, p) {
        Some(stories) => inner.child(col().key("stories").w_full().grow(1.0).shrink().min_h(0.0).child(stories)),
        None => inner.child(
            col()
                .key("actions")
                .w_full()
                .gap(4.0)
                .child(action_row(p, "home", &ic::GRID_PLUS, "Home", "⌘B", s.route == Route::Home).on_press_tx(
                    |s: &mut App, tx: &mut Tx| {
                        s.opening = None;
                        s.route = Route::Home;
                        tx.settle();
                    },
                ))
                .child(action_row(p, "search", &ic::MAGNIFYING_GLASS, "Search sessions", "", false).on_press_tx(
                    |s: &mut App, tx: &mut Tx| {
                        s.opening = None;
                        s.route = Route::Home;
                        s.search.focused = true;
                        tx.settle();
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
        ),
    };

    // The aside's width moves the panel's edge; its contents keep their width, pinned to that edge.
    el().key("sidebar")
        .role("navigation")
        .w(if s.sidebar_open { w } else { 0.0 })
        .h_full()
        
        .clip_soft(0.0)
        .motion(SIDEBAR_SPRING)
        .child(inner)
}

const TAB_H: f32 = 28.0;
const TAB_GAP: f32 = 4.0;
/// How far a tab steps aside for a dragged one.
const TAB_PITCH: f32 = TAB_H + TAB_GAP;
/// The tabs a dragged one passes step aside, and a dropped one settles, on this: Beta's
/// sortable transition (250 ms, `cubic-bezier(0.25, 1, 0.5, 1)`) as a spring with no bounce.
const REORDER_SPRING: Spring = Spring::new(0.25, 0.0);

/// `t` of the way from `a` to `b` in gamma-encoded sRGB, as CSS opacity blends.
fn srgb_mix(a: Color, b: Color, t: f32) -> Color {
    let m = |x: f32, y: f32| { use rune::color::{srgb_decode as dec, srgb_encode as enc}; dec(enc(x) + (enc(y) - enc(x)) * t) };
    Color { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b), a: 1.0 }
}

/// Source-over of straight-alpha colours, for a lifted tab that hides the rows it passes.
fn over(top: Color, bottom: Color) -> Color {
    let a = top.a + bottom.a * (1.0 - top.a);
    if a <= 0.0 {
        return Color::TRANSPARENT;
    }
    let mix = |t: f32, b: f32| (t * top.a + b * bottom.a * (1.0 - top.a)) / a;
    Color { r: mix(top.r, bottom.r), g: mix(top.g, bottom.g), b: mix(top.b, bottom.b), a }
}

/// A tab's hit area spans half the gap to each neighbour, so the pointer is always over one tab
/// (Beta's "fill vertical tab hover gaps"). Only the 28pt pill inside it paints.
fn tab_row(s: &App, p: &Pal, t: &crate::tabs::Tab, index: usize, active: bool, count: usize) -> El {
    let info = s.session(&t.id);
    let title = info.map(title_of).unwrap_or_else(|| if t.title.is_empty() { "Untitled session".into() } else { t.title.clone() });
    let dir = info.map_or(t.directory.as_str(), |i| i.directory.as_str());
    let project = info.and_then(|i| s.project(&i.project));
    let proj = project
        .map(|x| x.name.clone())
        .unwrap_or_else(|| dir.rsplit('/').find(|x| !x.is_empty()).unwrap_or("").to_owned());
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
        .on_hover_change(move |s: &mut App, inside, _: &mut Tx| if inside { s.prefetch_session(&hover_id); })
        .on_key(move |s: &mut App, ev, _: &mut Tx| s.prefetch_navigation(&nav_ids, index, ev))
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
                .child(text(title).font(UI_MED).color(if active { p.text } else { p.faint })),
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
    }
    if dragging {
        // Beta lifts it with the pressed overlay alone; here it's opaque on the sidebar's deep
        // background so the rows it passes don't show through.
        r = r.fill(over(p.pressed, p.bg_deep));
    }
    // Its layout and its step aside share one spring, so a drop that swaps one for the other
    // leaves a passed tab still and slides the dropped one from where it was drawn.
    hit = hit.child(r).motion(REORDER_SPRING).offset(0.0, s.tab_drag.shift(index, TAB_PITCH));
    if dragging {
        // It follows the pointer exactly.
        hit = hit.raise().offset_motion(INSTANT);
    }
    hit
}

/// The sidebar edge's resize handle: an 8px hit area across the gap, with a pill that waits a
/// moment before showing on hover (`resize-handle.css`).
fn resize_handle(s: &App, p: &Pal, w: f32) -> El {
    let enter = Curve::tween(0.15, Easing::cubic(0.2, 0.8, 0.2, 1.0)).delay(0.2);
    let exit = Curve::tween(0.11, Easing::cubic(0.4, 0.0, 1.0, 1.0));
    let dragging = s.sidebar_drag.is_some();
    el().key("sidebar-resize")
        .absolute(w, 0.0)
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
fn sidebar_toggle(p: &Pal, open: bool, w: f32) -> El {
    let x = if open { w - 2.0 - 28.0 } else { 18.0 + COLLAPSED_SLOT - 4.0 - 28.0 };
    el().key("sidebar-toggle")
        .role("button")
        .label("Toggle sidebar")
        .absolute(x, 18.0)
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
fn collapsed_slot(s: &App) -> El {
    // Beta measures from the header's 10px padding; this header pads 14.
    el().key("toggle-space").w(if s.sidebar_open { 0.0 } else { COLLAPSED_SLOT - 4.0 }).h(28.0).motion(SIDEBAR_SPRING)
}

/// Desktop's activity spinner, "Soft Radar": a 5×5 dot matrix whose sweep head steps clockwise
/// around each square ring, leaving a fading trail (`spinner-radar.ts`), 14px, icon-muted, over
/// the page.
pub(crate) fn activity_ring(p: &Pal) -> El {
    activity_ring_on(p, p.bg_base, 14.0)
}

/// The spinner at `size` over the opaque `bg`. Its dots' opacities are Desktop's (0.2 at rest, 1
/// lit), re-solved so a dot over `bg` comes out the pixel a browser blends in sRGB; blended as is
/// in linear light, a resting dot would show at about twice its brightness.
pub(crate) fn activity_ring_on(p: &Pal, bg: Color, size: f32) -> El {
    let mut r = el().key("ring").size(size, size);
    for (k, d) in ring_dots(p.icon_muted, bg, size).iter().enumerate() {
        let mut dot = el().key(format!("d{k}")).absolute(d.x, d.y).size(d.size, d.size).radius(d.size * 0.24).fill(p.icon_muted);
        dot = match d.cycle {
            Some(c) => dot.cycle(c),
            None => dot.opacity(d.still),
        };
        r = r.child(dot);
    }
    r
}

struct RadarDot {
    x: f32,
    y: f32,
    size: f32,
    still: f32,
    cycle: Option<Cycle>,
}

/// The opacity that, blended in linear light, gives the pixel `a` gives in sRGB.
fn srgb_opacity(a: f32, fg: Color, bg: Color) -> f32 {
    use rune::theme::{srgb_decode, srgb_encode};
    let solve = |f: f32, b: f32| {
        let t = srgb_decode(a * srgb_encode(f) + (1.0 - a) * srgb_encode(b));
        ((t - b) / (f - b)).clamp(0.0, 1.0)
    };
    let pairs = [(fg.r, bg.r), (fg.g, bg.g), (fg.b, bg.b)];
    let solved: Vec<f32> = pairs.iter().filter(|(f, b)| (f - b).abs() > 1e-4).map(|&(f, b)| solve(f, b)).collect();
    if solved.is_empty() { a } else { solved.iter().sum::<f32>() / solved.len() as f32 }
}

/// The radar's dots for one colour, backdrop and size, built once each.
fn ring_dots(fg: Color, bg: Color, size: f32) -> &'static [RadarDot] {
    type Key = [u32; 7];
    static BUILT: std::sync::Mutex<Vec<(Key, &'static [RadarDot])>> = std::sync::Mutex::new(Vec::new());
    let key: Key = [fg.r, fg.g, fg.b, bg.r, bg.g, bg.b, size].map(f32::to_bits);
    let mut built = BUILT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&(_, dots)) = built.iter().find(|(k, _)| *k == key) {
        return dots;
    }
    let scale = size / 16.0;
    let op = |a: f32| srgb_opacity(a, fg, bg);
    let dots: Vec<RadarDot> = radar()
        .iter()
        .map(|d| {
            let (x, y, side) = (d.x * scale, d.y * scale, d.size * scale);
            let still = op(d.still);
            let cycle = d.stops.as_ref().map(|(at, stops)| {
                let stops: Vec<(f32, f32)> = stops.iter().map(|&(t, a)| (t, op(a))).collect();
                Cycle { period: RADAR_LOOP, phase: *at, stops: Box::leak(stops.into_boxed_slice()), still, fps: 30.0 }
            });
            RadarDot { x, y, size: side, still, cycle }
        })
        .collect();
    let dots: &'static [RadarDot] = Box::leak(dots.into_boxed_slice());
    built.push((key, dots));
    dots
}

const RADAR_LOOP: f32 = 1.8;

/// One dot in Desktop's 16-unit viewBox, with its opacities as the browser blends them:
/// `stops` is where in the loop it lights and its keyframes; none for the steady centre.
struct Dot16 {
    x: f32,
    y: f32,
    size: f32,
    still: f32,
    stops: Option<(f32, Vec<(f32, f32)>)>,
}

fn radar() -> &'static [Dot16] {
    static DOTS: OnceLock<Vec<Dot16>> = OnceLock::new();
    DOTS.get_or_init(|| {
        const SPAN: i32 = 5;
        const DIM: f32 = 0.2;
        const EDGE: f32 = 1.5;
        let r3 = |v: f32| (v * 1000.0).round() / 1000.0;
        // Soft Radar's trail from 0.55, sampled every 4% over 48% of the loop.
        let fade = [1.0, 0.987, 0.934, 0.812, 0.599, 0.384, 0.238, 0.143, 0.081, 0.041, 0.017, 0.004, 0.0];
        let mut trail: Vec<(f32, f32)> = fade.iter().enumerate().map(|(i, l)| (i as f32 * 4.0, r3(0.55 * l))).collect();
        trail.push((100.0, 0.0));
        let level_at = |stops: &[(f32, f32)], pc: f32| -> f32 {
            let Some(k) = stops.iter().position(|s| s.0 >= pc) else { return stops.last().map_or(0.0, |s| s.1) };
            if k == 0 {
                return stops[0].1;
            }
            let ((p0, l0), (p1, l1)) = (stops[k - 1], stops[k]);
            l0 + (l1 - l0) * (pc - p0) / if p1 - p0 == 0.0 { 1.0 } else { p1 - p0 }
        };
        let frac = |v: f32| v - v.floor();
        let cells: Vec<(i32, i32)> = (0..SPAN * SPAN).map(|i| (i % SPAN - SPAN / 2, i / SPAN - SPAN / 2)).collect();
        let angle = |(gx, gy): (i32, i32)| frac((gx as f32).atan2(-(gy as f32)) / std::f32::consts::TAU);
        let ring_of = |(gx, gy): (i32, i32)| gx.abs().max(gy.abs());
        cells
            .iter()
            .map(|&c| {
                let (x, y, size) = (8.0 + c.0 as f32 * 3.0 - 1.0, 8.0 + c.1 as f32 * 3.0 - 1.0, 2.0);
                if c == (0, 0) {
                    // The steady centre glows like the fresh trail.
                    return Dot16 { x, y, size, still: DIM + (1.0 - DIM) * trail[0].1, stops: None };
                }
                let at = r3(angle(c));
                let others = cells.iter().filter(|&&o| o != c && o != (0, 0) && ring_of(o) == ring_of(c));
                let behind = others.clone().map(|&o| r3(frac(at - r3(angle(o))))).fold(1.0f32, f32::min) * 100.0;
                let ahead = others.map(|&o| r3(frac(r3(angle(o)) - at))).fold(1.0f32, f32::min) * 100.0;
                // "step": lit while it's the dot nearest the head, then the trail.
                let off = ahead / 2.0 + EDGE;
                let on = 100.0 - behind / 2.0;
                let mut shape = vec![(0.0, 1.0), (ahead / 2.0, 1.0), (off, level_at(&trail, off))];
                shape.extend(trail.iter().copied().filter(|&(pc, _)| pc > off && pc < 100.0 && pc < on - EDGE));
                shape.extend([(on - EDGE, 0.0), (on, 1.0), (100.0, 1.0)]);
                let still = DIM + (1.0 - DIM) * level_at(&shape, frac(0.22 - at) * 100.0);
                let stops = shape.iter().map(|&(pc, l)| (pc / 100.0, DIM + (1.0 - DIM) * l)).collect();
                Dot16 { x, y, size, still, stops: Some((at, stops)) }
            })
            .collect()
    })
}

pub(crate) fn title_of(s: &SessionInfo) -> String {
    if s.title.is_empty() { "Untitled session".into() } else { s.title.clone() }
}

// ---------------------------------------------------------------------------------------------
// Home: search and recent sessions by day (Desktop's home sessions column).

fn home(s: &App, p: &Pal) -> El {
    let q = s.search.text.to_lowercase();
    let mut groups: Vec<(&str, Vec<&SessionInfo>)> = vec![("Today", vec![]), ("Yesterday", vec![]), ("Older", vec![])];
    let day = |ms: i64| ms.div_euclid(86_400_000);
    for ses in roots(s).filter(|x| q.is_empty() || x.title.to_lowercase().contains(&q)).take(60) {
        let d = day(s.now_ms) - day(ses.updated);
        groups[if d <= 0 { 0 } else if d == 1 { 1 } else { 2 }].1.push(ses);
    }
    let nav_ids = std::rc::Rc::new(groups.iter().flat_map(|(_, items)| items.iter().map(|s| s.id.clone())).collect::<Vec<_>>());
    let mut list = col().key("groups").role("list").focus_group().w_full().pr(12.0).pb(64.0);
    let mut first = true;
    for (title, items) in groups.into_iter().filter(|g| !g.1.is_empty()) {
        let mut header = row().key(format!("h-{title}")).h(28.0).pl(12.0).center_y().child(text(title).font(UI).color(p.muted));
        if first {
            header = header.child(spacer()).child(
                row()
                    .key("new-session")
                    .role("button")
                    .h(28.0)
                    .px(8.0)
                    .gap(6.0)
                    .center_y()
                    .radius(6.0)
                    .hover(p.hover)
                    .on_press_tx(|s: &mut App, tx: &mut Tx| s.new_session(tx))
                    .child(icon(&ic::EDIT, 16.0).color(p.icon_muted))
                    .child(text("New session").font(UI_MED).color(p.muted)),
            );
        }
        let mut rows = col().key(format!("g-{title}")).gap(1.0).pt(16.0).mb(24.0);
        for ses in items {
            let project = s.project(&ses.project);
            let proj = project.map_or("", |x| x.name.as_str());
            let id = ses.id.clone();
            let hover_id = id.clone();
            let index = nav_ids.iter().position(|x| x == &id).unwrap_or(0);
            let nav = nav_ids.clone();
            rows = rows.child(
                row()
                    .key(format!("r-{}", ses.id))
                    .role("button")
                    .label(ses.title.clone())
                    .h(40.0)
                    .pl(12.0)
                    .pr(16.0)
                    .gap(6.0)
                    .center_y()
                    .radius(6.0)
                    .hover(p.hover)
                    .press(p.pressed)
                    .on_hover_change(move |s: &mut App, inside, _: &mut Tx| if inside { s.prefetch_session(&hover_id); })
                    .on_key(move |s: &mut App, ev, _: &mut Tx| s.prefetch_navigation(&nav, index, ev))
                    .on_press_tx(move |s: &mut App, tx: &mut Tx| s.open(&id, tx))
                    .child(crate::icons::badge(s, p, project, proj))
                    .child(text(title_of(ses)).font(UI_MED).color(p.text).truncate())
                    .child(text(proj.to_owned()).font(UI).color(p.faint).truncate())
                    .child(spacer())
                    .child(text(model::ago(s.now_ms, ses.updated)).font(UI).color(p.faint)),
            );
        }
        list = list.child(header).child(rows);
        first = false;
    }
    if !q.is_empty() && first {
        list = list.child(
            row().key("no-results").h(40.0).pl(12.0).center_y().child(text(format!("No sessions found for {}", s.search.text)).font(UI).color(p.muted)),
        );
    }

    let search = row()
        .key("search")
        .h(36.0)
        .pl(12.0)
        .pr(8.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .fill(p.search_bg)
        .hover(p.layer2)
        .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
        .child(
            text_input(&s.search, UI.lh(20.0))
                .key("search-input")
                .grow(1.0)
                .shrink()
                .h(20.0)
                .placeholder("Search sessions", p.faint)
                .caret(p.text, p.selection)
                .on_edit(|s: &mut App| &mut s.search)
                .on_input_key(|s: &mut App, ev, tx| s.shortcut(ev, tx)),
        );

    col()
        .key("home")
        .grow(1.0)
        .w_full()
        .scroll(ScrollSpec { stick_bottom: false })
        .clip_soft(0.0)
        .child(
            col()
                .key("home-content")
                .w_full()
                .center()
                .px(24.0)
                .pt(48.0)
                .child(col().key("home-column").w_full().max_w(720.0).child(search).child(list.mt(12.0))),
        )
}

// ---------------------------------------------------------------------------------------------
// Session

/// The new-session page, as Desktop's `new-session/view.tsx`: the wordmark about a quarter of
/// the way down, the composer 32 below it with where the session will start under that.
/// `width`: the panel's width last frame.
fn draft_page(s: &App, p: &Pal, width: f32) -> El {
    let content = col()
        .key("draft-content")
        .w_full()
        .max_w(880.0)
        .child(wordmark(p, width))
        .child(col().key("draft-composer").w_full().mt(32.0).gap(4.0).child(composer(s, p).max_w(880.0)).child(location(s, p)));
    col()
        .key("draft")
        .grow(1.0)
        .w_full()
        .child(row().key("header").h(48.0).pl(14.0).center_y().window_drag().child(collapsed_slot(s)))
        // Flex shares that put the wordmark near Desktop's 25.375% from the top at usual sizes.
        .child(el().key("above").grow(0.3))
        .child(col().key("draft-column").w_full().px(24.0).center().child(content))
        .child(el().key("below").grow(0.7))
        .children(s.debug.then(|| status_bar(s, p, None)))
}

/// OpenCode's wordmark (`ui/logo.tsx`) as Desktop's new-session page draws it at rest: every
/// letter in the icon colour at 16%, the counters fainter still. It spans 80% of up to 720 pt,
/// scaled so its 6-unit grid lands on whole points.
fn wordmark(p: &Pal, width: f32) -> El {
    // x, y, w, h in the logo's 234×42 units; `true` for a counter's lower half.
    const RECTS: [(f32, f32, f32, f32, bool); 40] = [
        (0., 6., 24., 6., false), (0., 12., 6., 24., false), (18., 12., 6., 24., false), (6., 30., 12., 6., false), (6., 18., 12., 12., true),
        (30., 6., 24., 6., false), (30., 12., 6., 30., false), (48., 12., 6., 24., false), (36., 30., 12., 6., false), (36., 18., 12., 12., true),
        (60., 6., 24., 6., false), (60., 12., 6., 24., false), (78., 12., 6., 12., false), (66., 18., 12., 6., false), (66., 30., 18., 6., false), (66., 24., 18., 6., true),
        (90., 6., 18., 6., false), (90., 12., 6., 24., false), (108., 12., 6., 24., false), (96., 18., 12., 18., true),
        (120., 6., 24., 6., false), (120., 12., 6., 18., false), (120., 30., 24., 6., false), (126., 18., 18., 12., true),
        (150., 6., 24., 6., false), (150., 12., 6., 24., false), (168., 12., 6., 24., false), (156., 30., 12., 6., false), (156., 18., 12., 12., true),
        (180., 6., 18., 6., false), (198., 0., 6., 36., false), (180., 12., 6., 24., false), (186., 30., 12., 6., false), (186., 18., 12., 12., true),
        (210., 6., 24., 6., false), (210., 12., 6., 24., false), (228., 12., 6., 12., false), (216., 18., 12., 6., false), (216., 30., 18., 6., false), (216., 24., 18., 6., true),
    ];
    let span = 0.8 * if width > 0.0 { (width - 48.0).min(720.0) } else { 720.0 };
    let k = ((span / 39.0).round().max(2.0)) / 6.0;
    // Mixed in sRGB as the browser composites the 16% group, so it's as faint as Desktop's.
    let (stem, weak) = (srgb_mix(p.bg_base, p.icon, 0.16), srgb_mix(p.bg_base, p.icon, 0.16 * 0.3));
    let mut logo = el().key("wordmark").size(234.0 * k, 42.0 * k);
    for (x, y, w, h, counter) in RECTS {
        logo = logo.child(el().absolute(x * k, y * k).size(w * k, h * k).fill(if counter { weak } else { stem }));
    }
    row().key("wordmark-row").w_full().justify_center().child(logo)
}

/// Where the new session starts: the project's avatar and name, which open the project
/// picker, and its folder, faint at the end.
fn location(s: &App, p: &Pal) -> El {
    let project = s.draft_project();
    let name = project.map_or_else(|| s.draft_dir.rsplit('/').next().unwrap_or("").to_owned(), |x| x.name.clone());
    let dir = s.draft_dir.replace(&home_dir(), "~");
    let open = s.pickers.open == Some(Kind::Project);
    let button = row()
        .key("project")
        .role("button")
        .label("Choose a project")
        .h(28.0)
        .pl(6.0)
        .pr(8.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .fill(if open { p.hover } else { Color::TRANSPARENT })
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(|s: &mut App, tx: &mut Tx| { s.open_picker(Kind::Project); if s.pickers.open.is_some() { tx.instant(); } })
        .child(crate::icons::badge(s, p, project, &name))
        .child(text(name.clone()).font(UI).color(if open { p.text } else { p.muted }).truncate());
    row()
        .key("location")
        .w_full()
        .min_h(36.0)
        .pl(8.0)
        .pr(6.0)
        .gap(12.0)
        .center_y()
        .child(button)
        .child(spacer())
        .child(text(dir).font(UI).color(p.faint).truncate().shrink().pr(6.0))
}

pub(crate) fn session_page(s: &App, p: &Pal, id: &str) -> El {
    let t = s.transcripts.get(id).filter(|_| !s.loading_session(id));
    let page = col().key("session").grow(1.0).w_full().child(header(s, p, s.session(id), t));
    session_body(s, p, id, page)
}

/// The session page under its header: the timeline, and what sits under it.
fn session_body(s: &App, p: &Pal, id: &str, page: El) -> El {
    let info = s.session(id);
    let t = s.transcripts.get(id).filter(|_| !s.loading_session(id));
    let child = info.is_some_and(|i| i.parent.is_some());
    let mut page = page
        .child(crate::jump::over(s, p, id, timeline(s, p, id, t)))
        .child(requests(s, p, id));
    // A subagent takes no prompts: its status line stands where the composer would.
    page = if child {
        page.child(crate::subagents::bar(s, p, id))
    } else {
        page.children(crate::agents_view::dock(s, p, id)).child(composer_dock(s, p))
    };
    page = page.children(s.debug.then(|| status_bar(s, p, info)));
    if s.opening.is_some() { page.inert() } else { page }
}

/// The storybook's stages (`stories.rs`): the open session's timeline and what sits under it,
/// its header, and its composer with the subagent capsule over it.
pub(crate) fn story_session(s: &App, p: &Pal) -> El {
    let Route::Session(id) = &s.route else { return el() };
    session_body(s, p, id, col().key("session").grow(1.0).w_full().min_h(0.0))
}

pub(crate) fn story_header(s: &App, p: &Pal) -> El {
    let Route::Session(id) = &s.route else { return el() };
    header(s, p, s.session(id), s.transcripts.get(id))
}

pub(crate) fn story_composer(s: &App, p: &Pal) -> El {
    let Route::Session(id) = &s.route else { return composer_dock(s, p) };
    col().key("session")
        .w_full()
        .children(crate::agents_view::dock(s, p, id))
        .child(composer_dock(s, p))
}

fn header(s: &App, p: &Pal, info: Option<&SessionInfo>, t: Option<&Transcript>) -> El {
    let mut crumbs = row().key("crumbs").gap(0.0).center_y().shrink();
    if let Some(info) = info {
        let project = s.project(&info.project);
        let proj = project.map_or("", |x| x.name.as_str());
        crumbs = crumbs
            .child(crate::icons::badge(s, p, project, proj).mr(8.0))
            .child(text(proj.to_owned()).font(UI_MED).color(p.faint))
            .child(text("/").font(TextStyle::new(11.0, 500.0)).color(p.faint).pl(8.0).pr(4.0));
        if let Some(parent) = info.parent.as_deref().and_then(|pid| s.session(pid)) {
            let pid = parent.id.clone();
            crumbs = crumbs
                .child(
                    row().key("parent-crumb").role("button").label(format!("Open {}", title_of(parent))).h(24.0).px(4.0).center_y().radius(4.0)
                        .hover(p.hover).hover_motion(INSTANT, INSTANT)
                        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                            tx.instant();
                            s.open(&pid, tx);
                        })
                        .child(text(title_of(parent)).font(UI_MED).color(p.faint).truncate()),
                )
                .child(text("/").font(TextStyle::new(11.0, 500.0)).color(p.faint).pl(4.0).pr(4.0));
        }
        crumbs = crumbs.child(text(title_of(info)).font(UI_MED).color(p.text).truncate().pl(4.0));
        // A running subagent's header spins beside its title (`SessionWorkingIndicator`).
        if info.parent.is_some() && s.subtree_running(&info.id) {
            crumbs = crumbs.child(activity_ring(p).key("working").ml(6.0));
        }
    }
    // Context and spend: the last finished step's context window, the session's cost.
    let last = t.and_then(|t| {
        t.messages.iter().rev().find_map(|m| match m {
            Message::Assistant(a) if a.tokens.context() > 0 => Some(a.tokens.context()),
            _ => None,
        })
    });
    let ctx = last.unwrap_or(0);
    // The session's own total (it includes subagents); the transcript's sum when it's ahead.
    let summed = t.map_or(0.0, |t| t.messages.iter().map(|m| if let Message::Assistant(a) = m { a.cost } else { 0.0 }).sum::<f64>());
    let cost = info.map_or(0.0, |i| i.cost).max(summed);
    // The session model's context window, or 200k when the server doesn't say.
    let limit = info
        .and_then(|i| s.pickers.limits.get(&format!("{}:{}", i.model.provider, i.model.id)))
        .map_or(200_000.0, |&n| n.max(1) as f32);
    let usage = row()
        .key("usage")
        .gap(6.0)
        .center_y()
        .child(ring(p, (ctx as f32 / limit).min(1.0)))
        .child(text(model::tokens_short(ctx)).font(UI_SMALL).color(p.muted))
        .child(text(format!("${cost:.2}")).font(UI_SMALL).color(p.faint));
    row()
        .key("header")
        .h(48.0)
        .pl(14.0)
        .pr(14.0)
        .gap(8.0)
        .center_y()
        .window_drag()
        .child(collapsed_slot(s))
        .child(crumbs)
        .child(spacer())
        .children(t.filter(|_| last.is_some()).map(|_| usage))
}

/// Context ring: a track circle and an arc, both icons built once per 1/32 step.
fn ring(p: &Pal, frac: f32) -> El {
    static ARCS: OnceLock<Vec<Icon>> = OnceLock::new();
    static TRACK: Icon = Icon { name: "ring-track", view: 16.0, body: r#"<circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="2"/>"# };
    let arcs = ARCS.get_or_init(|| {
        (0..=32)
            .map(|k| {
                let f = k as f32 / 32.0;
                let body = if k == 0 {
                    String::new()
                } else if k == 32 {
                    r#"<circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="2"/>"#.to_owned()
                } else {
                    let a = f * std::f32::consts::TAU;
                    let (x, y) = (8.0 + 6.0 * a.sin(), 8.0 - 6.0 * a.cos());
                    format!(
                        r#"<path d="M8 2A6 6 0 {} 1 {x:.3} {y:.3}" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>"#,
                        u8::from(f > 0.5)
                    )
                };
                Icon { name: "ring-arc", view: 16.0, body: Box::leak(body.into_boxed_str()) }
            })
            .collect()
    });
    let k = (frac * 32.0).round().clamp(0.0, 32.0) as usize;
    el().key("ring")
        .size(16.0, 16.0)
        .child(icon(&TRACK, 16.0).color(p.layer3).absolute(0.0, 0.0))
        .child(icon(&arcs[k], 16.0).color(p.icon).absolute(0.0, 0.0))
}

/// One timeline row's content, before it becomes a growth-stack child.
struct RowSpec {
    key: String,
    content: RowContent,
    streaming: bool,
}

enum RowContent {
    User { message: usize, tight: bool, above: bool, below: bool },
    /// An undelivered steer: the bubble it becomes once delivered, muted and marked Pending,
    /// under the same row key so delivery keeps it in place.
    Pending { id: String, tight: bool, above: bool, below: bool },
    Note(usize),
    Turn { start: usize, end: usize, block: usize, live: bool, pt: f32 },
    Actions { message: usize, busy: bool },
    /// A prompt's images, and a tool's (by id, in messages `start..end`): built, and so
    /// fetched and decoded, only when the row mounts.
    UserImages { id: String, pt: f32 },
    ToolImages { start: usize, end: usize, tool: String },
    Built(Box<El>),
}

impl RowSpec {
    fn build(&self, s: &App, p: &Pal, t: &Transcript, fade: f32) -> El {
        #[cfg(test)]
        ROW_BUILDS.set(ROW_BUILDS.get() + 1);
        let el = match &self.content {
            RowContent::User { message, tight, above, below } => match &t.messages[*message] {
                Message::User { id, text, .. } => user_bubble(p, id, text, *tight, *above, *below, s.glows.get(id).map_or(0.0, |at| crate::knobs::glow(&s.knobs, s.clock - at)), None),
                _ => unreachable!(),
            },
            RowContent::Pending { id, tight, above, below } => {
                let item = s.inbox_for(&t.session).and_then(|i| i.items.iter().find(|i| &i.id == id));
                match item {
                    Some(item) => {
                        let label = crate::queue::pending_label(item);
                        user_bubble(p, id, &label, *tight, *above, *below, 0.0, Some(&t.session))
                    }
                    None => col(),
                }
            }
            RowContent::Note(i) => match &t.messages[*i] {
                Message::Note { text: body, .. } => row().pt(16.0).child(text(body.clone()).font(UI).color(p.faint)),
                _ => unreachable!(),
            },
            RowContent::Actions { message, busy } => col().w_full().pt(8.0).child(turn_actions(s, p, *busy, &t.session, &t.messages[*message])),
            RowContent::UserImages { id, pt } => {
                let files = model::user_files(id);
                crate::attachments::gallery(s, p, "images", &files, true).unwrap_or_else(col).pt(*pt)
            }
            RowContent::ToolImages { start, end, tool } => {
                let files = t.messages[*start..*end].iter().find_map(|m| match m {
                    Message::Assistant(a) => a.parts.iter().find_map(|part| match part {
                        Part::Tool { id, files, .. } if id == tool => Some(files),
                        _ => None,
                    }),
                    _ => None,
                });
                files.and_then(|f| crate::attachments::gallery(s, p, "images", f, false)).unwrap_or_else(col).pt(8.0)
            }
            RowContent::Built(el) => (**el).clone(),
            RowContent::Turn { start, end, block, live, pt } => {
                let steps: Vec<&Assistant> = t.messages[*start..*end].iter().filter_map(|m| match m {
                    Message::Assistant(a) => Some(a), _ => None,
                }).collect();
                let blocks = turns::fold(&steps, *live);
                let busy = *live && *block + 1 == blocks.len();
                match &blocks[*block] {
                    Block::Group { key, steps } => turns::group(s, p, key, steps, busy, fade).pt(*pt),
                    Block::Text { body, live, .. } => col().w_full().pt(*pt).child(markdown_view(p, body, fade, *live)),
                    Block::Agent { agent, title, status, session, background, .. } =>
                        crate::agents_view::card(s, p, &t.session, agent, title, status, *session, *background).pt(*pt),
                    Block::Tool(Step::Tool { id, name, title, status, .. }) =>
                        row().pt(*pt).child(turns::tool_row(p, id, name, title, status)),
                    Block::Tool(Step::Thought { .. }) => unreachable!(),
                }
            }
        };
        let row = col().w_full().max_w(CONTENT_W).streaming(self.streaming).child(el);
        // Finished content opens in one quick glide, which a pinned timeline follows.
        if self.streaming { row } else { row.arrives_whole() }
    }
}

#[cfg(test)]
thread_local! {
    static ROW_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(super) static EAGER_ROWS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn timeline(s: &App, p: &Pal, id: &str, t: Option<&Transcript>) -> El {
    let e = edge::ALL[s.edge];
    let specs = std::rc::Rc::new(t.map(|t| rows(s, p, t)).unwrap_or_default());
    let mut stack = col()
        .key("timeline")
        .w_full()
        .center()
        .px(24.0)
        .pt(8.0)
        .pb(24.0)
        .growth(GrowthSpec { spring: crate::knobs::growth(&s.knobs), edge: e });
    let vis = specs.clone();
    stack = stack.on_visible_rows(move |s: &mut App, r| crate::agents::style(s) == crate::agents::Style::Inline && s.agents.seen(vis.iter().map(|x| x.key.as_str()), r));
    #[cfg(test)]
    let eager = EAGER_ROWS.get();
    #[cfg(not(test))]
    let eager = false;
    // Eager construction is the benchmark and regression-test reference.
    if eager && let Some(t) = t {
        stack = stack.virtualize(80.0)
            .children(specs.iter().map(|r| r.build(s, p, t, e.word_fade).key(r.key.clone())));
    } else if t.is_some() {
        let keys = specs.clone();
        let id = id.to_owned();
        let p = *p;
        stack = stack.virtual_rows(specs.len(), 80.0,
            move |_: &App, i| keys[i].key.clone().into(),
            move |s: &App, i| specs[i].build(s, &p, &s.transcripts[&id], e.word_fade));
    }
    if t.is_none() {
        stack = stack.child(
            row()
                .key("loading")
                .w_full()
                .max_w(CONTENT_W)
                .h(40.0)
                .gap(8.0)
                .center()
                .child(activity_ring(p))
                .child(text(s.opening.as_ref().filter(|o| o.id == id).and_then(|o| o.error.as_deref()).unwrap_or("Loading messages…")).font(UI).color(p.faint)),
        );
    }
    col()
        .key(format!("scroll-{id}"))
        .grow(1.0)
        .shrink()
        .w_full()
        .scroll(ScrollSpec { stick_bottom: true })
        .clip_fade(0.0, 16.0, 0.0, 8.0)
        .child(stack)
}

fn rows(s: &App, p: &Pal, t: &Transcript) -> Vec<RowSpec> {
    let mut out = Vec::new();
    let busy = t.busy();
    // A staged revert hides its boundary message and all after it, and the steers waiting behind them.
    let revert = s.session(&t.session).and_then(|i| i.revert.as_deref());
    let shown = t.visible_len(revert);
    let last_assistant = t.messages[..shown].iter().rposition(|m| matches!(m, Message::Assistant(_)));
    let pending: Vec<&InboxItem> = if revert.is_some() { Vec::new() } else { s.inbox_for(&t.session).map(|i| i.steering().collect()).unwrap_or_default() };
    // A turn's consecutive steps fold together (see `turns`); prompts and notes end them.
    let mut steps: Vec<&Assistant> = Vec::new();
    let mut start = 0;
    for (mi, m) in t.messages[..shown].iter().enumerate() {
        match m {
            Message::Assistant(a) => {
                if steps.is_empty() { start = mi; }
                steps.push(a);
            }
            Message::User { id, text: body, .. } => {
                turn_rows(s, p, &mut out, &std::mem::take(&mut steps), start, mi, false);
                let above = mi > 0 && joins(&t.messages[mi - 1], m);
                // The last prompt runs on into the pending steers under it.
                let below = match t.messages[..shown].get(mi + 1) {
                    Some(n) => joins(m, n),
                    None => pending.first().is_some_and(|n| prompts_join(body, n)),
                };
                let tight = prompt_images(&mut out, id);
                if !body.trim().is_empty() {
                    out.push(RowSpec { key: format!("u-{id}"), content: RowContent::User { message: mi, tight, above, below }, streaming: false });
                }
            }
            Message::Note { id, .. } => {
                turn_rows(s, p, &mut out, &std::mem::take(&mut steps), start, mi, false);
                out.push(RowSpec { key: format!("n-{id}"), content: RowContent::Note(mi), streaming: false });
            }
        }
    }
    turn_rows(s, p, &mut out, &steps, start, shown, busy && revert.is_none());
    // Undelivered steers wait below the running work and above Working, as Desktop shows them,
    // in the row each becomes when the server delivers it to the end of the transcript.
    let mut prev = match t.messages[..shown].last() {
        Some(Message::User { text, .. }) if steps.is_empty() => Some(text.as_str()),
        _ => None,
    };
    for (k, item) in pending.iter().enumerate() {
        let above = prev.is_some_and(|x| prompts_join(x, item));
        let below = pending.get(k + 1).is_some_and(|n| prompts_join(&item.text, n));
        let tight = prompt_images(&mut out, &item.id);
        out.push(RowSpec { key: format!("u-{}", item.id), content: RowContent::Pending { id: item.id.clone(), tight, above, below }, streaming: false });
        prev = Some(&item.text);
    }
    if let Some(last) = last_assistant {
        // Working lasts the whole run, like Desktop's session status: each step's message
        // finishes before the next one starts, and those gaps used to blink it out.
        let busy = busy || s.running.contains(&t.session);
        out.push(RowSpec { key: "turn-actions".into(), content: RowContent::Actions { message: last, busy }, streaming: false });
    }
    out
}

/// A prompt's own images, in a row 8 above its bubble. Returns whether its bubble sits tight
/// under what's above it.
fn prompt_images(out: &mut Vec<RowSpec>, id: &str) -> bool {
    let gallery = crate::attachments::shows(&model::user_files(id));
    let tight = out.is_empty() || gallery;
    if gallery {
        let pt = if out.is_empty() { 8.0 } else { 16.0 };
        out.push(RowSpec { key: format!("ui-{id}"), content: RowContent::UserImages { id: id.to_owned(), pt }, streaming: false });
    }
    tight
}

/// Space above a turn's row, as Desktop spaces assistant parts: none under the prompt, 8 between
/// two tools, 16 before anything else.
fn part_gap(prev: Option<bool>, tool: bool) -> f32 {
    match prev {
        None => 0.0,
        Some(true) if tool => 8.0,
        Some(_) => 16.0,
    }
}

/// `live`: these steps end the transcript and the last one is still running.
fn turn_rows(s: &App, p: &Pal, out: &mut Vec<RowSpec>, steps: &[&Assistant], start: usize, end: usize, live: bool) {
    let blocks = turns::fold(steps, live);
    let n = blocks.len();
    // Whether the row above was a tool; `None` above the turn's first row.
    let mut prev: Option<bool> = None;
    for (i, b) in blocks.into_iter().enumerate() {
        let tool = !matches!(b, Block::Text { .. });
        let pt = part_gap(prev, tool);
        let content = RowContent::Turn { start, end, block: i, live, pt };
        out.push(match b {
            Block::Group { key, steps } => {
                let busy = live && i + 1 == n;
                out.push(RowSpec { streaming: busy && s.expanded.contains(&key), content, key });
                // An image a tool returned shows even while its group is closed.
                for st in &steps {
                    if let Step::Tool { id, files, .. } = st
                        && crate::attachments::shows(files)
                    {
                        let content = RowContent::ToolImages { start, end, tool: id.to_string() };
                        out.push(RowSpec { key: format!("ti-{id}"), content, streaming: false });
                    }
                }
                prev = Some(true);
                continue;
            }
            Block::Text { key, live, .. } => RowSpec { content, key, streaming: live },
            Block::Agent { id, .. } => RowSpec {
                key: format!("a-{id}"),
                content,
                streaming: false,
            },
            Block::Tool(Step::Tool { id, .. }) => {
                RowSpec { key: format!("q-{id}"), content, streaming: false }
            }
            Block::Tool(Step::Thought { .. }) => continue,
        });
        prev = Some(tool);
    }
    // Like Desktop, only the turn's last step reports its error, 24 below the work above it.
    if let Some(a) = steps.last()
        && let Some(err) = &a.error
    {
        let pt = if prev.is_some() { 24.0 } else { 0.0 };
        out.push(RowSpec { key: format!("e-{}", a.id), content: RowContent::Built(Box::new(row().pt(pt).child(text(err.clone()).font(UI).color(p.danger).wrap().w_full()))), streaming: false });
    }
}

/// 64ch of the bubble's 14 px Inter: Desktop caps a bubble at `min(82%, 64ch)`.
const BUBBLE_MAX: f32 = 565.0;

/// Whether prompt `b` joins the run of prompt `a` just above it: both have words and `b` brings
/// no files (Desktop's `user-runs.ts`).
fn joins(a: &Message, b: &Message) -> bool {
    match (a, b) {
        (Message::User { text: x, .. }, Message::User { id, text: y }) => {
            !x.trim().is_empty() && !y.trim().is_empty() && model::user_files(id).is_empty()
        }
        _ => false,
    }
}

/// Whether pending steer `b` joins the run of the prompt (or steer) with text `a` above it.
fn prompts_join(a: &str, b: &InboxItem) -> bool {
    !a.trim().is_empty() && !b.text.trim().is_empty() && b.files.is_empty() && model::user_files(&b.id).is_empty()
}

/// A prompt's bubble. A run of prompts sits 4 apart with the trailing corners tightened where
/// they meet (`above`, `below`); the run's last prompt has a copy line under it, and the others
/// show copy beside the bubble, all only while hovered.
///
/// `pending` (the session) draws an undelivered steer: muted, with Move to queue and Delete in
/// copy's place, and the run's last one marked Pending under its bubble at all times.
fn user_bubble(p: &Pal, id: &str, body: &str, tight: bool, above: bool, below: bool, glow: f32, pending: Option<&str>) -> El {
    let words = body.to_owned();
    let copy = match pending {
        Some(session) => crate::queue::pending_actions(p, session, id, below),
        None => row()
            .key("copy")
            .role("button")
            .label("Copy message")
            .size(20.0, 20.0)
            .center()
            .radius(4.0)
            .hover(p.hover)
            .press(p.pressed)
            .group_opacity(0.0, 1.0)
            .on_press_tx(move |_: &mut App, tx: &mut Tx| tx.copy(words.clone()))
            .child(icon(&ic::OUTLINE_COPY, 14.0).color(p.icon_muted)),
    };
    let (top, end) = (if above { 4.0 } else { 10.0 }, if below { 4.0 } else { 10.0 });
    let mut bubble = col()
        .key("bubble")
        .max_w(BUBBLE_MAX)
        .shrink()
        .px(12.0)
        .py(8.0)
        .corners([10.0, top, end, 10.0])
        .fill(if pending.is_some() { p.layer1 } else { p.layer2 });
    if glow > 0.0 {
        // A soft inner glow and a slight lift, under the text.
        let (rim, lift) = if p.dark { (p.accent.mix(Color::WHITE, 0.45), p.text) } else { (p.accent, Color::WHITE) };
        bubble = bubble.child(
            el().key("glow").absolute(0.0, 0.0).w_full().h_full().corners([10.0, top, end, 10.0])
                .fill(lift.alpha(0.1))
                .box_shadow(Shadow { blur: 14.0, color: rim.alpha(0.55), inset: true, ..Shadow::default() })
                .opacity(glow)
                .opacity_motion(INSTANT),
        );
    }
    let ink = if pending.is_some() { p.muted } else { p.text };
    bubble = bubble.child(text(body.to_owned()).key(format!("text-{id}")).font(BUBBLE).color(ink).wrap());
    let copy = if below {
        let x = if pending.is_some() { -52.0 } else { -28.0 };
        bubble = bubble.child(copy.absolute(x, 8.5));
        None
    } else {
        Some(copy)
    };
    let c = col()
        .w_full()
        .pt(if above { 4.0 } else if tight { 8.0 } else { 16.0 })
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .child(row().w_full().justify_end().child(row().w_frac(0.82).justify_end().child(bubble)));
    c.children(copy.map(|copy| row().key("actions").w_full().h(20.0).mt(4.0).justify_end().child(copy)))
}

pub(crate) fn chevron(open: bool) -> &'static Icon {
    if open { &ic::CHEVRON_DOWN } else { &ic::CHEVRON_RIGHT }
}

pub(crate) fn reasoning(p: &Pal, key: &str, body: &str, secs: Option<i64>, open: bool, fade: f32) -> El {
    let k = key.to_owned();
    let label = match secs {
        Some(s) => format!("Thought for {s}s"),
        None => "Thinking".into(),
    };
    let mut c = col().w_full().child(
        row()
            .key("trigger")
            .role("button")
            .h(28.0)
            .gap(6.0)
            .center_y()
            // "Thinking" crossfades to "Thought for Ns" while the chevron slides over.
            .animate_layout()
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                // Opening and closing is instant; only the streaming label change animates.
                tx.instant();
                if !s.expanded.remove(&k) {
                    s.expanded.insert(k.clone());
                }
            })
            .child(icon(&ic::BRAIN, 16.0).color(p.icon_muted))
            .child(text(label).key(if secs.is_some() { "thought" } else { "thinking" }).font(UI).color(p.muted))
            .child(icon(chevron(open), 14.0).color(p.icon_faint)),
    );
    if open {
        c = c.child(
            row().key("thought").pl(22.0).pt(4.0).child(text(body.to_owned()).font(UI.lh(20.0)).color(p.muted).wrap().reveal(fade).w_full()),
        );
    }
    c
}

fn turn_actions(s: &App, p: &Pal, busy: bool, session: &str, message: &Message) -> El {
    // These share a footprint and swap at once: a crossfade would lay the copy icon's outline
    // over the W in Working.
    let has_text = matches!(message, Message::Assistant(a) if a.parts.iter().any(|part| matches!(part, Part::Text { text } if !text.trim().is_empty())));
    let mut button = row().key("copy").role("button").label("Copy response")
        .size(28.0, 28.0).pb(12.0).center().radius(4.0).hover(p.hover).press(p.pressed)
        .opacity(if busy || !has_text { 0.0 } else { 1.0 })
        .opacity_motion(INSTANT)
        .child(icon(&ic::OUTLINE_COPY, 16.0).color(p.muted));
    if !busy && has_text {
        let session = session.to_owned();
        let id = message.id().to_owned();
        button = button.on_press_tx(move |s: &mut App, tx: &mut Tx| {
            if let Some(Message::Assistant(a)) = s.transcripts.get(&session).and_then(|t| t.messages.iter().find(|m| m.id() == id)) {
                tx.copy(a.parts.iter().filter_map(|part| match part {
                    Part::Text { text } => Some(text.as_str()),
                    _ => None,
                }).collect::<Vec<_>>().join("\n\n"));
            }
        });
    }
    // One reserved tail: completion crossfades in place rather than first removing Working's
    // height and then adding actions. The scroll endpoint cannot reverse during this handoff.
    el().w_full().h(28.0).motion(crate::knobs::actions_tail(&s.knobs))
        .child(crate::working::row(s, p, busy).key("working").absolute(0.0, 0.0).opacity(if busy { 1.0 } else { 0.0 }).opacity_motion(INSTANT))
        .child(button.absolute(0.0, 0.0))
}

// ---------------------------------------------------------------------------------------------
// Markdown

fn markdown_view(p: &Pal, src: &str, fade: f32, streaming: bool) -> El {
    let kit = Kit::neutral(p.dark);
    let style = MarkdownStyle::from_kit(&kit)
        .text(p.text)
        .muted(p.muted)
        .link(p.accent)
        .inline_code_bg(p.inline_code_bg)
        .code_block(p.layer1, p.border_muted)
        .quote_rule(p.layer3)
        .rule(p.border)
        .body_font(MD)
        .heading_font(1, H1)
        .heading_font(2, H2)
        .heading_font(3, H3)
        .code_block_font(CODE);
    Markdown::new(src).style(style).reveal(fade).streaming(streaming).on_link(|s: &mut App, url: &str| s.open_link(url)).build(&kit)
}

// ---------------------------------------------------------------------------------------------
// Requests: permission asks and questions waiting on you, docked above the composer.

fn small_button(p: &Pal, key: String, label: &str, primary: bool) -> El {
    row()
        .key(key)
        .role("button")
        .label(label.to_owned())
        .h(28.0)
        .px(10.0)
        .center_y()
        .radius(6.0)
        .fill(if primary { p.contrast } else { p.layer2 })
        .hover(p.hover)
        .press(p.pressed)
        .child(text(label.to_owned()).font(UI_MED).color(p.text))
}

fn requests(s: &App, p: &Pal, id: &str) -> El {
    let mut dock = col().key("requests").w_full().center().px(24.0).gap(8.0);
    // Questions take the composer's place (`question.rs`).
    for r in s.pending_for(id).into_iter().filter(|r| matches!(r, Pending::Permission { .. })) {
        dock = dock.child(request_card(p, r));
    }
    dock
}

fn request_card(p: &Pal, r: &Pending) -> El {
    let rid = r.id().to_owned();
    let card = col()
        .key(format!("request-{rid}"))
        .w_full()
        .max_w(CONTENT_W)
        .padding(14.0)
        .gap(10.0)
        .radius(12.0)
        .fill(if p.dark { p.layer1 } else { p.bg_base })
        .border(0.5, p.accent);
    match r {
        Pending::Permission { action, resources, message, .. } => {
            let mut c = card.child(text(format!("Allow {action}?")).font(UI_MED).color(p.text));
            if !message.is_empty() {
                c = c.child(text(message.clone()).font(UI).color(p.muted).wrap());
            }
            for (i, res) in resources.iter().take(6).enumerate() {
                c = c.child(text(res.clone()).key(format!("res-{i}")).font(CODE).color(p.muted).truncate());
            }
            let b = |key: &str, label: &str, decision: &'static str, primary: bool| {
                let rid = rid.clone();
                small_button(p, format!("{key}-{rid}"), label, primary).on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    s.reply_permission(&rid, decision);
                })
            };
            c.child(
                row()
                    .key("actions")
                    .gap(6.0)
                    .child(b("once", "Allow once", "once", true))
                    .child(b("always", "Always allow", "always", false))
                    .child(b("reject", "Reject", "reject", false)),
            )
        }
        Pending::Form { .. } => card,
    }
}

// ---------------------------------------------------------------------------------------------
// Composer and status bar

fn ghost(p: &Pal, key: &str, label: &str, chevron: bool, color: Color) -> El {
    let mut r = row()
        .key(key)
        .role("button")
        .h(28.0)
        .px(8.0)
        .gap(2.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .child(text(label.to_owned()).font(UI).color(color));
    if chevron {
        r = r.child(icon(&ic::CHEVRON_DOWN, 16.0).color(p.icon_faint));
    }
    r
}

fn composer_dock(s: &App, p: &Pal) -> El {
    let queue = match &s.route {
        Route::Session(id) => Some(crate::queue::panel(s, p, id, s.busy(id))),
        _ => None,
    };
    col().key("dock").w_full().center().px(24.0).pb(12.0).children(queue).child(composer(s, p))
}

pub fn composer_fill(p: &Pal) -> Color {
    if p.dark { p.layer1 } else { p.bg_base }
}

fn composer(s: &App, p: &Pal) -> El {
    let selected = s.selection();
    let model = selected.model;
    let effort = if model.variant.is_empty() { "Default".into() } else { model::capitalize(&model.variant) };
    let ready = !s.composer.text.trim().is_empty() || (s.attach.any() && !s.add.shell);
    let busy = matches!(&s.route, Route::Session(id) if s.busy(id));
    let stop = busy && !ready;
    let submit = el()
        .key("submit")
        .role("button")
        .label("Send")
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .fill(if ready { p.contrast } else { p.layer2 })
        .hover(p.hover)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| if stop { s.stop() } else { s.send(tx) })
        .child(if stop {
            el().key("stop-glyph").size(10.0, 10.0).radius(2.0).fill(p.text)
        } else {
            icon(&ic::ARROW_UP, 16.0).color(if ready { p.text } else { p.icon_faint })
        });
    let submit = if stop { submit.label("Stop").fill(p.contrast) } else { submit };
    let composer = col()
        .key("composer")
        .w_full()
        .max_w(CONTENT_W)
        .min_h(96.0)
        .corners(if crate::queue::docked(s) { [0.0, 0.0, 12.0, 12.0] } else { [12.0; 4] })
        .fill(composer_fill(p))
        .border(0.5, p.ring)
        .shadow(1.0, 2.0, p.shadow)
        // The question dock's face springs in and out under this edge.
        .clip_soft(0.0);
    match crate::question::face(s, p) {
        Some(face) => composer.children(face),
        None => composer.children(crate::attachments::chips(s, p))
        .child(
            col().key("editor").transition(crate::question::face_transition(s)).w_full().px(16.0).pt(16.0).pb(8.0).child(
                text_input(&s.composer, if s.add.shell { SHELL_PROMPT } else { PROMPT })
                    .key("prompt")
                    .w_full()
                    .min_h(40.0)
                    .animate_edit_layout()
                    .placeholder(if s.add.shell { "Enter shell command…" } else { "Ask anything, / for commands, @ for context…" }, p.faint)
                    .caret(p.text, p.selection)
                    .on_edit(|s: &mut App| &mut s.composer)
                    .pill_style(crate::mentions::pill_style(p))
                    .on_input_change(|s: &mut App, tx| s.mentions_changed(tx))
                    .on_input_key(|s: &mut App, ev, tx| {
                        if s.mention_key(ev, tx) || s.add_key(ev, tx) {
                            return true;
                        }
                        if ev.key == Key::Enter && ev.mods.cmd {
                            s.submit(true, tx);
                            return true;
                        }
                        s.shortcut(ev, tx) || s.picker_key(ev, tx)
                    })
                    .on_input_paste(crate::attachments::paste)
                    .on_submit(|s: &mut App, tx: &mut Tx| s.send(tx)),
            ),
        )
        .child(
            row()
                .key("toolbar")
                .transition(crate::question::face_transition(s))
                .h(44.0)
                .px(8.0)
                .gap(4.0)
                .center_y()
                .child(crate::add_menu::trigger(s, p))
                .children(s.settings.show_agent.then(|| picker_trigger(s, p, Kind::Agent, &model::capitalize(&selected.agent), p.muted)))
                .child(picker_trigger(s, p, Kind::Model, &s.pickers.models.iter()
                    .find(|m| m.id == model.id && m.provider == model.provider).map_or_else(|| model.display(), |m| m.name.clone()), p.muted))
                // "Default" is the absence of a choice, so it reads fainter than a picked effort.
                .children(s.has_effort().then(|| {
                    picker_trigger(s, p, Kind::Effort, &effort, if model.variant.is_empty() { p.faint } else { p.muted })
                }))
                .child(spacer())
                .children((busy && ready).then(|| crate::queue::alternate_hint(p)))
                .child(submit),
        ),
    }
}

fn picker_trigger(s: &App, p: &Pal, kind: Kind, label: &str, color: Color) -> El {
    // A new label resizes it and moves its neighbours in the choice, with no glide.
    let mut button = ghost(p, kind.key(), label, true, color).label(format!("Choose {}", kind.key()))
        .motion(no_motion().enter);
    if !s.composer.focused && s.pickers.toolbar_focus == Some(kind) {
        button = button.fill(p.hover);
    }
    if !s.live() {
        button = button.on_press_tx(move |s: &mut App, tx: &mut Tx| { s.open_picker(kind); if s.pickers.open.is_some() { tx.instant(); } });
    }
    button
}

/// Overrides Rune's default 0.2 s fade: menus and palettes appear and go on the keypress or
/// click. (`tx.instant()` would also skip it, but an instant unmount of a focused field drops
/// the composer's restored focus.)
pub fn no_motion() -> rune::SpringPair {
    rune::SpringPair { enter: rune::Spring::new(0.001, 0.0), exit: rune::Spring::new(0.001, 0.0) }
}

fn picker(s: &App, p: &Pal) -> El {
    let kind = s.pickers.open.unwrap();
    let active = s.picker_active();
    let selected = s.picker_current(kind);
    let items = s.picker_items();
    let mut menu = col().key(format!("picker-{}", kind.key())).role("menu").label(format!("Choose {}", kind.key()))
        .anchored(kind.key(), if kind == Kind::Project { Side::Bottom } else { Side::Top }, Align::Start, 6.0)
        .w(match kind { Kind::Model => 284.0, Kind::Project => 240.0, _ => 180.0 })
        .padding(2.0).radius(6.0).fill(p.layer1).border(0.5, p.border_muted)
        .shadow(4.0, 12.0, p.shadow).transition(rune::Transition::opacity().springs(no_motion()))
        // Its height and rows follow the filter in the keystroke.
        .motion(no_motion().enter)
        // Empty space in the menu consumes pointer presses instead of dismissing it.
        .on_press(|_: &mut App| {});
    if kind == Kind::Model {
        menu = menu.child(row().key("picker-search-row").h(28.0).pl(10.0).pr(4.0).gap(8.0).center_y()
            .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
            .child(text_input(&s.pickers.search, UI).key("picker-search").grow(1.0).shrink().h(28.0).animate_edit_layout()
                .placeholder("Search models", p.faint).caret(p.text, p.selection)
                .on_edit(|s: &mut App| &mut s.pickers.search)
                .on_input_key(|s: &mut App, ev, tx| s.shortcut(ev, tx) || s.picker_key(ev, tx))))
            .child(el().h(0.5).w_full().fill(p.border_muted));
    }
    // Filtering is instant, like Desktop's list: rows come and go in the keystroke.
    let appear = rune::Transition::opacity().springs(no_motion());
    let mut list = col().key("picker-options").w_full();
    let mut group = "";
    let mut height = 0.0_f32;
    for item in &items {
        if !item.group.is_empty() && item.group != group {
            list = list.child(col().key(format!("group-{}", item.group)).h(24.0).transition(appear.clone())
                .child(text(item.group.clone()).font(UI_SMALL).color(p.faint).h(24.0).px(10.0).pt(4.0)));
            height += 24.0;
            group = &item.group;
        }
        let key = item.key.clone();
        let hover_key = key.clone();
        let mut option = row().key(format!("option-{key}")).role("menuitemradio").label(item.label.clone())
            .h(28.0).w_full().px(10.0).gap(8.0).center_y().radius(4.0).transition(appear.clone())
            .fill(if active.as_deref() == Some(&key) { p.hover } else { Color::TRANSPARENT })
            .hover(p.hover).press(p.pressed)
            // Not `on_hover`: its commit would animate the highlight's fill from row to row.
            .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
                if inside {
                    tx.instant();
                    s.pickers.activate(hover_key.clone());
                }
            })
            .on_press(move |s: &mut App| s.choose_picker(&key))
            .child(text(item.label.clone()).font(UI).color(p.text)).child(spacer());
        if item.key == selected { option = option.child(icon(&ic::CHECK_SMALL, 16.0).color(p.icon)); }
        list = list.child(option);
        height += 28.0;
    }
    if kind == Kind::Model && items.is_empty() {
        list = list.child(col().key("picker-empty").h(48.0).transition(appear)
            .child(text("No models found").font(UI).color(p.faint).h(48.0).px(10.0).pt(16.0)));
        height = 48.0;
    }
    menu.child(col().key("picker-scroll").w_full().h(height.min(220.0))
        .scroll(ScrollSpec { stick_bottom: false }).clip_fade(0.0, 8.0, 0.0, 8.0).child(list))
}

fn status_bar(s: &App, p: &Pal, info: Option<&SessionInfo>) -> El {
    let dir = info.map_or(String::new(), |i| i.directory.replace("/Users/demo", "~").replace(&home_dir(), "~"));
    let e = edge::ALL[s.edge];
    let live = s.live();
    row()
        .key("status")
        .h(28.0)
        .px(16.0)
        .gap(12.0)
        .center_y()
        .child(icon(&ic::FOLDER, 14.0).color(p.icon_faint))
        .child(text(dir).font(UI_SMALL).color(p.faint).truncate())
        .child(spacer())
        .child(
            row()
                .key("edge")
                .role("button")
                .label("Streaming edge")
                .h(22.0)
                .px(6.0)
                .center_y()
                .radius(4.0)
                .hover(p.hover)
                .on_press(|s: &mut App| s.edge = (s.edge + 1) % edge::ALL.len())
                .child(text(format!("Edge: {}", e.name)).font(UI_SMALL).color(p.faint)),
        )
        .child(el().size(6.0, 6.0).radius(3.0).fill(if live { p.success } else { p.icon_faint }))
        .child(text(s.status.clone()).font(UI_SMALL).color(p.faint))
}

fn home_dir() -> String {
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| std::env::var("HOME").unwrap_or_else(|_| "\u{0}".into())).clone()
}

#[cfg(test)]
mod tests {
    use opencode_kit::client::Incoming;
    use opencode_kit::model::Message;
    use rune::{Env, Key, KeyEvent, Runtime, TextEdit};

    use crate::App;

    #[test]
    fn lazy_timeline_keeps_visible_rows_and_builds_only_the_window() {
        fn scene(s: &App, _: &mut rune::Cx<App>) -> rune::El {
            let crate::Route::Session(id) = &s.route else { unreachable!() };
            rune::col().w_full().h_full().child(super::timeline(s, super::pal(true), id, s.transcripts.get(id)))
        }
        fn visible(rt: &Runtime<App>) -> Vec<(String, Option<String>, [f32; 4])> {
            rt.elements().into_iter().filter(|e| e.visible[3] > 0.0 && !e.exiting)
                .map(|e| (e.path, e.text, e.rect)).collect()
        }
        let state = || {
            let mut s = crate::perf::fixture();
            let mut other = s.transcripts["bench"].clone();
            other.session = "other".into();
            s.transcripts.insert("other".into(), other);
            s.reduced_motion = true;
            s
        };
        super::EAGER_ROWS.set(true);
        let mut eager = Runtime::new(state(), scene, (800.0, 600.0), Env::new(true, 1.0));
        eager.frame(0.0);
        super::EAGER_ROWS.set(false);
        super::ROW_BUILDS.set(0);
        let mut lazy = Runtime::new(state(), scene, (800.0, 600.0), Env::new(true, 1.0));
        lazy.frame(0.0);
        assert!(super::ROW_BUILDS.get() < 100, "2402 rows must not build at open");
        // Compare at rest: initial content, a streamed word, a group expanded, a new prompt,
        // and a session switch. Each has the same keys, content and visible layout.
        for case in 0..5 {
            for (is_eager, rt) in [(true, &mut eager), (false, &mut lazy)] {
                super::EAGER_ROWS.set(is_eager);
                rt.update(|s, tx| {
                    tx.settle();
                    match case {
                        1 => { s.apply_event("session.text.delta", &serde_json::json!({"sessionID":"bench", "assistantMessageID":"stream", "ordinal":0, "delta":" more text"})); }
                        2 => { s.expanded.insert("c-a0799-0".into()); }
                        3 => { s.apply_event("session.prompted", &serde_json::json!({"sessionID":"bench", "messageID":"new", "text":"Next question"})); }
                        4 => s.route = crate::Route::Session("other".into()),
                        _ => {}
                    }
                });
                for _ in 0..120 { if !rt.frame(1.0 / 120.0) { break; } }
            }
            let (a, b) = (visible(&eager), visible(&lazy));
            assert_eq!(a.len(), b.len(), "case {case}: visible elements");
            for ((ak, at, ar), (bk, bt, br)) in a.iter().zip(&b) {
                assert_eq!((ak, at), (bk, bt), "case {case}: identity and text");
                for (a, b) in ar.iter().zip(br) { assert!((a-b).abs() < 0.1, "case {case}, {ak}: {ar:?} != {br:?}"); }
            }
        }
        super::EAGER_ROWS.set(false);
    }

    #[test]
    fn the_new_session_page_picks_its_project() {
        let mut app = App::empty();
        app.load_fixture(crate::FIXTURE);
        app.draft_dir = "/Users/demo/code/lumen/src".into();
        app.route = crate::Route::Draft;
        let mut rt = Runtime::new(app, super::app, (1280.0, 820.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let tap = |rt: &mut Runtime<App>, key: &str| {
            let e = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).unwrap();
            let (x, y) = (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
            rt.pointer_move(x, y);
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            rt.frame(1.0 / 60.0);
        };
        assert_eq!(rt.state.draft_project().map(|p| p.name.as_str()), Some("lumen"));
        tap(&mut rt, "project");
        assert_eq!(rt.state.pickers.open, Some(crate::pickers::Kind::Project));
        let harbor = rt.state.projects.iter().find(|p| p.name == "harbor").unwrap().id.clone();
        tap(&mut rt, &format!("option-{harbor}"));
        assert_eq!(rt.state.draft_dir, "/Users/demo/code/harbor");
        assert_eq!(rt.state.pickers.open, None);
    }

    #[test]
    fn only_mounted_image_rows_fetch_their_media() {
        use opencode_kit::model::{self, Assistant, Attachment, Message, Part, ToolStatus, Transcript};
        use std::sync::{Arc, Mutex};
        fn scene(s: &App, _: &mut rune::Cx<App>) -> rune::El {
            rune::col().w_full().h_full().child(super::timeline(s, super::pal(true), "lazy-media", s.transcripts.get("lazy-media")))
        }
        let image = |c: char, name: &str| Attachment { mime: "image/png".into(), name: name.into(), media: Some(c.to_string().repeat(64)), ..Default::default() };
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = calls.clone();
        let mut app = App::empty();
        app.attach.set_fetch(Arc::new(move |hash: &str| {
            seen.lock().unwrap().push(hash.to_owned());
            Err("stub".into())
        }));
        // A prompt's image and a read tool's image far above, and a prompt's image at the bottom.
        let mut messages = vec![
            Message::User { id: "msg_lazy_first".into(), text: "look".into() },
            Message::Assistant(Assistant {
                id: "msg_lazy_read".into(),
                completed: Some(1),
                parts: vec![Part::Tool {
                    id: "call_lazy_read".into(),
                    name: "read".into(),
                    status: ToolStatus::Completed,
                    title: "a.png".into(),
                    input: serde_json::Value::Null,
                    files: vec![image('b', "a.png")],
                    metadata: serde_json::Value::Null,
                }],
                ..Default::default()
            }),
        ];
        messages.extend((0..200).map(|i| Message::User { id: format!("msg_lazy_{i:03}"), text: format!("filler {i}") }));
        messages.push(Message::User { id: "msg_lazy_last".into(), text: "and this".into() });
        model::set_user_files("msg_lazy_first", vec![image('a', "first.png")]);
        model::set_user_files("msg_lazy_last", vec![image('c', "last.png")]);
        app.transcripts.insert("lazy-media".into(), Transcript { session: "lazy-media".into(), messages });
        app.route = crate::Route::Session("lazy-media".into());
        app.reduced_motion = true;
        let mut rt = Runtime::new(app, scene, (800.0, 600.0), Env::new(true, 1.0));
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        model::set_user_files("msg_lazy_first", Vec::new());
        model::set_user_files("msg_lazy_last", Vec::new());
        assert!(rt.elements().iter().any(|e| e.path.contains("ui-msg_lazy_last")), "the bottom image row mounts");
        assert!(!rt.elements().iter().any(|e| e.path.contains("ti-call_lazy_read")), "the read tool's row is offscreen");
        for _ in 0..200 {
            if !calls.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // Room for any other fetch to show up.
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(*calls.lock().unwrap(), ["c".repeat(64)], "only the mounted row fetched");
    }

    #[test]
    fn a_link_in_a_message_opens() {
        fn view(_: &App, cx: &mut rune::Cx<App>) -> rune::El {
            let p = super::pal(cx.system_dark());
            rune::col().w_full().child(super::markdown_view(&p, "[openthislink](https://example.com/pr/1)", 0.0, false))
        }
        let mut rt = Runtime::new(App::empty(), view, (600.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let link = rt.elements().into_iter().find(|e| e.text.as_deref().is_some_and(|t| t.contains("openthislink"))).unwrap();
        let (x, y) = (link.rect[0] + 8.0, link.rect[1] + link.rect[3] / 2.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.opened, vec!["https://example.com/pr/1".to_owned()]);
    }

    #[test]
    fn a_tool_joining_the_group_slides_its_chevron() {
        let mut app = App::empty();
        app.load_fixture(crate::FIXTURE);
        crate::open_scene(&mut app);
        app.finish_stream();
        let mut rt = Runtime::new(app, super::app, (1280.0, 820.0), Env::new(true, 1.0));
        rt.poll = Some(crate::poll);
        rt.frame(0.0);
        rt.update(|s, tx| {
            s.composer = TextEdit::new("Can you also check the reconnect jitter?");
            s.send(tx);
        });
        // The reply reads a file, then greps: "Used 1 Read", then "Used 2 Read, Grep".
        let chevron = |rt: &Runtime<App>| rt.elements().into_iter()
            .find(|e| !e.exiting && e.path.contains("c-call_reply_read-0001/") && e.key.as_deref() == Some("chevron"))
            .map(|e| e.rect[0]);
        let mut xs = Vec::new();
        for _ in 0..(4.5 * 120.0) as usize {
            rt.frame(1.0 / 120.0);
            xs.extend(chevron(&rt));
        }
        let (lo, hi) = xs.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)));
        assert!(hi - lo > 4.0, "the header changes width: {lo}..{hi}");
        let step = xs.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        // A spring, not a jump: no frame covers more than a tenth of the way.
        assert!(step < (hi - lo) / 10.0, "the chevron moves at most {step} pt in a frame");
    }

    fn session() -> Runtime<App> {
        let mut app = App::empty();
        app.load_fixture(crate::FIXTURE);
        crate::open_scene(&mut app);
        app.finish_stream();
        let mut rt = Runtime::new(app, super::app, (1280.0, 820.0), Env::new(true, 1.0));
        rt.poll = Some(crate::poll);
        rt.frame(0.0);
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
        }
        rt
    }

    /// A message row's bubble fill (drawn and clipped rects), its text's visible height, and
    /// its glow's opacity, if it has one this frame.
    struct Bubble {
        rect: [f32; 4],
        text: [f32; 4],
        visible_h: f32,
        text_h: f32,
        glow: Option<f32>,
    }

    fn bubble(rt: &Runtime<App>, id: &str) -> Option<Bubble> {
        let els = rt.elements();
        let row = format!("u-{id}");
        let inside = |e: &&rune::ElementInfo| !e.exiting && e.path.split('/').any(|p| p == row);
        let fill = els.iter().filter(inside).find(|e| e.key.as_deref() == Some("bubble"))?;
        let text = els.iter().filter(inside).find(|e| e.key.as_deref() == Some(&format!("text-{id}")))?;
        let glow = els.iter().filter(inside).find(|e| e.key.as_deref() == Some("glow")).map(|e| e.opacity);
        Some(Bubble { rect: fill.rect, text: text.rect, visible_h: fill.visible[3], text_h: text.visible[3], glow })
    }

    fn deliver(rt: &mut Runtime<App>, messages: Vec<Message>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let session = rt.state.transcripts.keys().find(|k| rt.state.route == crate::Route::Session((*k).clone())).unwrap().clone();
        tx.send(Incoming::Messages { session, messages }).unwrap();
        rt.state.inbox = Some(rx);
    }

    #[derive(Clone, Copy, Debug)]
    enum Via {
        Enter,
        Click,
    }

    /// Sends through real input (focus, type, then Enter or a click on Send) and checks every
    /// frame, `dts` giving each frame's length (dropped frames included).
    fn send_and_trace(via: Via, dts: &[f64]) {
        let mut rt = session();
        let body = "Can you also check the reconnect jitter?";
        let tap = |rt: &mut Runtime<App>, key: &str| {
            let (x, y) = rt.find(key).unwrap();
            rt.pointer_move(x, y);
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
        };
        tap(&mut rt, "prompt");
        rt.frame(1.0 / 60.0);
        rt.key(KeyEvent { key: Key::Text(body.into()), mods: Default::default() });
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        let typed = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("prompt")).unwrap().rect;
        match via {
            Via::Enter => rt.key(KeyEvent { key: Key::Enter, mods: Default::default() }),
            Via::Click => tap(&mut rt, "submit"),
        }
        let id = "msg_zzzzsent0001";
        let mut texts = Vec::new();
        let working = |rt: &Runtime<App>| rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("working")).map(|e| e.rect[1]);
        let (mut ys, mut work, mut worst) = (Vec::new(), Vec::new(), 0.0f64);
        let mut t = 0.0;
        for (f, &dt) in dts.iter().enumerate() {
            let start = std::time::Instant::now();
            rt.frame(dt);
            worst = worst.max(start.elapsed().as_secs_f64());
            t += dt;
            if f == dts.len() / 2 {
                // The server's copy of the prompt replaces the local one under the same id.
                deliver(&mut rt, vec![Message::User { id: id.into(), text: body.into() }]);
            }
            assert!(rt.state.composer.text.is_empty(), "{via:?}: the composer clears");
            let b = bubble(&rt, id).unwrap_or_else(|| panic!("{via:?} frame {f}: the message has a bubble"));
            assert!(b.text_h <= 0.0 || b.visible_h > 0.0, "{via:?} frame {f}: text shows without its bubble: {:?} {} {}", b.rect, b.visible_h, b.text_h);
            assert!(b.glow.is_none(), "{via:?} frame {f}: this app's own send doesn't glow");
            let rows: Vec<String> = rt.elements().into_iter().filter(|e| !e.exiting).filter_map(|e| e.key).filter(|k| k.starts_with("u-")).collect();
            assert_eq!(rows.iter().filter(|k| *k == &format!("u-{id}")).count(), 1, "frame {f}: one row, same key");
            assert!(rows.iter().collect::<std::collections::HashSet<_>>().len() == rows.len(), "frame {f}: duplicate row keys {rows:?}");
            ys.push((t, b.rect[1]));
            texts.push(b.text);
            work.extend(working(&rt).map(|y| (t, y)));
        }
        eprintln!("{via:?}: bubble y per frame {:?}", ys.iter().map(|p| (p.1 * 10.0).round() / 10.0).collect::<Vec<_>>());
        eprintln!("{via:?}: slowest frame {:.1} ms", worst * 1000.0);
        // It leaves from the composer: its words start where they were typed, at full size.
        // The first frame is a frame into the flight: a few points at 120 Hz.
        let (first, last) = (texts[0], texts[texts.len() - 1]);
        let near = if dts[0] < 0.01 { 5.0 } else { 15.0 };
        assert!((first[0] - typed[0]).abs() < near && (first[1] - typed[1]).abs() < near,
            "{via:?}: the first frame's words sit on the typed ones: {first:?} vs {typed:?}");
        for (f, r) in texts.iter().enumerate() {
            assert!((r[2] - last[2]).abs() < 0.5 && (r[3] - last[3]).abs() < 0.5, "{via:?} frame {f}: the words keep their size: {r:?} vs {last:?}");
        }
        // Then it flies up over several frames, not in one.
        let land = ys.iter().position(|y| (y.1 - ys[ys.len() - 1].1).abs() < 0.5).unwrap();
        assert!(ys[land].0 > 0.25 && ys[land].0 < 0.7, "{via:?}: lands in {:.2} s: {ys:?}", ys[land].0);
        let rise = ys[0].1 - ys[ys.len() - 1].1;
        assert!(rise > 40.0, "{via:?}: it rises into place: {ys:?}");
        // One smooth flight: never back, and never a tenth of the way in a 1/120 s frame (a
        // snap would be all of it in one frame).
        for w in ys.windows(2) {
            let speed = (w[0].1 - w[1].1) / (w[1].0 - w[0].0) as f32;
            assert!(w[1].1 <= w[0].1 + 0.01 && speed < rise * 12.0, "{via:?}: bubble y {ys:?}");
        }
        assert!(work.len() == ys.len(), "{via:?}: Working shows below it throughout");
        for w in work.windows(2) {
            assert!((w[1].1 - w[0].1).abs() / ((w[1].0 - w[0].0) as f32) < 700.0, "{via:?}: Working moves smoothly: {work:?}");
        }
    }

    #[test]
    fn a_sent_message_rises_whole_from_the_composer_and_keeps_its_key() {
        send_and_trace(Via::Enter, &[1.0 / 120.0; 90]);
        send_and_trace(Via::Click, &[1.0 / 120.0; 90]);
        // A dev build that drops frames: 60 Hz with two 50 ms stalls stays on the same path.
        let mut dts = vec![1.0 / 60.0; 45];
        dts[3] = 0.05;
        dts[10] = 0.05;
        send_and_trace(Via::Enter, &dts);
    }

    /// A run of several steps (thinking and a tool, a new assistant message that answers, then
    /// another tool step): Working stays mounted at full opacity from the run's start to its end,
    /// through the gaps between one step ending and the next starting.
    #[test]
    fn working_stays_up_between_steps_until_the_run_ends() {
        let mut app = App::empty();
        app.load_fixture(crate::FIXTURE);
        crate::open_scene(&mut app);
        let crate::Route::Session(sid) = app.route.clone() else { panic!("no open session") };
        let mut rt = Runtime::new(app, super::app, (1280.0, 820.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let ev = |t: f64, ty: &str, step: &str, more: serde_json::Value| {
            let mut d = serde_json::json!({ "sessionID": sid, "assistantMessageID": step });
            d.as_object_mut().unwrap().extend(more.as_object().cloned().unwrap_or_default());
            (t, ty.to_owned(), d)
        };
        let none = serde_json::Value::Null;
        let tool = |id: &str| serde_json::json!({ "id": id, "name": "read", "input": { "filePath": "src/backoff.ts" } });
        let mut script = vec![
            ev(0.10, "session.execution.started", "msg_zzzz_step1", none.clone()),
            ev(0.10, "session.step.started", "msg_zzzz_step1", none.clone()),
            ev(0.20, "session.reasoning.started", "msg_zzzz_step1", serde_json::json!({ "ordinal": 0 })),
            ev(0.25, "session.reasoning.delta", "msg_zzzz_step1", serde_json::json!({ "ordinal": 0, "delta": "Read it first." })),
            ev(0.40, "session.reasoning.ended", "msg_zzzz_step1", serde_json::json!({ "ordinal": 0 })),
            ev(0.50, "session.tool.input.started", "msg_zzzz_step1", tool("call_zzzz_1")),
            ev(0.55, "session.tool.called", "msg_zzzz_step1", tool("call_zzzz_1")),
            ev(0.80, "session.tool.success", "msg_zzzz_step1", tool("call_zzzz_1")),
            ev(0.85, "session.step.ended", "msg_zzzz_step1", serde_json::json!({ "finish": "tool-calls" })),
            // The next step's message arrives a beat later.
            ev(0.95, "session.step.started", "msg_zzzz_step2", none.clone()),
            ev(1.00, "session.text.started", "msg_zzzz_step2", serde_json::json!({ "ordinal": 0 })),
        ];
        for (k, word) in "The backoff resets on every message, so ".split_inclusive(' ').enumerate() {
            script.push(ev(1.05 + k as f64 * 0.04, "session.text.delta", "msg_zzzz_step2", serde_json::json!({ "ordinal": 0, "delta": word })));
        }
        script.extend([
            ev(1.50, "session.text.ended", "msg_zzzz_step2", serde_json::json!({ "ordinal": 0 })),
            ev(1.55, "session.tool.input.started", "msg_zzzz_step2", tool("call_zzzz_2")),
            ev(1.60, "session.tool.called", "msg_zzzz_step2", tool("call_zzzz_2")),
            ev(1.90, "session.tool.success", "msg_zzzz_step2", tool("call_zzzz_2")),
            ev(1.95, "session.step.ended", "msg_zzzz_step2", serde_json::json!({ "finish": "tool-calls" })),
            ev(2.20, "session.step.started", "msg_zzzz_step3", none.clone()),
            ev(2.25, "session.text.started", "msg_zzzz_step3", serde_json::json!({ "ordinal": 0 })),
            ev(2.30, "session.text.delta", "msg_zzzz_step3", serde_json::json!({ "ordinal": 0, "delta": "Done." })),
            ev(2.40, "session.text.ended", "msg_zzzz_step3", serde_json::json!({ "ordinal": 0, "text": "Done." })),
            ev(2.50, "session.step.ended", "msg_zzzz_step3", serde_json::json!({ "finish": "stop" })),
            ev(2.50, "session.execution.succeeded", "msg_zzzz_step3", none.clone()),
        ]);
        let working = |rt: &Runtime<App>| rt.elements().into_iter().filter(|e| e.key.as_deref() == Some("working")).map(|e| (e.exiting, e.opacity)).collect::<Vec<_>>();
        let (dt, mut t, mut next) = (1.0 / 120.0, 0.0, 0);
        while t < 3.0 {
            while next < script.len() && script[next].0 <= t {
                let (_, ty, d) = script[next].clone();
                rt.update(move |s, _| {
                    s.apply_event(&ty, &d);
                });
                next += 1;
            }
            rt.frame(dt);
            t += dt;
            let w = working(&rt);
            if t > 0.1 && t < 2.5 {
                assert!(matches!(w[..], [(false, o)] if o > 0.999), "{t:.3}s: Working is one mounted row at opacity 1: {w:?}");
                // The shimmer's lit copy sits exactly on the label while its window sweeps.
                let els = rt.elements();
                let label = els.iter().find(|e| e.path.contains("/working/") && e.key.as_deref() == Some("label")).unwrap();
                let lit = els.iter().find(|e| e.path.contains("/lit/") && e.text.is_some()).unwrap();
                let (sheen, lit_box) = (els.iter().find(|e| e.key.as_deref() == Some("sheen")).unwrap(), els.iter().find(|e| e.key.as_deref() == Some("lit")).unwrap());
                assert!((label.rect[2] - crate::working::WORKING_W).abs() < 2.0, "the label is {} wide", label.rect[2]);
                assert!((lit.rect[0] - label.rect[0]).abs() < 0.01 && (lit.rect[1] - label.rect[1]).abs() < 0.01, "{t:.3}s: lit {:?} on label {:?}", lit.rect, label.rect);
                assert_eq!(sheen.loop_phase, lit_box.loop_phase, "{t:.3}s: one sweep");
            }
        }
        assert!(working(&rt).iter().all(|&(_, o)| o < 0.001), "the finished run hides Working");
    }

    #[test]
    fn a_message_from_elsewhere_glows_and_settles_and_a_big_one_glides_in() {
        let mut rt = session();
        let id = "msg_zzzzzelsewhere";
        let body = (0..14).map(|i| format!("Line {i} of a long message typed on another device.")).collect::<Vec<_>>().join("\n");
        deliver(&mut rt, vec![Message::User { id: id.into(), text: body }]);
        let (mut glows, mut bottoms) = (Vec::new(), Vec::new());
        for _ in 0..(1.2 * 120.0) as usize {
            rt.frame(1.0 / 120.0);
            let b = bubble(&rt, id).expect("the arrival shows");
            glows.push(b.glow.unwrap_or(0.0));
            bottoms.push(b.rect[1] + b.rect[3]);
        }
        let peak = glows.iter().cloned().fold(0.0, f32::max);
        assert!(peak > 0.9, "it glows: {glows:?}");
        let top = glows.iter().position(|&g| g == peak).unwrap();
        assert!(top <= 12, "the glow rises within 0.1 s: {glows:?}");
        assert!(glows[top..].windows(2).all(|w| w[1] <= w[0] + 1e-4), "then only fades: {glows:?}");
        assert!(glows[(0.9 * 120.0) as usize + 2..].iter().all(|&g| g == 0.0), "settled by 0.9 s: {glows:?}");

        // The pinned timeline follows the ~300 pt insert in one glide: its bubble rises without
        // stopping or stepping (speed up, then down, once) and lands within half a second.
        let speeds: Vec<f32> = bottoms.windows(2).map(|w| w[0] - w[1]).collect();
        let rise = bottoms[0] - bottoms[bottoms.len() - 1];
        assert!(rise > 200.0, "it rises from under the composer: {bottoms:?}");
        let fast = speeds.iter().cloned().fold(0.0, f32::max);
        let at = speeds.iter().position(|&v| v == fast).unwrap();
        assert!(speeds[..=at].windows(2).all(|w| w[1] >= w[0] - 0.05), "speeds {speeds:?}");
        assert!(speeds[at..].windows(2).all(|w| w[1] <= w[0] + 0.05), "speeds {speeds:?}");
        assert!(speeds.iter().all(|&v| v >= -0.01), "never back down: {speeds:?}");
        let land = bottoms.iter().position(|&y| (y - bottoms[bottoms.len() - 1]).abs() < 0.5).unwrap();
        assert!(land <= 60, "landed in {} s: {bottoms:?}", land as f32 / 120.0);
        assert!(!rt.state.glows.contains_key(id));
    }

    #[test]
    fn a_run_of_prompts_sits_close_with_tight_trailing_corners() {
        use opencode_kit::model::Message;
        let mut app = App::empty();
        app.load_fixture(crate::FIXTURE);
        crate::open_scene(&mut app);
        let crate::Route::Session(id) = app.route.clone() else { panic!("no open session") };
        let t = app.transcripts.get_mut(&id).unwrap();
        t.messages.truncate(1);
        for (n, text) in ["Also the toast says reconnected twice.", "And keep the cap at 30s."].iter().enumerate() {
            t.messages.push(Message::User { id: format!("run{n}"), text: (*text).into() });
        }
        let mut rt = Runtime::new(app, super::app, (1280.0, 820.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let bubble = |rt: &Runtime<App>, row: &str| rt.elements().into_iter()
            .find(|e| !e.exiting && e.path.contains(&format!("{row}/")) && e.key.as_deref() == Some("bubble"))
            .unwrap_or_else(|| panic!("no bubble in {row}"));
        let first = format!("u-{}", rt.state.transcripts[&id].messages[0].id());
        let (a, b, c) = (bubble(&rt, &first), bubble(&rt, "u-run0"), bubble(&rt, "u-run1"));
        assert!((b.rect[1] - (a.rect[1] + a.rect[3]) - 4.0).abs() < 0.5, "4 apart: {:?} {:?}", a.rect, b.rect);
        assert!((c.rect[1] - (b.rect[1] + b.rect[3]) - 4.0).abs() < 0.5, "4 apart: {:?} {:?}", b.rect, c.rect);
        assert!(a.rect[2] <= super::BUBBLE_MAX + 0.5, "64ch at most: {:?}", a.rect);
        // Only the run's last prompt has a copy line under it.
        let lines = rt.elements().into_iter().filter(|e| !e.exiting && e.path.contains("/u-") && e.key.as_deref() == Some("actions")).count();
        assert_eq!(lines, 1);
    }

    fn sidebar() -> Runtime<App> {
        let mut app = App::empty();
        app.load_fixture(crate::FIXTURE);
        crate::open_scene(&mut app);
        app.finish_stream();
        let mut rt = Runtime::new(app, super::app, (1280.0, 820.0), Env::new(true, 1.0));
        rt.poll = Some(crate::poll);
        rt.frame(0.0);
        for _ in 0..90 {
            rt.frame(1.0 / 60.0);
        }
        rt
    }

    fn tab_key(rt: &Runtime<App>, i: usize) -> String {
        format!("t-{}", rt.state.tabs[i].id)
    }

    /// The top of a tab's painted pill (its hit area reaches into the gaps).
    fn tab_y(rt: &Runtime<App>, key: &str) -> f32 {
        pill(rt, key).rect[1]
    }

    fn pill(rt: &Runtime<App>, key: &str) -> rune::ElementInfo {
        rt.elements().into_iter().find(|e| !e.exiting && e.path.contains(&format!("{key}/")) && e.key.as_deref() == Some("pill")).unwrap_or_else(|| panic!("no {key}"))
    }

    #[test]
    fn the_gap_between_tabs_belongs_to_a_tab() {
        let mut rt = sidebar();
        let (a, b) = (pill(&rt, &tab_key(&rt, 1)), pill(&rt, &tab_key(&rt, 2)));
        assert!(b.rect[1] - (a.rect[1] + a.rect[3]) > 3.5, "the pills keep their gap: {:?} {:?}", a.rect, b.rect);
        // Just under tab 1's pill, in the gap: a click there opens tab 1.
        let (x, y) = (a.rect[0] + a.rect[2] / 2.0, a.rect[1] + a.rect[3] + 1.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..4 {
            rt.frame(1.0 / 60.0);
        }
        let id = rt.state.tabs[1].id.clone();
        assert_eq!(rt.state.route, crate::Route::Session(id));
    }

    fn ids(rt: &Runtime<App>) -> Vec<String> {
        rt.state.tabs.iter().map(|t| t.id.clone()).collect()
    }

    #[test]
    fn dragging_a_tab_moves_it_saves_the_order_and_slides_every_row() {
        let mut rt = sidebar();
        let before = ids(&rt);
        let keys: Vec<String> = (0..before.len()).map(|i| tab_key(&rt, i)).collect();
        let rest: Vec<f32> = keys.iter().map(|k| tab_y(&rt, k)).collect();
        let pitch = super::TAB_PITCH;
        assert!((rest[1] - rest[0] - pitch).abs() < 0.5, "rows sit one pitch apart: {rest:?}");

        // Row 1 dragged two rows down, in steps, then dropped.
        let (x, y) = rt.find(&keys[1]).unwrap();
        crate::tabs::SAVED.with(|s| *s.borrow_mut() = None);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        let mut drawn: Vec<Vec<f32>> = Vec::new();
        let steps = 40;
        for i in 1..=steps {
            let dy = 2.0 * pitch * i as f32 / steps as f32;
            rt.pointer_move(x, y + dy);
            rt.frame(1.0 / 120.0);
            let dragged = tab_y(&rt, &keys[1]);
            if dy >= 4.0 {
                assert!((dragged - (rest[1] + dy)).abs() < 0.01, "the lifted row follows the pointer 1:1: {dragged} vs {}", rest[1] + dy);
            } else {
                assert_eq!(dragged, rest[1], "inside the 4 pt slop it's still a press");
            }
            drawn.push(keys.iter().map(|k| tab_y(&rt, k)).collect());
        }
        assert_eq!(rt.state.tab_drag.dragging(), Some(1));
        for _ in 0..60 {
            rt.frame(1.0 / 120.0);
            drawn.push(keys.iter().map(|k| tab_y(&rt, k)).collect());
        }
        let held: Vec<f32> = keys.iter().map(|k| tab_y(&rt, k)).collect();
        assert!((held[2] - rest[1]).abs() < 0.5 && (held[3] - rest[2]).abs() < 0.5, "the passed rows stepped up: {held:?}");
        assert!((held[4] - rest[4]).abs() < 0.01, "rows past the target stay put");

        // Dropped 10 pt short of the slot it takes.
        rt.pointer_move(x, y + 2.0 * pitch - 10.0);
        rt.frame(1.0 / 120.0);
        drawn.push(keys.iter().map(|k| tab_y(&rt, k)).collect());
        rt.pointer_up(x, y + 2.0 * pitch - 10.0);
        for _ in 0..90 {
            rt.frame(1.0 / 120.0);
            drawn.push(keys.iter().map(|k| tab_y(&rt, k)).collect());
        }
        let mut want = before.clone();
        let moved = want.remove(1);
        want.insert(3, moved);
        assert_eq!(ids(&rt), want, "the drop moves it");
        assert_eq!(crate::tabs::SAVED.with(|s| s.borrow().clone()), Some(want), "and saves the new order");
        assert_eq!(rt.state.route, crate::Route::Session(before[0].clone()), "a drag doesn't open the tab");

        // Every row's drawn top, frame to frame: no jumps, no reversals once it starts toward
        // its place, and each lands where it rests.
        for (r, key) in keys.iter().enumerate() {
            let ys: Vec<f32> = drawn.iter().map(|f| f[r]).collect();
            let biggest = ys.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
            // The dragged row moves with the hand (one 10 pt step); the rest spring.
            let limit = if r == 1 { 10.5 } else { 6.0 };
            assert!(biggest < limit, "{key} jumps {biggest} pt in a frame: {ys:?}");
            let settled = *ys.last().unwrap();
            let slot = tab_y(&rt, key);
            assert!((settled - slot).abs() < 0.01, "{key} lands in its slot");
        }
        // From where it was let go, it settles straight down into its slot.
        let dropped: Vec<f32> = drawn[drawn.len() - 91..].iter().map(|f| f[1]).collect();
        assert!((dropped[0] - (rest[3] - 10.0)).abs() < 0.01, "let go 10 pt short: {}", dropped[0]);
        assert!(dropped[1] > dropped[0] && dropped[1] < dropped[0] + 5.0, "it starts from where it was drawn: {dropped:?}");
        assert!(dropped.windows(2).all(|w| w[1] >= w[0] - 0.01), "one way, no bounce: {dropped:?}");
        assert!((tab_y(&rt, &keys[1]) - rest[3]).abs() < 0.5, "the dropped row ends in row 3's slot");
    }

    #[test]
    fn switching_tabs_lets_you_type() {
        let mut rt = sidebar();
        let before = ids(&rt);
        for i in [2, 1] {
            let (x, y) = rt.find(&tab_key(&rt, i)).unwrap();
            rt.pointer_move(x, y);
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            for _ in 0..30 {
                rt.frame(1.0 / 60.0);
            }
            assert_eq!(rt.state.route, crate::Route::Session(before[i].clone()));
            rt.state.composer.text.clear();
            rt.key(KeyEvent { key: Key::Text("hi".into()), mods: Default::default() });
            rt.frame(1.0 / 60.0);
            assert_eq!(rt.state.composer.text, "hi", "tab {i} takes typing right away");
        }
    }

    #[test]
    fn a_click_without_a_drag_still_opens_the_tab() {
        let mut rt = sidebar();
        let before = ids(&rt);
        let key = tab_key(&rt, 2);
        let (x, y) = rt.find(&key).unwrap();
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_move(x, y + 2.0);
        rt.frame(1.0 / 60.0);
        rt.pointer_up(x, y + 2.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.route, crate::Route::Session(before[2].clone()), "the click opens it");
        assert_eq!(ids(&rt), before, "and moves nothing");
        assert_eq!(rt.state.tab_drag.dragging(), None);
    }

    #[test]
    fn escape_puts_a_dragged_tab_back() {
        let mut rt = sidebar();
        rt.on_key = Some(crate::on_key);
        let before = ids(&rt);
        let key = tab_key(&rt, 0);
        let rest = tab_y(&rt, &key);
        let (x, y) = rt.find(&key).unwrap();
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_move(x, y + 3.0 * super::TAB_PITCH);
        rt.frame(1.0 / 60.0);
        rt.key(rune::KeyEvent { key: rune::Key::Escape, mods: rune::Mods::default() });
        rt.frame(1.0 / 60.0);
        rt.pointer_up(x, y + 3.0 * super::TAB_PITCH);
        for _ in 0..90 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(ids(&rt), before);
        assert!((tab_y(&rt, &key) - rest).abs() < 0.01, "back in its place");
    }
}

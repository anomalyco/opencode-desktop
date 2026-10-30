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
/// Where the zoom button ends with the lights on the header's centre line (`macVerticalTrafficLights`).
pub(super) const TRAFFIC_LIGHTS_END: f32 = 85.0;
/// The collapsed toggle's box: from the shell inset plus header padding (18) to past the lights.
pub(super) const COLLAPSED_SLOT: f32 = TRAFFIC_LIGHTS_END + 16.0 - 18.0 + 32.0;
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

pub(super) fn sidebar(s: &App, p: &Pal) -> El {
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
    let inner = inner.child(
        col()
            .key("actions")
            .w_full()
            .gap(4.0)
            .child(action_row(p, "home", &ic::GRID_PLUS, "Home", "⌘B", s.route == Route::Home).on_press_tx(
                |s: &mut App, tx: &mut Tx| s.go_home(tx),
            ))
            .child(action_row(p, "search", &ic::MAGNIFYING_GLASS, "Search sessions", "", false).on_press_tx(
                |s: &mut App, tx: &mut Tx| {
                    s.search.focused = true;
                    s.go_home(tx);
                },
            ))
            .child(action_row(p, "new", &ic::EDIT, "New session", "⌘T", false).on_press_tx(
                |s: &mut App, tx: &mut Tx| s.new_session(tx),
            ))
            .child(action_row(p, "browser", &ic::GLOBE, "Browser", "", s.browser.open).on_press_tx(
                |s: &mut App, tx: &mut Tx| s.toggle_browser(tx),
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
    );

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
    let project = info.and_then(|i| s.project(&i.project));
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
        .on_hover_change(move |s: &mut App, inside, _: &mut Tx| if inside { s.prefetch_session(&hover_id); })
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
pub(super) fn sidebar_toggle(p: &Pal, open: bool, w: f32) -> El {
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
pub(super) fn collapsed_slot(s: &App) -> El {
    // Beta measures from the header's 10px padding; this header pads 14.
    el().key("toggle-space").w(if s.sidebar_open { 0.0 } else { COLLAPSED_SLOT - 4.0 }).h(28.0).motion(SIDEBAR_SPRING)
}

/// Desktop's activity spinner, "Soft Radar": a 5×5 dot matrix whose sweep head steps clockwise
/// around each square ring, leaving a fading trail (`spinner-radar.ts`), 14px, icon-muted.
pub(crate) fn activity_ring(p: &Pal) -> El {
    activity_ring_sized(p, 14.0)
}

/// The spinner at `size`: Desktop's dot opacities (0.2 at rest, 1 lit), composited in sRGB.
pub(crate) fn activity_ring_sized(p: &Pal, size: f32) -> El {
    let mut r = el().key("ring").size(size, size);
    for (k, d) in ring_dots(size).iter().enumerate() {
        let mut dot = el().key(format!("d{k}")).absolute(d.x, d.y).size(d.size, d.size).radius(d.size * 0.24).fill(p.icon_muted);
        dot = match d.cycle {
            Some(c) => dot.cycle(c),
            None => dot.opacity(d.still),
        };
        r = r.child(dot);
    }
    r
}

pub(super) struct RadarDot {
    x: f32,
    y: f32,
    size: f32,
    still: f32,
    cycle: Option<Cycle>,
}

/// The radar's dots for one size, built once each.
pub(super) fn ring_dots(size: f32) -> &'static [RadarDot] {
    static BUILT: std::sync::Mutex<Vec<(u32, &'static [RadarDot])>> = std::sync::Mutex::new(Vec::new());
    let key = size.to_bits();
    let mut built = BUILT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&(_, dots)) = built.iter().find(|(k, _)| *k == key) {
        return dots;
    }
    let scale = size / 16.0;
    let dots: Vec<RadarDot> = radar()
        .iter()
        .map(|d| {
            let (x, y, side) = (d.x * scale, d.y * scale, d.size * scale);
            let cycle = d.stops.as_ref().map(|(at, stops)| Cycle {
                period: RADAR_LOOP,
                phase: *at,
                stops: Box::leak(stops.clone().into_boxed_slice()),
                still: d.still,
                fps: 30.0,
            });
            RadarDot { x, y, size: side, still: d.still, cycle }
        })
        .collect();
    let dots: &'static [RadarDot] = Box::leak(dots.into_boxed_slice());
    built.push((key, dots));
    dots
}

pub(super) const RADAR_LOOP: f32 = 1.8;

/// One dot in Desktop's 16-unit viewBox, with its opacities as the browser blends them:
/// `stops` is where in the loop it lights and its keyframes; none for the steady centre.
pub(super) struct Dot16 {
    x: f32,
    y: f32,
    size: f32,
    still: f32,
    stops: Option<(f32, Vec<(f32, f32)>)>,
}

pub(super) fn radar() -> &'static [Dot16] {
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

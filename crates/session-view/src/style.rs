//! Shared timeline typography, Markdown styling, Find marks, reasoning disclosure, and the
//! Soft Radar activity spinner.

use std::ops::Range;
use std::sync::{Mutex, OnceLock};

use rune::{Anim, Color, Cycle, El, Icon, TextHighlight, TextStyle, Tx, col, el, icon, row, text};
use rune_kit::{Markdown, MarkdownStyle};
use rune_theme_opencode::icons as ic;

use crate::TimelineHost;
use crate::palette::Pal;

pub const INSTANT: Anim = Anim::INSTANT;

// Type: Inter variable (weights 440/530/670 are real instances) and IBM Plex Mono.
pub const UI: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
pub const UI_MED: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
pub const UI_SMALL: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);
pub const MD: TextStyle = TextStyle::new(14.0, 440.0).lh(22.4);
pub const BUBBLE: TextStyle = TextStyle::new(14.0, 400.0).lh(21.0);
pub const H1: TextStyle = TextStyle::new(17.0, 600.0).lh(24.0);
pub const H2: TextStyle = TextStyle::new(15.0, 600.0).lh(24.0);
pub const H3: TextStyle = TextStyle::new(13.0, 600.0).lh(20.0);
// Reply headings (`markdown.css`): line-height 150%; h4 to h6 share one, in the muted colour.
const MD_H: [TextStyle; 4] = [
    TextStyle::new(17.0, 600.0).lh(25.5),
    TextStyle::new(15.0, 600.0).lh(22.5),
    TextStyle::new(14.0, 670.0).lh(21.0),
    TextStyle::new(14.0, 530.0).lh(21.0),
];
pub const CODE: TextStyle = TextStyle::new(13.0, 400.0).mono().lh(20.0);

/// `--container-session`: 744 including 24 px padding on each side.
pub const CONTENT_W: f32 = 696.0;

/// Desktop's dock: the composer and all that stacks with it sit 12 pt off the chat's sides and
/// bottom, so at full width they stand 12 pt wider than the reply text on each side.
pub const DOCK_SIDE: f32 = 12.0;
pub const DOCK_W: f32 = CONTENT_W + 2.0 * DOCK_SIDE;

/// 64ch of the bubble's 14 px Inter: Desktop caps a bubble at `min(82%, 64ch)`.
pub const BUBBLE_MAX: f32 = 565.0;

pub const ACTIVE_KEY: &str = "find-active";

pub fn chevron(open: bool) -> &'static Icon {
    if open { &ic::CHEVRON_DOWN } else { &ic::CHEVRON_RIGHT }
}

/// Text people can select and copy, as Desktop's content is (`user-select: text`); the chrome
/// around it (triggers, labels, buttons) stays unselectable.
pub fn readable(p: &Pal, t: El) -> El {
    t.selectable(true).selection_color(p.selection)
}

pub fn reasoning<S: TimelineHost>(p: &Pal, key: &str, body: &str, secs: Option<i64>, open: bool, fade: f32) -> El {
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
            // Opening and closing is instant; only the streaming label change animates.
            .on_press_tx(crate::disclosure::toggle::<S>(key.to_owned(), format!("{key}/trigger")))
            .child(icon(&ic::BRAIN, 16.0).color(p.icon_muted))
            .child(text(label).key(if secs.is_some() { "thought" } else { "thinking" }).font(UI).color(p.muted))
            .child(icon(chevron(open), 14.0).color(p.icon_faint)),
    );
    if open {
        c = c.child(
            row().key("thought").pl(22.0).pt(4.0).child(readable(p, text(body.to_owned()).font(UI.lh(20.0)).color(p.muted).wrap().reveal(fade).w_full())),
        );
    }
    c
}

/// Match `i` of a row in plain text, as `text_highlights` paints it.
pub fn text_mark(p: &Pal, r: Range<usize>, i: usize, active: bool) -> TextHighlight {
    if active {
        TextHighlight::new(r, p.accent).color(p.bg_deep).key(ACTIVE_KEY)
    } else {
        TextHighlight::new(r, hit_bg(p)).key(format!("find-hit-{i}"))
    }
}

/// Match `i` of a row in Markdown, as `markdown_highlights` paints it.
pub fn markdown_mark(p: &Pal, i: usize, active: bool) -> rune_kit::Highlight {
    if active {
        rune_kit::Highlight::new().bg(p.accent).fg(p.bg_deep).key(ACTIVE_KEY)
    } else {
        rune_kit::Highlight::new().bg(hit_bg(p)).key(format!("find-hit-{i}"))
    }
}

fn hit_bg(p: &Pal) -> Color {
    p.accent.alpha(0.28)
}

/// The timeline's Markdown look, its body text in `color`, its diagrams at most `width` points
/// wide (the column they sit in: wider ones lay out top-down or draw smaller).
///
/// Every value is Desktop's `markdown.css`: links with no underline until hovered, inline code
/// at the text's size in the medium weight, code blocks on `background-stronger` coloured by
/// the theme's `syntax-*` (shell commands plain on `layer-02`), a hairline quote rule, invisible
/// rules, outside list markers, the browser's disabled checkboxes, and the Console's data table.
pub fn markdown_style(p: &Pal, color: Color, width: f32) -> MarkdownStyle {
    let k = crate::palette::kit(p);
    let [h1, h2, h3, h4] = MD_H;
    MarkdownStyle::from_kit(k)
        .text(color)
        .muted(p.muted)
        .link(p.accent)
        .link_underline(Color::TRANSPARENT)
        .link_hover(p.accent_hover)
        .inline_code_bg(p.inline_code_bg)
        .inline_code_font(1.0, 530.0)
        .inline_code_pad(4.0)
        .code_block(p.code_bg, p.border)
        .code_block_text(p.syntax.variable)
        .shell_code(p.code_bg, p.border_muted, p.text)
        .quote(p.border, 0.5, 8.0, p.muted)
        .rule(Color::TRANSPARENT)
        .body_font(MD)
        .heading_font(1, h1)
        .heading_font(2, h2)
        .heading_font(3, h3)
        .heading_font(4, h4)
        .list(32.0, 8.0)
        .list_markers_outside(true)
        .list_nested_indent(16.0, 28.0)
        .task_checkbox(13.0, 2.0, 12.0, Some(4.0), checkbox(p, true), checkbox_mark(p), checkbox(p, false), checkbox_border(p))
        .table_card(k)
        .table_header_text(p.muted)
        .table_header_weight(530.0)
        .table_column_align(false)
        .code_block_font(CODE)
        .selection(p.selection)
        .diagram_max_width(width)
        // However wide, a diagram draws whole; it opens full size to be read (`diagram`).
        .diagram_min_scale(0.0)
        .syntax_theme(p.syntax.syntax(rune_kit::syntax::SyntaxTheme::from_kit(k), p.diff_add, p.diff_del))
}

// A task item's checkbox is the browser's own, disabled (`<input type="checkbox" disabled>`):
// Chromium's grey fill and white tick when checked, a pale box in a grey outline when not.
fn checkbox(p: &Pal, checked: bool) -> Color {
    let hex = |s: &str| rune::color::hex(s).unwrap_or(p.layer2);
    match (p.dark, checked) {
        (false, true) => hex("#d2d2d2"),
        (false, false) => hex("#f9f9f9"),
        (true, true) => hex("#757575"),
        (true, false) => hex("#3b3b3b"),
    }
}

fn checkbox_mark(p: &Pal) -> Color {
    rune::color::hex(if p.dark { "#3b3b3b" } else { "#ffffff" }).unwrap_or(p.text)
}

fn checkbox_border(p: &Pal) -> Color {
    rune::color::hex(if p.dark { "#626262" } else { "#d1d1d1" }).unwrap_or(p.border)
}

/// `copied` is the copy button showing its tick (`copied::current`), for code blocks' own.
#[allow(clippy::too_many_arguments)]
pub fn markdown_styled<S: TimelineHost, H: rune_kit::markdown::HighlightItem>(
    p: &Pal,
    src: &str,
    fade: f32,
    streaming: bool,
    style: MarkdownStyle,
    highlights: Vec<H>,
    copied: Option<String>,
) -> El {
    // Its texts join the timeline's selection group rather than forming one of their own, so
    // a drag runs on out of this reply into the rest of the timeline.
    let full = std::rc::Rc::new(style.clone());
    let pal = *p;
    let rewritten = S::rewrite_markdown(src);
    Markdown::new(rewritten.as_deref().unwrap_or(src))
        .style(style)
        .highlights(highlights)
        .reveal(fade)
        .streaming(streaming)
        .selectable(true)
        .diagram_frame(move |card, d| crate::diagram::frame::<S>(&pal, card, d, &full))
        .code_frame(move |card, c| code_card::<S>(&pal, card, c, copied.as_deref()))
        .on_link_tx(|s: &mut S, url: &str, tx: &mut Tx| s.open_link(url, tx))
        .link_help(S::reply_link_help)
        .build(crate::palette::kit(p))
        .selection_group(false)
}

/// A code block with Desktop's Copy button (`markdown.css`'s `markdown-copy-button`): a 24 pt
/// ghost icon button 4 pt in from the top right, shown while the pointer is over the block,
/// its glyph a tick for a moment after a copy.
fn code_card<S: TimelineHost>(p: &Pal, card: El, c: &rune_kit::CodeFrame<'_>, copied: Option<&str>) -> El {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (c.index, c.source).hash(&mut h);
    let key = format!("code-copy-{:016x}", h.finish());
    let done = copied == Some(key.as_str());
    let label = if done { "Copied" } else { "Copy" };
    let (code, mark) = (c.source.to_owned(), key);
    let button = row()
        .key("copy")
        .role("button")
        .label(label)
        .help(label)
        .size(24.0, 24.0)
        .center()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            tx.copy(code.clone());
            crate::copied::mark_copied(s, mark.clone(), tx);
        })
        .child(icon(if done { &ic::CHECK } else { &ic::OUTLINE_COPY }, 16.0).color(p.icon_muted));
    col()
        .w_full()
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .child(card)
        .child(button.pin(None, Some(4.0), Some(4.0), None).group_opacity(0.0, 1.0))
}

/// `markdown_view` with its body text in `color`, its diagrams at most `width` points wide.
pub fn markdown_tinted<S: TimelineHost>(p: &Pal, src: &str, fade: f32, streaming: bool, color: Color, width: f32) -> El {
    markdown_with_highlights::<S>(p, src, fade, streaming, color, width, Vec::new(), None)
}

#[allow(clippy::too_many_arguments)]
pub fn markdown_with_highlights<S: TimelineHost>(
    p: &Pal,
    src: &str,
    fade: f32,
    streaming: bool,
    color: Color,
    width: f32,
    highlights: Vec<(&[Range<usize>], rune_kit::Highlight)>,
    copied: Option<String>,
) -> El {
    markdown_styled::<S, _>(p, src, fade, streaming, markdown_style(p, color, width), highlights, copied)
}

/// Desktop's activity spinner, "Soft Radar": a 5×5 dot matrix whose sweep head steps clockwise
/// around each square ring, leaving a fading trail (`spinner-radar.ts`), 14px, icon-muted.
pub fn activity_ring(p: &Pal) -> El {
    activity_ring_sized(p, 14.0)
}

/// The spinner at `size`: Desktop's dot opacities (0.2 at rest, 1 lit), composited in sRGB.
pub fn activity_ring_sized(p: &Pal, size: f32) -> El {
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

struct RadarDot {
    x: f32,
    y: f32,
    size: f32,
    still: f32,
    cycle: Option<Cycle>,
}

const RADAR_LOOP: f32 = 1.8;

fn ring_dots(size: f32) -> &'static [RadarDot] {
    static BUILT: Mutex<Vec<(u32, &'static [RadarDot])>> = Mutex::new(Vec::new());
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
                    return Dot16 { x, y, size, still: DIM + (1.0 - DIM) * trail[0].1, stops: None };
                }
                let at = r3(angle(c));
                let others = cells.iter().filter(|&&o| o != c && o != (0, 0) && ring_of(o) == ring_of(c));
                let behind = others.clone().map(|&o| r3(frac(at - r3(angle(o))))).fold(1.0f32, f32::min) * 100.0;
                let ahead = others.map(|&o| r3(frac(r3(angle(o)) - at))).fold(1.0f32, f32::min) * 100.0;
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

/// A stopped turn's tail: Desktop's quiet Interrupted beside Copy, as its reply footer says
/// it. It rises out of Working's place as Working goes, so the stop reads as the run settling.
pub fn interrupted_label(p: &Pal, beside_copy: bool) -> El {
    use crate::working::{COPY, COPY_AT, H, LANE_PAD};
    use rune::{Spring, SpringPair, Transition};
    // Level with Copy's glyph, 4 pt after its square, as Desktop's footer spaces them.
    let (y, h) = (COPY_AT.1, COPY);
    let rise = LANE_PAD + H / 2.0 - (y + h / 2.0);
    let arrive = Transition::offset(0.0, rise).combined(Transition::opacity());
    row()
        .key("interrupted")
        .absolute(if beside_copy { COPY_AT.0 + COPY + 4.0 } else { 0.0 }, y)
        .h(h)
        .center_y()
        .transition(Transition::asymmetric(arrive, Transition::opacity()).springs(SpringPair::new(Spring::new(0.24, 0.0), Spring::new(0.12, 0.0))))
        .child(text("Interrupted").font(UI_SMALL).color(p.faint))
}

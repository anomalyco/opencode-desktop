//! Shared OpenCode UI typography, layout metrics, pointer/safe-area frame state, Markdown styling,
//! Find marks, disclosure arrows, and the Soft Radar activity spinner.

use std::ops::Range;

use rune::{Anim, Color, Curve, Easing, El, Icon, LocalTime, TextHighlight, TextStyle, col, el, icon};
use rune_kit::MarkdownStyle;
use rune_theme_opencode::icons as ic;

use crate::palette::Pal;

pub const INSTANT: Anim = Anim::INSTANT;
/// Desktop's `transition: opacity 0.15s ease` on a hover-revealed control.
pub const HOVER_FADE: Anim = Anim::new(Curve::tween(0.15, Easing::EASE));

// Type: Inter variable (weights 440/530/670 are real instances) and IBM Plex Mono.
pub const UI: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
pub const UI_MED: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
pub const UI_SMALL: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);
pub const MD: TextStyle = TextStyle::new(14.0, 440.0).lh(22.4);
pub const BUBBLE: TextStyle = TextStyle::new(14.0, 440.0).lh(21.0);
pub const H1: TextStyle = TextStyle::new(17.0, 600.0).lh(24.0);
pub const H2: TextStyle = TextStyle::new(15.0, 600.0).lh(24.0);
pub const H3: TextStyle = TextStyle::new(13.0, 600.0).lh(20.0);
// Reply headings (`markdown.css`): line-height 150%; h4 to h6 share one, in the muted colour.
pub const MD_H: [TextStyle; 4] = [
    TextStyle::new(17.0, 600.0).lh(25.5),
    TextStyle::new(15.0, 600.0).lh(22.5),
    TextStyle::new(14.0, 670.0).lh(21.0),
    TextStyle::new(14.0, 530.0).lh(21.0),
];
pub const CODE: TextStyle = TextStyle::new(13.0, 440.0).mono().lh(20.0);
/// A tool's code (`--v2-font-family-code` at 440, tracking -0.04): a console's text, a shell's
/// command, Execute's calls.
pub const TOOL_CODE: TextStyle = TextStyle::new(13.0, 440.0).mono().lh(20.0).tracking(-0.04);

/// `--container-session`: 744 including 24 px padding on each side.
pub const CONTENT_W: f32 = 696.0;

/// Desktop's dock: the composer and all that stacks with it sit 12 pt off the chat's sides and
/// bottom, so at full width they stand 12 pt wider than the reply text on each side.
pub const DOCK_SIDE: f32 = 12.0;
pub const DOCK_W: f32 = CONTENT_W + 2.0 * DOCK_SIDE;

/// The composer box's corner radius: Desktop's form is `rounded-xl`, 10 px (`editor.tsx:254`).
/// The new-session Easter egg's paddle grows out of the composer from this radius.
pub const COMPOSER_RADIUS: f32 = 10.0;

/// The composer surface's background fill: `layer1` in dark mode, `bg_base` in light mode.
pub fn composer_fill(p: &Pal) -> Color {
    if p.dark { p.layer1 } else { p.bg_base }
}

/// Shared outline quote glyph used by quote buttons and quote cards.
pub static OUTLINE_QUOTE: Icon = Icon {
    name: "outline-quote",
    view: 16.0,
    body: r#"<path d="M9.5 8.49927C10.8333 8.49927 12.1667 8.49927 13.5 8.49927C13.5 9.74927 13.5 11.2493 13.5 12.4993C12.1667 12.4993 10.8333 12.4993 9.5 12.4993V7C9.5 5 10.5 3.5 13 3.5001" stroke="currentColor"/><path d="M2.5 8.49927C3.83333 8.49927 5.16667 8.49927 6.5 8.49927C6.5 9.74927 6.5 11.2493 6.5 12.4993C5.16667 12.4993 3.83333 12.4993 2.5 12.4993V7C2.5 5 3.5 3.5 6 3.5001" stroke="currentColor"/>"#,
};

/// Desktop's `speech-bubble`: what a session mention's chip leads with.
pub static SESSION: Icon = Icon {
    name: "speech-bubble",
    view: 20.0,
    body: r#"<path d="M18.3334 10.0003C18.3334 5.57324 15.0927 2.91699 10.0001 2.91699C4.90749 2.91699 1.66675 5.57324 1.66675 10.0003C1.66675 11.1497 2.45578 13.1016 2.5771 13.3949C2.5878 13.4207 2.59839 13.4444 2.60802 13.4706C2.69194 13.6996 3.04282 14.9364 1.66675 16.7684C3.5186 17.6538 5.48526 16.1982 5.48526 16.1982C6.84592 16.9202 8.46491 17.0837 10.0001 17.0837C15.0927 17.0837 18.3334 14.4274 18.3334 10.0003Z" stroke="currentColor" stroke-linecap="square"/>"#,
};

thread_local! {
    /// The column this frame draws at (Timeline width): `CONTENT_W` unless the host says otherwise.
    static SESSION_W: std::cell::Cell<f32> = const { std::cell::Cell::new(CONTENT_W) };
}

/// Settings › Preferences › Timeline width (`timelineWidth`), as `--container-session` less its
/// 24 px sides: Focused 488, Comfortable 744 (Desktop's default) and Wide 1000.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Width {
    Focused,
    #[default]
    Comfortable,
    Wide,
}

impl Width {
    pub fn content(self) -> f32 {
        match self {
            Width::Focused => 488.0 - 48.0,
            Width::Comfortable => CONTENT_W,
            Width::Wide => 1000.0 - 48.0,
        }
    }
}

/// The timeline's column this frame: the reply text's measure, where its rows stop growing.
pub fn content_w() -> f32 {
    SESSION_W.get()
}

/// The composer and its docks' width this frame, 12 pt wider than the column each side.
pub fn dock_w() -> f32 {
    content_w() + 2.0 * DOCK_SIDE
}

/// Sets the column the frame's session views draw at; the host calls it before building them.
pub fn set_content_w(w: f32) {
    SESSION_W.set(w);
}

thread_local! {
    /// The window is below Desktop's 768 px breakpoint this frame.
    static NARROW: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The pointer can't hover this frame (a touch screen: `Cx::hover_none`).
    static POINTER: std::cell::Cell<Pointer> = const { std::cell::Cell::new(Pointer::Hover) };
}

/// Tells this frame's session views the window is narrow; the host calls it before building them.
pub fn set_narrow(narrow: bool) {
    NARROW.set(narrow);
}

/// Tells this frame's session views the pointer can't hover (`Cx::hover_none`), so what a
/// mouse would reveal on hover shows, as Desktop's `@media (hover: none)` rules do.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pointer { Hover, NoHover, Touch }

pub fn set_pointer(hover_none: bool, touch: bool) {
    POINTER.set(if touch { Pointer::Touch } else if hover_none { Pointer::NoHover } else { Pointer::Hover });
}

/// The pointer can't hover this frame.
pub fn hover_none() -> bool {
    POINTER.get() != Pointer::Hover
}

pub fn touch_controls() -> bool { POINTER.get() == Pointer::Touch }

thread_local! {
    /// A phone's safe area this frame, top/right/bottom/left in points (`Cx::safe_area`).
    static SAFE_AREA: std::cell::Cell<[f32; 4]> = const { std::cell::Cell::new([0.0; 4]) };
}

pub fn set_safe_area(insets: [f32; 4]) { SAFE_AREA.set(insets); }

thread_local! {
    /// The page draws on `CpuCanvas` permanently (no WebGPU, or hand-over/device failed): skip
    /// `backdrop_blur` and draw its flat tint instead.
    static PERMANENT_CPU_CANVAS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn set_permanent_cpu_canvas(on: bool) { PERMANENT_CPU_CANVAS.set(on); }

pub fn permanent_cpu_canvas() -> bool { PERMANENT_CPU_CANVAS.get() }

/// The window keeps its content clear of these and lets its surfaces (the shell's panel, a
/// sheet, the touch bar) run under them to the screen's edge. Zero off a phone.
pub fn safe_area() -> [f32; 4] { SAFE_AREA.get() }

/// A text field (`rune::text_input`), as `base.css`'s touch rule has every field: on a phone it
/// types at 16 (iOS zooms to a smaller one) while its placeholder keeps the design's size.
pub fn field(edit: &rune::TextEdit, style: TextStyle) -> El {
    if !touch_controls() {
        return rune::text_input(edit, style);
    }
    rune::text_input(edit, TextStyle { size: 16.0, ..style }).placeholder_font(style)
}

/// CSS hover-reveal: always visible when the primary pointer cannot hover.
pub fn hover_reveal(e: El) -> El { if hover_none() { e } else { e.group_opacity(0.0, 1.0) } }

/// The window is below Desktop's 768 px breakpoint this frame.
pub fn narrow() -> bool {
    NARROW.get()
}

/// The session header's height (`.session-header-layout`'s `block-size: 48px`).
pub const HEADER_H: f32 = 48.0;

thread_local! {
    /// Where the timeline's stuck headers hold: under the session header stuck over it, else at
    /// the top (`virtualizer.tsx:1173`'s `--sticky-accordion-top`).
    static STICKY_TOP: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) };
}

/// Sets where the rows built next stick their headers; `timeline` sets it before building rows.
pub fn set_sticky_top(top: f32) {
    STICKY_TOP.set(top);
}

/// Where a timeline row's header sticks: under the session header while one shows over the
/// timeline (`--sticky-accordion-top: var(--session-header-height, 48px)`), else at the top.
pub fn sticky_top() -> f32 {
    STICKY_TOP.get()
}

/// The timeline's two side paddings together: `px-6`, `px-4` in a narrow window.
pub fn sides() -> f32 {
    if NARROW.get() { 32.0 } else { 48.0 }
}

/// Room above the timeline's first row, which starts with 8 of its own over the first prompt:
/// the header's 16 under its bar (`pb-4`, `virtualizer.tsx:147`'s 64), or the narrow window's
/// `h-4` where the header hides (`virtualizer.tsx:1175`), less those 8.
pub fn timeline_top() -> f32 {
    8.0
}

/// 64ch of the bubble's 14 px Inter: Desktop caps a bubble at `min(82%, 64ch)`, which Chromium
/// computes as `min(82%, 570.5px)` (64 advances of its `0`).
pub const BUBBLE_MAX: f32 = 570.5;

pub const ACTIVE_KEY: &str = "find-active";

/// What a reply's file-naming inline code links to: this, then the code as written.
pub const CODE_LINK: &str = "opencode-code-path:";

/// Desktop's disclosure triangle (`Collapsible.Arrow`): `fill-triangle-down` at 14 pt, pointing
/// right while shut, in the colour its arrows inherit.
pub fn arrow(p: &Pal, open: bool) -> El {
    icon(if open { &ic::FILL_TRIANGLE_DOWN } else { &TRIANGLE_RIGHT }, 14.0).key("chevron").color(p.v1_text_base)
}

/// `fill-triangle-down` turned a quarter left, as Desktop's CSS turns a shut arrow, drawn so
/// rather than rotated so it needs no layer of its own.
static TRIANGLE_RIGHT: Icon = Icon {
    name: "oc-fill-triangle-right",
    view: 16.0,
    body: r#"<path d="M6.75194 10.62376C6.41861 10.81816 6 10.57776 6 10.19186V5.8079C6 5.422 6.41861 5.1816 6.75194 5.376L10.5096 7.56797C10.8404 7.76091 10.8404 8.23881 10.5096 8.43175L6.75194 10.62376Z" fill="currentColor"/>"#,
};

/// Text people can select and copy, as Desktop's content is (`user-select: text`); the chrome
/// around it (triggers, labels, buttons) stays unselectable.
pub fn readable(p: &Pal, t: El) -> El {
    t.selectable(true).selection_color(p.timeline_selection)
}

/// Between an open row's line and the first line of text nested under it: the New timeline's
/// small gap (`--timeline-gap-s`, 16 text to text), less the 6 under the 28 pt trigger's 16 pt
/// line and the 2 of half-leading over nested Markdown.
pub const NESTED_GAP: f32 = 8.0;
/// Between two 28 pt rows around 16 pt lines, and between an open row and a row nested under it:
/// 16 from text to text.
pub const ROW_GAP: f32 = 4.0;

/// What hangs under an open row in the New timeline (`timeline-next.css`, "Anything nested
/// under a row"), `gap` under it: 14 in, from a 1 pt line 2 in from the row's leading edge in
/// the border's colour at half strength, running about from the nested content's first line to
/// its last.
pub fn nested(p: &Pal, gap: f32, content: El) -> El {
    let line = p.border.alpha(p.border.a * 0.5);
    let ink = if gap < NESTED_GAP { 6.0 } else { 2.0 };
    let rail = el().key("rail").pin(Some(2.0), Some(gap + ink), None, Some(ink)).w(1.0).fill(line);
    col().key("nested").w_full().pt(gap).pl(14.0).child(rail).child(content)
}

/// Interactive nested rail strip (`nested-rail.ts`, `timeline-next.css:130-146`): 14 pt hit strip
/// holding a 1 pt rail 2 pt in that rests at half `p.border`, brightens to full `p.border` on
/// hover, and runs `collapse` on click without entering Tab order.
pub fn nested_rail<S: 'static>(
    p: &Pal,
    top: f32,
    bottom: f32,
    collapse: impl Fn(&mut S, &mut rune::Tx) + 'static,
) -> El {
    let line = el()
        .key("rail-line")
        .pin(Some(2.0), Some(0.0), None, Some(0.0))
        .w(1.0)
        .fill(p.border)
        .group_opacity(0.5, 1.0);
    el().key("rail")
        .pin(Some(0.0), Some(top), None, Some(bottom))
        .w(14.0)
        .cursor(rune::Cursor::Pointer)
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .focusable(false)
        .on_press_tx(collapse)
        .child(line)
}

/// Like [`nested`], with an interactive rail strip that brightens on hover and collapses the
/// open row on click (`nested-rail.ts`, `timeline-next.css:130-146`).
pub fn nested_collapsible<S: 'static>(
    p: &Pal,
    gap: f32,
    content: El,
    collapse: impl Fn(&mut S, &mut rune::Tx) + 'static,
) -> El {
    let ink = if gap < NESTED_GAP { 6.0 } else { 2.0 };
    let rail = nested_rail(p, gap + ink, ink, collapse);
    col().key("nested").w_full().pt(gap).pl(14.0).child(rail).child(content)
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
/// the theme's `syntax-*` (shell commands plain on `layer-02`), invisible rules, outside list
/// markers, the browser's disabled checkboxes, the Console's data table, and quotes in muted text
/// 8 pt past a hairline rule (`markdown.css:258-265`).
/// Selected text, and a quote's source while its comment is written, takes the timeline's
/// selection colour (`quote-highlight.css`).
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
        // Kit's default code margins are Desktop's: 12 over a block, 24 under it, so two blocks in a
        // row stand 24 apart (`markdown.css:420-422`, `pre { margin: 12px 0 24px }`).
        // Code no token colours: Pierre's `editor.foreground`, the v1 `text-base`.
        .code_block_text(p.syntax.parameter)
        // A fence no highlighter colours takes `.highlights`' muted text (`markdown.css:276`).
        .plain_code_text(p.muted)
        .shell_code(p.code_bg, p.border_muted, p.text)
        // `border-inline-start: 0.5px solid border-base`, `padding-inline-start: 0.5rem`, `text-muted`.
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
        // Wide tables and code scroll sideways (`overflow: auto`): code at a hard edge with
        // its bar hidden, a table with `scrollbar.css`'s thumb in a strip under its rows.
        .text_width(width)
        .wide_fade(0.0)
        .table_scroll_bar(p.border_muted, p.q.border_strong)
        .link_icons(14.0, 4.0)
        .code_block_font(CODE)
        .selection(p.timeline_selection)
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

/// Desktop's activity spinner, Soft Radar (`ActivityRing`, `spinner-radar.ts`): 14px, icon-muted.
pub fn activity_ring(p: &Pal) -> El {
    activity_ring_sized(p, 14.0)
}

/// The spinner at `size`: [`busy_glyph`].
pub fn activity_ring_sized(p: &Pal, size: f32) -> El {
    busy_glyph(crate::palette::kit(p), size)
}

/// The one busy glyph, rune-kit's `Spinner` at `size` in [`busy_ink`]: every row, capsule and
/// card that works draws this, so they all read alike at any size.
pub fn busy_glyph(k: &rune_kit::Kit, size: f32) -> El {
    rune_kit::Spinner::new().size(size).color(busy_ink(k)).build(k).key("ring")
}

/// A busy tab's glyph (`TabNavItem`'s `ActivityRing data-mark="busy"`): [`busy_glyph`], but in
/// `icon_base` in dark, where Desktop's muted dim dots vanish against the tab
/// (`tab-nav.css:64-67`). The top and side tabs and the narrow drawer's tab list show it.
pub fn tab_busy_glyph(k: &rune_kit::Kit, size: f32) -> El {
    rune_kit::Spinner::new().size(size).color(if k.dark { k.c.icon_base } else { busy_ink(k) }).build(k).key("ring")
}

/// The busy glyph's ink in light and dark: `icon_muted` (`activity-ring.css`,
/// `session-progress-indicator-v2.css`).
pub fn busy_ink(k: &rune_kit::Kit) -> Color {
    k.c.icon_muted
}

/// Desktop's eased fade into clear (`index.css:224-243`, `select-dialog.css:131-148`): `bg` at
/// full strength through its sixteen stops, as CSS writes an eased curve as a stop list.
pub fn eased_fade(bg: Color) -> [(f32, Color); 16] {
    [
        (0.0, 1.0), (0.0793, 0.920456), (0.1414, 0.849947), (0.19, 0.786813), (0.2285, 0.729394), (0.2605, 0.676028),
        (0.2895, 0.625055), (0.3191, 0.574815), (0.3527, 0.523647), (0.394, 0.46989), (0.4465, 0.411884), (0.5136, 0.347969),
        (0.599, 0.276484), (0.7062, 0.195767), (0.8387, 0.10416), (1.0, 0.0),
    ]
    .map(|(at, a)| (at, bg.alpha(bg.a * a)))
}

/// `t` as the locale's date formats read it.
pub fn moment(t: LocalTime) -> opencode_i18n::date::Moment {
    opencode_i18n::date::Moment { year: t.year, month: t.month, day: t.day, hour: t.hour, minute: t.minute, offset: t.offset }
}

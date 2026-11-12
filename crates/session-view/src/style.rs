//! Shared timeline typography, Markdown styling, Find marks, reasoning disclosure, and the
//! Soft Radar activity spinner.

use std::ops::Range;

use rune::{Anim, Color, Curve, Easing, El, Icon, TextHighlight, TextStyle, Tx, col, el, icon, row, text};
use rune_kit::{Markdown, MarkdownStyle};
use rune_theme_opencode::icons as ic;

use crate::TimelineHost;
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
const MD_H: [TextStyle; 4] = [
    TextStyle::new(17.0, 600.0).lh(25.5),
    TextStyle::new(15.0, 600.0).lh(22.5),
    TextStyle::new(14.0, 670.0).lh(21.0),
    TextStyle::new(14.0, 530.0).lh(21.0),
];
pub const CODE: TextStyle = TextStyle::new(13.0, 400.0).mono().lh(20.0);
/// A tool's code (`--v2-font-family-code` at 440, tracking -0.04): a console's text, a shell's
/// command, Execute's calls.
pub const TOOL_CODE: TextStyle = TextStyle::new(13.0, 440.0).mono().lh(20.0).tracking(-0.04);

/// `--container-session`: 744 including 24 px padding on each side.
pub const CONTENT_W: f32 = 696.0;

/// Desktop's dock: the composer and all that stacks with it sit 12 pt off the chat's sides and
/// bottom, so at full width they stand 12 pt wider than the reply text on each side.
pub const DOCK_SIDE: f32 = 12.0;
pub const DOCK_W: f32 = CONTENT_W + 2.0 * DOCK_SIDE;

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
}

/// Tells this frame's session views the window is narrow; the host calls it before building them.
pub fn set_narrow(narrow: bool) {
    NARROW.set(narrow);
}

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

/// 64ch of the bubble's 14 px Inter: Desktop caps a bubble at `min(82%, 64ch)`.
pub const BUBBLE_MAX: f32 = 565.0;

pub const ACTIVE_KEY: &str = "find-active";

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

/// A thought in a tool group, as Desktop's `AssistantReasoningContent` (`message-content.tsx`)
/// draws it in a compact `BasicTool`: a 28 pt trigger reading "Thought" and how long it took
/// ("0s", faint), or "Thinking" shimmering with its latest heading while it streams shut, the
/// disclosure triangle 6 after them, shown under the pointer and while open
/// (`collapsible.css:27-45`); open, the thought in 13/20 muted Markdown 4 below, 12 in
/// (`basic-tool.css:350-354`, `message-part.css:672-709`). `secs` is `None` until it finishes;
/// `live`, the turn still runs.
#[allow(clippy::too_many_arguments)]
pub fn reasoning<S: TimelineHost>(s: &S, p: &Pal, key: &str, body: &str, secs: Option<i64>, live: bool, open: bool, fade: f32) -> El {
    let streaming = live && secs.is_none();
    let title = if streaming { crate::working::shimmer(s, p, opencode_i18n::t!("ui.sessionTurn.status.thinking"), UI_MED, 0.0) } else { text(opencode_i18n::t!("ui.message.thought")).font(UI_MED).color(p.text) };
    let subtitle = match (streaming, secs) {
        (true, _) if !open => reasoning_heading(body),
        (false, Some(secs)) => Some(opencode_i18n::t!("ui.message.duration.seconds", count = opencode_i18n::integer(secs))),
        _ => None,
    };
    let a = el().key("arrow").w(16.0).h(24.0).center().child(arrow(p, open));
    let trigger = row()
        .key("trigger")
        .role("button")
        .label(if streaming { opencode_i18n::t!("ui.sessionTurn.status.thinking") } else { opencode_i18n::t!("ui.message.thought") })
        .expanded(open)
        .busy(streaming)
        .h(28.0)
        .gap(6.0)
        .center_y()
        .hover_group()
        // The triangle fades in under the pointer (`collapsible.css:26-29`).
        .hover_motion(HOVER_FADE, HOVER_FADE)
        // "Thinking" crossfades to "Thought" while the triangle slides over.
        .animate_layout()
        .on_press_tx(crate::disclosure::set::<S>(key.to_owned(), open, format!("{key}/trigger")))
        .child(title.key(if streaming { "thinking" } else { "thought" }))
        .children(subtitle.map(|t| text(t).key("subtitle").font(UI).color(p.faint).shrink().min_w(0.0).truncate_fade()))
        .child(if open { a } else { a.group_opacity(0.0, 1.0) });
    let mut c = col().w_full().child(trigger);
    if open {
        let style = markdown_style(p, p.muted, content_w() - 48.0).body_font(THOUGHT).heading_font(1, THOUGHT).heading_font(2, THOUGHT).heading_font(3, THOUGHT).heading_font(4, THOUGHT);
        let md = markdown_styled::<S, (&[Range<usize>], rune_kit::Highlight)>(p, body, fade, streaming, style, Vec::new(), Vec::new(), None, &[], None);
        c = c.child(col().key("thought").w_full().pt(4.0).pl(12.0).child(md));
    }
    c
}

/// A thought's Markdown: 13/20 at the body weight (`message-part.css:677-682`).
const THOUGHT: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0);

/// The heading a streaming thought last wrote, an ATX heading or a line in bold
/// (`projection.ts`'s `reasoningHeading`).
fn reasoning_heading(body: &str) -> Option<String> {
    body.lines().rev().find_map(|line| {
        let l = line.trim();
        let atx = l.trim_start_matches('#');
        if l.starts_with('#') && l.len() - atx.len() <= 6 && atx.starts_with([' ', '\t']) {
            return Some(atx.trim().trim_end_matches('#').trim().to_owned()).filter(|h| !h.is_empty());
        }
        let bold = l.strip_prefix("**").and_then(|r| r.strip_suffix("**")).or_else(|| l.strip_prefix("__").and_then(|r| r.strip_suffix("__")));
        bold.map(|b| b.trim().to_owned()).filter(|b| !b.is_empty())
    })
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
        // A block sits as far from the words after it as from the words before (`timeline::rhythm`).
        .code_margin(crate::timeline::rhythm::BLOCK, crate::timeline::rhythm::BLOCK)
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

/// `copied` are the copy buttons showing their ticks (`copied::current`), for code blocks' own.
/// `chips` are the tooltips of the session chips in `src` (`session_chips::tips`), by link.
#[allow(clippy::too_many_arguments)]
pub fn markdown_styled<S: TimelineHost, H: rune_kit::markdown::HighlightItem>(
    p: &Pal,
    src: &str,
    fade: f32,
    streaming: bool,
    style: MarkdownStyle,
    highlights: Vec<H>,
    copied: Vec<String>,
    media: Option<crate::images::markdown::MdMedia>,
    chips: &[(String, String)],
    tentative: Option<rune::Direction>,
) -> El {
    use crate::user_parts::session_of_link;
    // Its texts join the timeline's selection group rather than forming one of their own, so
    // a drag runs on out of this reply into the rest of the timeline.
    let full = std::rc::Rc::new(style.clone());
    let pal = *p;
    let rewritten = S::rewrite_markdown(src);
    let tips = chips.to_vec();
    let mut md = Markdown::new(rewritten.as_deref().unwrap_or(src))
        .style(style)
        .highlights(highlights)
        .reveal(fade)
        .streaming(streaming)
        .selectable(true)
        .scroll_wide(true)
        .diagram_frame(move |card, d| crate::diagram::frame::<S>(&pal, card, d, &full))
        .code_frame(move |card, c| code_card::<S>(&pal, card, c, &copied))
        .on_link_tx(|s: &mut S, url: &str, tx: &mut Tx| match session_of_link(url) {
            Some(id) => s.open_session(id, tx),
            None => s.open_link(url, tx),
        })
        .link_help(move |url| tips.iter().find(|(l, _)| l == url).map(|(_, t)| t.clone()).or_else(|| S::reply_link_help(url)));
    if !chips.is_empty() {
        // Only a reply with chips takes right-clicks on its links; others keep the reply's menu.
        md = md.link_style(move |url| crate::session_chips::look(&pal, url)).on_link_secondary(|s: &mut S, url: &str, press, tx: &mut Tx| {
            if let Some(id) = session_of_link(url) {
                s.session_chip_menu(id, press, tx);
            }
        });
    }
    let md = match media {
        Some(m) => crate::images::markdown_pictures::<S>(md, m),
        None => md,
    };
    // Each block reads the way its own words do, code always left to right; until a reply's
    // first words arrive it reads the way its prompt does.
    let md = match tentative {
        Some(d) => md.dir_tentative(d),
        None => md,
    };
    md.build(crate::palette::kit(p)).selection_group(false)
}

/// A code block with Desktop's Copy button (`markdown.css`'s `markdown-copy-button`): a 24 pt
/// ghost icon button 4 pt in from the top right, fading in over 0.15 s while the pointer is over
/// the block (`markdown.css:352-381`), its glyph a tick for a moment after a copy.
fn code_card<S: TimelineHost>(p: &Pal, card: El, c: &rune_kit::CodeFrame<'_>, copied: &[String]) -> El {
    let key = copy_key("code-copy", &(c.index, c.source));
    let button = copy_button::<S>(p, c.source, key, opencode_i18n::t!("ui.message.copy"), copied);
    let actions = if c.open { Vec::new() } else { S::code_block_actions(p, c.language, c.source) };
    let buttons = if actions.is_empty() {
        button.pin(None, Some(4.0), Some(4.0), None)
    } else {
        row().key("code-actions").gap(2.0).pin(None, Some(4.0), Some(4.0), None).children(actions).child(button)
    };
    // It also shows while the keyboard is on it or a plugin's action beside it
    // (`markdown.css:379-382`'s `:focus-within`).
    col()
        .w_full()
        .hover_group_focus()
        .hover_motion(HOVER_FADE, HOVER_FADE)
        .child(card)
        .child(buttons.group_opacity(0.0, 1.0))
}

/// A copy button's `copied::mark_copied` mark: `prefix` and a hash of `what`.
fn copy_key(prefix: &str, what: &impl std::hash::Hash) -> String {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    what.hash(&mut h);
    format!("{prefix}-{:016x}", h.finish())
}

/// The 24 pt ghost button that copies `text`, its glyph a tick for a moment after (while
/// `copied` holds its `key`), its tooltip `label` until then.
fn copy_button<S: TimelineHost>(p: &Pal, text: &str, key: String, label: &'static str, copied: &[String]) -> El {
    let done = copied.contains(&key);
    let label = if done { opencode_i18n::t!("ui.message.copied") } else { label };
    let text = text.to_owned();
    row()
        .key("copy")
        .role("button")
        .label(label)
        .help(label)
        .help_above()
        .size(24.0, 24.0)
        .center()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            tx.copy(text.clone());
            crate::copied::mark_copied(s, key.clone(), tx);
        })
        .child(icon(if done { &ic::CHECK } else { &ic::OUTLINE_COPY }, 16.0).color(p.icon_muted))
}

/// `markdown_view` with its body text in `color`, its diagrams at most `width` points wide.
pub fn markdown_tinted<S: TimelineHost>(p: &Pal, src: &str, fade: f32, streaming: bool, color: Color, width: f32) -> El {
    markdown_with_highlights::<S>(p, src, fade, streaming, color, width, Vec::new(), Vec::new(), None, &[], None)
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
    copied: Vec<String>,
    media: Option<crate::images::markdown::MdMedia>,
    chips: &[(String, String)],
    tentative: Option<rune::Direction>,
) -> El {
    markdown_styled::<S, _>(p, src, fade, streaming, markdown_style(p, color, width), highlights, copied, media, chips, tentative)
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

/// A stopped turn's tail: Desktop's quiet Interrupted, the last item of its reply footer
/// (`text-part-meta`: 12/18 at 440, tracking 0.05, faint; `message-part.css:608-623`). It rises
/// out of Working's place as Working goes, so the stop reads as the run settling (Kit's).
pub fn interrupted_label(p: &Pal) -> El {
    use crate::working::{COPY, COPY_AT, H, LANE_PAD};
    use rune::{Spring, SpringPair, Transition};
    let rise = LANE_PAD + H / 2.0 - (COPY_AT.1 + COPY / 2.0);
    let arrive = Transition::offset(0.0, rise).combined(Transition::opacity());
    row()
        .key("interrupted")
        .h(COPY)
        .center_y()
        .transition(Transition::asymmetric(arrive, Transition::opacity()).springs(SpringPair::new(Spring::new(0.24, 0.0), Spring::new(0.12, 0.0))))
        .child(text(opencode_i18n::t!("ui.message.interrupted")).font(TextStyle::new(12.0, 440.0).lh(18.0).tracking(0.05)).color(p.faint))
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

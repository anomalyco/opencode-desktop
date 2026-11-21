//! Timeline-specific Markdown, reasoning disclosure, and stopped-turn helpers.
//!
//! Shared pure UI tokens, typography, layout metrics, Markdown styling, Find marks, and spinners
//! live in [`opencode_ui::style`] and are re-exported here for compatibility.

use std::ops::Range;

use rune::{Color, El, TextStyle, Tx, col, el, icon, row, text};
use rune_kit::{Markdown, MarkdownStyle};
use rune_theme_opencode::icons as ic;

use crate::TimelineHost;
use crate::palette::Pal;

pub use opencode_ui::style::*;

/// A thought's Markdown: 13/20 at the body weight (`message-part.css:677-682`).
const THOUGHT: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0);

/// A thought in a tool group, as Desktop's `AssistantReasoningContent` (`message-content.tsx`)
/// draws it in a compact `BasicTool`: a 28 pt trigger reading "Thought" and how long it took
/// ("0s", faint), or "Thinking" shimmering with its latest heading while it streams shut, the
/// disclosure triangle 6 after them, shown under the pointer and while open
/// (`collapsible.css:27-45`); open, the thought in 13/20 muted Markdown 4 below, 12 in
/// (`basic-tool.css:350-354`, `message-part.css:672-709`). `secs` is `None` until it finishes;
/// `live`, the turn still runs.
///
/// The New timeline's `ThoughtRow` (`timeline-next/tools/thought.tsx`) reads like a row of
/// activity: "Thought" and how long it took, both muted at 440, or "Thinking" shimmering while it
/// streams, and opening shows the thought hung from the nested rail ([`nested`]).
#[allow(clippy::too_many_arguments)]
pub fn reasoning<S: TimelineHost>(s: &S, p: &Pal, key: &str, body: &str, secs: Option<i64>, live: bool, open: bool, fade: f32) -> El {
    let streaming = live && secs.is_none();
    let title = if streaming { crate::working::shimmer(s, p, opencode_i18n::t!("ui.sessionTurn.status.thinking"), UI, 0.0) } else { text(opencode_i18n::t!("ui.message.thought")).font(UI).color(p.muted) };
    let subtitle = match (streaming, secs) {
        (false, Some(secs)) => Some(crate::turns::elapsed(secs * 1000)),
        _ => None,
    };
    let a = el().key("arrow").w(16.0).h(24.0).center().child(arrow(p, open));
    let trigger = row()
        .key("trigger")
        .role("button")
        // Its words as official's trigger names itself from them, "Thought 4s": the texts below
        // are then nothing new for a reader, and the tree leaves them out.
        .label(match (streaming, &subtitle) {
            (true, _) => opencode_i18n::t!("ui.sessionTurn.status.thinking").to_string(),
            (false, Some(took)) => format!("{} {took}", opencode_i18n::t!("ui.message.thought")),
            (false, None) => opencode_i18n::t!("ui.message.thought").to_string(),
        })
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
        .children(subtitle.map(|t| text(t).key("subtitle").font(UI.features("'tnum' 1")).color(p.muted).shrink().min_w(0.0).truncate_fade()))
        .child(if open { a } else { a.group_opacity(0.0, 1.0) });
    let mut c = col().w_full().child(trigger);
    if open && !body.trim().is_empty() {
        let style = markdown_style(p, p.muted, content_w() - 48.0).body_font(THOUGHT).heading_font(1, THOUGHT).heading_font(2, THOUGHT).heading_font(3, THOUGHT).heading_font(4, THOUGHT);
        let md = markdown_styled::<S, (&[Range<usize>], rune_kit::Highlight)>(p, body, fade, streaming, style, Vec::new(), Vec::new(), None, &[], None, None);
        c = c.child(crate::disclosure::reveal(s.knobs(), nested(p, NESTED_GAP, col().key("thought").w_full().child(md))));
    }
    c
}

/// `copied` are the copy buttons showing their ticks (`copied::current`), for code blocks' own.
/// `chips` are the tooltips of the session chips in `src` (`session_chips::tips`), by link.
/// `code_links` is the inline code naming files the host found (`TimelineHost::code_links`).
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
    code_links: Option<std::rc::Rc<std::collections::HashSet<String>>>,
) -> El {
    use crate::user_parts::{session_of_link, session_of_raw_link};
    // Its texts join the timeline's selection group rather than forming one of their own, so
    // a drag runs on out of this reply into the rest of the timeline.
    let full = std::rc::Rc::new(style.clone());
    let pal = *p;
    let rewritten = S::rewrite_markdown(src);
    let md_src = rewritten.as_deref().unwrap_or(src);
    let has_session_refs = !chips.is_empty() || md_src.contains("session-raw:");
    let tips = chips.to_vec();
    let code_click = code_links.clone();
    let code_help = code_links.clone();
    let code_menu = code_links.clone();
    let mut md = Markdown::new(md_src)
        .style(style)
        .highlights(highlights)
        .reveal(fade)
        .streaming(streaming)
        .selectable(true)
        .scroll_wide(true)
        .diagram_frame(move |card, d| crate::diagram::frame::<S>(&pal, card, d, &full))
        .code_frame(move |card, c| code_card::<S>(&pal, card, c, &copied))
        .on_link_tx(move |s: &mut S, url: &str, tx: &mut Tx| match session_of_link(url) {
            // A session's chip is the app's own address: a middle click opens nothing (Desktop's
            // window-open handler refuses its own pages).
            Some(_) if tx.button() == rune::PointerButton::Middle => {}
            Some(id) => s.open_session(id, tx),
            None if session_of_raw_link(url).is_some() => {}
            // Block attacker-authored `[click](opencode-code-path:...)` links in Markdown: only
            // app-verified inline code spans present in `code_links` may use `CODE_LINK`.
            None if url
                .strip_prefix(CODE_LINK)
                .is_some_and(|code| !code_click.as_ref().is_some_and(|f| f.contains(code))) => {}
            None => s.open_link(url, tx),
        })
        // A path in code (verified by `code_links`) or raw unknown session ID has no tooltip;
        // attacker-authored `opencode-code-path:` Markdown links NEVER suppress their hover URL.
        .link_help(move |url| {
            let genuine_code = url
                .strip_prefix(CODE_LINK)
                .is_some_and(|code| code_help.as_ref().is_some_and(|f| f.contains(code)));
            match genuine_code || session_of_raw_link(url).is_some() {
                true => None,
                false => tips.iter().find(|(l, _)| l == url).map(|(_, t)| t.clone()).or_else(|| S::reply_link_help(url)),
            }
        });
    if has_session_refs {
        md = md.link_style(move |url| crate::session_chips::look(&pal, url));
    }
    let message = media.as_ref().and_then(|m| m.message.clone());
    md = md.on_link_secondary(move |s: &mut S, url: &str, press, tx: &mut Tx| {
        if let Some(id) = session_of_link(url).or_else(|| session_of_raw_link(url)) {
            s.session_chip_menu(id, press, tx);
        } else if url
            .strip_prefix(CODE_LINK)
            .is_some_and(|code| !code_menu.as_ref().is_some_and(|f| f.contains(code)))
        {
            if let Some(mid) = &message {
                let at = crate::images::menu::MenuAt {
                    session: s.active_session_id().map(str::to_owned),
                    message: Some(mid.to_string()),
                    image: None,
                };
                s.context_menu(at, press, tx);
            }
        } else if !s.reply_link_menu(url, press, tx) && let Some(mid) = &message {
            let at = crate::images::menu::MenuAt {
                session: s.active_session_id().map(str::to_owned), message: Some(mid.to_string()), image: None,
            };
            s.context_menu(at, press, tx);
        }
    });
    // A path a finished reply names that the host found is a link in `text-code-path`, without
    // its chip (`markdown.css:443-450`), underlined under the pointer; a press opens it.
    if let Some(found) = code_links {
        md = md.code_link(p.v2.text_code_path, move |code| found.contains(code).then(|| format!("{CODE_LINK}{code}")));
    }
    // Each block reads the way its own words do, code always left to right; until a reply's
    // first words arrive it reads the way its prompt does.
    let md = match tentative {
        Some(d) => md.dir_tentative(d),
        None => md,
    };
    match media {
        Some(m) => crate::images::markdown_pictures::<S>(md, m),
        None => md,
    }.build(crate::palette::kit(p)).selection_group(false)
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
    // (`markdown.css:379-382`'s `:focus-within`), and always where nothing hovers: its
    // `opacity: 0` is `@media (hover: hover)` only (`markdown.css:359`).
    let idle = if hover_none() { 1.0 } else { 0.0 };
    col()
        .w_full()
        .hover_group_focus()
        .hover_motion(HOVER_FADE, HOVER_FADE)
        .child(card)
        .child(buttons.group_opacity(idle, 1.0))
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
        // `forceOpen`: "Copied" stays up for the tick's time, though the press hid the tooltip.
        .help_open(done)
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
    markdown_with_highlights::<S>(p, src, fade, streaming, color, width, Vec::new(), Vec::new(), None, &[], None, None)
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
    code_links: Option<std::rc::Rc<std::collections::HashSet<String>>>,
) -> El {
    markdown_styled::<S, _>(p, src, fade, streaming, markdown_style(p, color, width), highlights, copied, media, chips, tentative, code_links)
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

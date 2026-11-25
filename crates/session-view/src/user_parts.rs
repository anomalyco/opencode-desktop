//! User-bubble inline parts shared across timeline hosts: `@` mention badges and quote cards.

use std::ops::Range;

use opencode_kit::mention::{self, MentionKind};
pub use opencode_kit::mention::{
    BubbleMark, bubble_marks, mention_bubble, mention_kind_of, recorded_mentions, session_link,
    session_of_link, session_of_raw_link, session_raw_link, short_session_id,
};
use opencode_kit::model::{FileComment, PromptComment, Quote};
use rune::{Color, El, Icon, PillStyle, TextStyle, Tx, col, el, icon, row, text};

use crate::TimelineHost;
use crate::focus::FocusExt as _;
use crate::palette::Pal;

/// `attachment-card.css`: 11/14, with room for Inter's descenders where it truncates.
pub const CARD_TITLE: TextStyle = TextStyle::new(11.0, 530.0).lh(14.0).tracking(0.05);
pub const CARD_SUBTITLE: TextStyle = TextStyle::new(11.0, 440.0).lh(14.0).tracking(0.05);

const BUTTON: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
const PREVIEW: usize = 200;
const TOOLTIP: usize = 600;
const BOUNDED: usize = 5;

pub use opencode_ui::style::{OUTLINE_QUOTE, SESSION};

pub static COMMENT: Icon = Icon {
    name: "comment",
    view: 16.0,
    body: r#"<path d="M13.5 2.5H2.5V13.5H4.5L5.5 11.5H13.5V2.5Z" stroke="currentColor" stroke-miterlimit="10" stroke-linecap="square"/>"#,
};

// ---------------------------------------------------------------------------------------------
// Mention badges

pub fn mention_tint(dark: bool) -> f32 {
    if dark { 0.035 } else { 0.16 }
}

pub fn mention_neutral(dark: bool) -> f32 {
    if dark { 0.035 } else { 0.09 }
}

/// Colours the shared palette doesn't carry: `syntax-type` for agents, `syntax-property` for
/// files, `icon-info-active`.
pub struct MentionInk {
    pub agent: Color,
    pub file: Color,
    pub info: Color,
}

pub fn mention_ink(p: &Pal) -> MentionInk {
    if let Some(code) = p.code {
        return MentionInk { agent: code.ty, file: code.property, info: p.info };
    }
    let hex = |s: &str| rune::color::hex(s).unwrap_or(p.text);
    MentionInk {
        agent: if p.dark { hex("#ecf58c") } else { hex("#596600") },
        file: if p.dark { hex("#ff9ae2") } else { hex("#ed6dc8") },
        info: p.info,
    }
}

/// A mention's badge: a chip and label, no icon, as Desktop draws mentions as text alone.
pub fn mention_style(p: &Pal, kind: MentionKind) -> PillStyle {
    let k = mention_ink(p);
    let tinted = |c: Color| PillStyle::new().bg(c.alpha(mention_tint(p.dark))).fg(c);
    let neutral = |fg: Color| PillStyle::new().bg(p.text.alpha(mention_neutral(p.dark))).fg(fg);
    match kind {
        MentionKind::Agent => tinted(k.agent),
        MentionKind::File => tinted(k.file),
        MentionKind::Directory => neutral(p.muted),
        MentionKind::Skill | MentionKind::Session => neutral(p.text),
    }
}

/// The composer's pill look by id. Pills that aren't mentions keep Rune's default.
pub fn mention_pill_style(p: &Pal) -> impl Fn(&str) -> PillStyle + 'static {
    let p = *p;
    move |id: &str| mention_kind_of(id).map_or_else(PillStyle::new, |k| mention_style(&p, k))
}

/// What resting on a session badge says.
pub fn session_tip(id: &str, title: Option<&str>) -> String {
    match title {
        Some(t) => opencode_i18n::t!("rune.session.mention.openTitled", title = t, id = id),
        None => opencode_i18n::t!("rune.session.mention.open", id = id),
    }
}

pub type BubbleBadges = (String, Vec<(Range<usize>, PillStyle)>, Vec<rune::Link>, Vec<Range<usize>>);

/// Message `id`'s bubble text and its mention badges: pills styled by kind, a session's with
/// its icon, and a link per session badge (opening it when the host can). A badge keeps its own
/// order in the line around it: a file's or folder's reads left to right as a path does, any
/// other's the way its own name does.
pub fn bubble_badges<S: TimelineHost>(s: &S, p: &Pal, id: &str, body: &str) -> BubbleBadges {
    let title = |sid: &str| s.session_ref(sid).map(|r| r.title).filter(|t| !t.trim().is_empty()).or_else(|| s.session_title(sid));
    let (text, marks) = bubble_marks(body, &mention::user_mentions(id), title);
    let links = marks
        .iter()
        .filter(|_| s.can_open_session())
        .filter_map(|m| {
            let sid = m.session.as_deref()?;
            let link = rune::Link::new(m.range.clone(), session_link(sid)).role("button").help(session_tip(sid, m.title.as_deref()));
            Some(link)
        })
        .collect();
    let paths = marks.iter().filter(|m| matches!(m.kind, MentionKind::File | MentionKind::Directory)).map(|m| m.range.clone()).collect();
    let pills = marks
        .into_iter()
        .map(|m| {
            let style = mention_style(p, m.kind);
            (m.range, if m.session.is_some() { style.icon(&SESSION) } else { style })
        })
        .collect();
    (text, pills, links, paths)
}

// ---------------------------------------------------------------------------------------------
// Quote cards

pub fn quote_preview(text: &str) -> String {
    one_line(&text.chars().take(PREVIEW).collect::<String>())
}

/// A card's line as Desktop's `white-space: nowrap` draws it: every run of spaces and line
/// breaks one space. A one-line text keeps its hard breaks, so a quote of two paragraphs
/// would stack three lines in its 14 pt row, over the line above it.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A quote that starts on a lowercase letter was most likely cut mid-word or mid-sentence
/// ("elieve escapability…"), so its first letters fade in rather than start hard. The
/// selection doesn't say what came before it, so this is a guess.
fn starts_cut(quote: &str) -> bool {
    quote.trim_start().chars().next().is_some_and(char::is_lowercase)
}

/// `line`, its leading edge faded when `cut`.
fn lead_fade(line: El, cut: bool) -> El {
    if cut { el().shrink().clip_fade(LEAD_FADE, 0.0, 0.0, 0.0).child(line) } else { line }
}

const LEAD_FADE: f32 = 6.0;

fn tooltip(q: &Quote) -> String {
    let text: String = if q.text.chars().count() > TOOLTIP { format!("{}…", q.text.chars().take(TOOLTIP).collect::<String>()) } else { q.text.clone() };
    match q.comment() {
        Some(c) => format!("{c}\n\n{text}"),
        None => text,
    }
}

/// A comment's two-line card (`CommentCard`): a quote's, or a file comment's, whose title is
/// the comment over the file's icon and its name with the lines (`format.ts:3`).
pub fn comment_card(p: &Pal, c: &PromptComment, hint: bool, active: bool, wide: bool) -> El {
    match c {
        PromptComment::Quote(q) => quote_card(p, q, hint, active, wide),
        PromptComment::File(f) => file_card(p, f, active, wide),
        PromptComment::Note(n) => note_card(p, n, active, wide),
    }
}

fn note_card(p: &Pal, n: &opencode_kit::model::NoteComment, active: bool, wide: bool) -> El {
    let comment = n.comment.trim();
    let heading = if comment.is_empty() { one_line(&n.label) } else { one_line(comment) };
    let sub_label = if comment.is_empty() {
        n.subtitle.as_deref().unwrap_or(&n.label)
    } else {
        &n.label
    };
    let glyph = match n.icon.as_deref() {
        Some("select-element") => &rune_theme_opencode::icons::SELECT_ELEMENT,
        _ => &rune_theme_opencode::icons::GLOBE,
    };
    let title = text(heading).key("title").font(CARD_TITLE).color(p.text).truncate_fade().w_full();
    let subtitle = row()
        .key("subtitle")
        .w_full()
        .h(16.0)
        .gap(4.0)
        .center_y()
        .min_w(0.0)
        .child(icon(glyph, 12.0).color(p.muted))
        .child(text(sub_label.to_owned()).font(CARD_SUBTITLE).color(p.muted).truncate_fade());
    let c = col()
        .key("card")
        .padding(8.0)
        .gap(6.0)
        .radius(6.0)
        .fill(if p.dark { p.hover } else { p.bg_base })
        .box_shadow(Pal::ring_in(if active { p.q.border_strong } else { p.border }))
        .child(title)
        .child(subtitle);
    let c = if wide { c.w_full() } else { c.w(160.0) };
    let tip = if comment.is_empty() {
        n.title.clone().unwrap_or_else(|| n.label.clone())
    } else {
        comment.to_owned()
    };
    c.help(tip).help_above()
}

/// `getFilenameTruncated`: a name over `max` characters keeps its extension after an ellipsis.
pub fn filename_truncated(path: &str, max: usize) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max {
        return name.to_owned();
    }
    let dot = name.rfind('.').filter(|&d| d > 0).map(|d| name[..d].chars().count());
    let extension: String = dot.map_or_else(String::new, |d| chars[d..].iter().collect());
    let available = max as i64 - extension.chars().count() as i64 - 1;
    if available <= 0 {
        return chars[..max - 1].iter().collect::<String>() + "…";
    }
    chars[..available as usize].iter().collect::<String>() + "…" + &extension
}

fn file_card(p: &Pal, f: &FileComment, active: bool, wide: bool) -> El {
    let title = text(one_line(&f.comment)).key("title").font(CARD_TITLE).color(p.text).truncate_fade().w_full();
    let subtitle = row()
        .key("subtitle")
        .w_full()
        // `attachment-card-subtitle` is 16 tall: its 14 px line with the 12 px icon inline.
        .h(16.0)
        .gap(4.0)
        .center_y()
        .min_w(0.0)
        .child(icon(rune_theme_opencode::file_icons::file(&f.path), 12.0))
        .child(text(format!("{}{}", filename_truncated(&f.path, 14), f.lines_label())).font(CARD_SUBTITLE).color(p.muted).truncate_fade());
    let c = col()
        .key("card")
        .padding(8.0)
        .gap(6.0)
        .radius(6.0)
        .fill(if p.dark { p.hover } else { p.bg_base })
        .box_shadow(Pal::ring_in(if active { p.q.border_strong } else { p.border }))
        .child(title)
        .child(subtitle);
    let c = if wide { c.w_full() } else { c.w(160.0) };
    c.help(f.comment.clone()).help_above()
}

/// A quote's two-line card.
pub fn quote_card(p: &Pal, q: &Quote, hint: bool, active: bool, wide: bool) -> El {
    let comment = q.comment();
    let quote = quote_preview(&q.text);
    let title = comment.map_or_else(|| quote.clone(), one_line);
    let label = if comment.is_some() { quote } else { opencode_i18n::t!("ui.commentCard.quote").to_owned() };
    let cut = starts_cut(&q.text);
    let mut title = text(title).key("title").font(CARD_TITLE).color(p.text).truncate_fade().w_full();
    let tip = tooltip(q);
    if comment.is_none() {
        title = title.help_when_truncated(tip.clone());
    }
    let title = lead_fade(title, cut && comment.is_none()).w_full();
    // `attachment-card-subtitle` is 16 tall, as on a file's card.
    let line = |key: &str, glyph, label: String, color, cut| {
        row().key(key).w_full().h(16.0).gap(4.0).center_y().min_w(0.0)
            .child(icon(glyph, 12.0).color(color))
            .child(lead_fade(text(label).font(CARD_SUBTITLE).color(color).truncate_fade(), cut))
    };
    let quoted = cut && comment.is_some();
    let mut subtitle = el().key("subtitle").w_full().h(16.0);
    if hint {
        let edit = if comment.is_some() { opencode_i18n::t!("ui.commentCard.editComment") } else { opencode_i18n::t!("ui.commentCard.addComment") };
        subtitle = if crate::style::hover_none() && comment.is_none() {
            subtitle.child(line("comment-line", &COMMENT, edit.to_owned(), p.text, false))
        } else {
            subtitle.child(line("quote-line", &OUTLINE_QUOTE, label, p.muted, quoted).group_opacity(1.0, 0.0))
                .child(line("comment-line", &COMMENT, edit.to_owned(), p.text, false).absolute(0.0, 0.0).group_opacity(0.0, 1.0))
        };
    } else {
        subtitle = subtitle.child(line("quote-line", &OUTLINE_QUOTE, label, p.muted, quoted));
    }
    let mut c = col()
        .key("card")
        .padding(8.0)
        .gap(6.0)
        .radius(6.0)
        .fill(if p.dark { p.hover } else { p.bg_base })
        .box_shadow(Pal::ring_in(if active { p.q.border_strong } else { p.border }))
        .child(title)
        .child(subtitle);
    c = if wide { c.w_full() } else { c.w(160.0) };
    if comment.is_some() && !active { c.help(tip) } else { c }
}

/// A sent prompt's comment cards (`UserMessageComments`), when it has any; under its words. Where
/// the host shows quotes inline (`TimelineHost::inline_quotes`), those instead.
pub fn quotes_on_message<S: TimelineHost>(s: &S, p: &Pal, id: &str, body: &str, pending: bool) -> Option<El> {
    if s.inline_quotes() {
        return crate::user_quotes::view(s, p, id, pending);
    }
    let quotes = opencode_kit::model::user_comments(id);
    if quotes.is_empty() {
        return None;
    }
    let bounded = !body.trim().is_empty();
    let open = !bounded || s.quote_more_open(id) || quotes.len() <= BOUNDED;
    let shown = if open { &quotes[..] } else { &quotes[..BOUNDED] };
    let mut c = col().key("quotes").w_full().gap(8.0);
    c = if bounded { c.max_w(318.0).mt(8.0) } else { c.max_w(320.0) };
    for (i, q) in shown.iter().enumerate() {
        c = c.child(comment_card(p, q, false, false, true).key(format!("quote-{i}")));
    }
    if !open {
        let id = id.to_owned();
        c = c.child(
            row()
                .key("show-more")
                .role("button")
                .label_static(opencode_i18n::t!("ui.common.showMore"))
                .h(24.0)
                .px(9.0)
                .center_y()
                .radius(4.0)
                .focus_inset(p)
                .hover(p.hover)
                .press(p.pressed)
                .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                    tx.instant();
                    s.open_quote_more(id.clone());
                })
                .child(text(opencode_i18n::t!("ui.common.showMore")).font(BUTTON).color(p.muted)),
        );
    }
    Some(c)
}

//! User-bubble inline parts shared across timeline hosts: `@` mention badges and quote cards.

use std::ops::Range;

use opencode_kit::mention::{self, MentionKind, UserMention};
use opencode_kit::model::{FileComment, PromptComment, Quote};
use rune::{Color, El, Icon, PillStyle, TextStyle, Tx, col, el, icon, row, text};

use crate::TimelineHost;
use crate::palette::Pal;

/// `attachment-card.css`: 11/14, with room for Inter's descenders where it truncates.
pub const CARD_TITLE: TextStyle = TextStyle::new(11.0, 530.0).lh(14.0).tracking(0.05);
pub const CARD_SUBTITLE: TextStyle = TextStyle::new(11.0, 440.0).lh(14.0).tracking(0.05);

const BUTTON: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
const PREVIEW: usize = 200;
const TOOLTIP: usize = 600;
const BOUNDED: usize = 5;

pub static OUTLINE_QUOTE: Icon = Icon {
    name: "outline-quote",
    view: 16.0,
    body: r#"<path d="M9.5 8.49927C10.8333 8.49927 12.1667 8.49927 13.5 8.49927C13.5 9.74927 13.5 11.2493 13.5 12.4993C12.1667 12.4993 10.8333 12.4993 9.5 12.4993V7C9.5 5 10.5 3.5 13 3.5001" stroke="currentColor"/><path d="M2.5 8.49927C3.83333 8.49927 5.16667 8.49927 6.5 8.49927C6.5 9.74927 6.5 11.2493 6.5 12.4993C5.16667 12.4993 3.83333 12.4993 2.5 12.4993V7C2.5 5 3.5 3.5 6 3.5001" stroke="currentColor"/>"#,
};

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

/// The kind of a composer pill by its id (`agent:`, `skill:`, `file:`, `reference:`, `session:`).
pub fn mention_kind_of(id: &str) -> Option<MentionKind> {
    Some(match id.split_once(':')?.0 {
        "agent" => MentionKind::Agent,
        "skill" => MentionKind::Skill,
        "file" => MentionKind::File,
        "reference" => MentionKind::Directory,
        "session" => MentionKind::Session,
        _ => return None,
    })
}

/// The composer's pill look by id. Pills that aren't mentions keep Rune's default.
pub fn mention_pill_style(p: &Pal) -> impl Fn(&str) -> PillStyle + 'static {
    let p = *p;
    move |id: &str| mention_kind_of(id).map_or_else(PillStyle::new, |k| mention_style(&p, k))
}

/// Desktop's `speech-bubble`: what a session mention's chip leads with.
pub static SESSION: Icon = Icon {
    name: "speech-bubble",
    view: 20.0,
    body: r#"<path d="M18.3334 10.0003C18.3334 5.57324 15.0927 2.91699 10.0001 2.91699C4.90749 2.91699 1.66675 5.57324 1.66675 10.0003C1.66675 11.1497 2.45578 13.1016 2.5771 13.3949C2.5878 13.4207 2.59839 13.4444 2.60802 13.4706C2.69194 13.6996 3.04282 14.9364 1.66675 16.7684C3.5186 17.6538 5.48526 16.1982 5.48526 16.1982C6.84592 16.9202 8.46491 17.0837 10.0001 17.0837C15.0927 17.0837 18.3334 14.4274 18.3334 10.0003Z" stroke="currentColor" stroke-linecap="square"/>"#,
};

/// A session id short enough for a chip: `ses_` and its first eight characters.
pub fn short_session_id(id: &str) -> &str {
    let end = id.char_indices().nth(12).map_or(id.len(), |(i, _)| i);
    &id[..end]
}

/// The recorded mentions still on an `@` in `body`, minus any over an `@session:` token, and
/// recorded session mentions once the text has tokens. Those come from a local echo of the
/// text as typed (a session pill reading `@SEER`), so on the server's copy their offsets can
/// land inside a token and cut it (p279); the token is the truth.
pub fn recorded_mentions<'a>(body: &'a str, mentions: &'a [UserMention]) -> impl Iterator<Item = &'a UserMention> + 'a {
    let tokens = mention::session_tokens(body);
    mentions.iter().filter(move |m| {
        let stale = !tokens.is_empty() && m.kind == MentionKind::Session;
        let cuts = tokens.iter().any(|(r, _)| m.range.start < r.end && r.start < m.range.end);
        !stale && !cuts && body.get(m.range.clone()).is_some_and(|t| t.starts_with('@'))
    })
}

/// One badge in a bubble's shown text; `session` is the id a session mention opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BubbleMark {
    pub range: Range<usize>,
    pub kind: MentionKind,
    pub session: Option<String>,
    /// The session's title, when known.
    pub title: Option<String>,
}

/// A bubble's text and badges: the message's [`recorded_mentions`], and each `@session:<id>`
/// token as one badge reading the session's title, or its short id when `title` (which names a
/// session by id) doesn't know it.
pub fn bubble_marks(body: &str, mentions: &[UserMention], title: impl Fn(&str) -> Option<String>) -> (String, Vec<BubbleMark>) {
    let mut marks: Vec<(Range<usize>, MentionKind, Option<(&str, Option<String>)>)> =
        recorded_mentions(body, mentions).map(|m| (m.range.clone(), m.kind, None)).collect();
    for (r, id) in mention::session_tokens(body) {
        marks.push((r, MentionKind::Session, Some((id, title(id).filter(|t| !t.trim().is_empty())))));
    }
    // A session id pasted as it is reads as its badge too, once its title is known.
    for (r, id) in crate::session_chips::plain_ids(body) {
        if let Some(t) = title(id).filter(|t| !t.trim().is_empty()) {
            marks.push((r, MentionKind::Session, Some((id, Some(t)))));
        }
    }
    marks.sort_by_key(|(r, _, _)| r.start);
    let (mut out, mut ranges, mut at) = (String::with_capacity(body.len()), Vec::new(), 0);
    for (r, kind, session) in marks {
        if r.start < at {
            continue;
        }
        out.push_str(&body[at..r.start]);
        let start = out.len();
        match &session {
            Some((id, t)) => out.push_str(t.as_deref().unwrap_or_else(|| short_session_id(id))),
            None => out.push_str(&body[r.clone()]),
        }
        let (session, title) = session.map_or((None, None), |(id, t)| (Some(id.to_owned()), t));
        ranges.push(BubbleMark { range: start..out.len(), kind, session, title });
        at = r.end;
    }
    out.push_str(&body[at..]);
    (out, ranges)
}

/// [`bubble_marks`] as the shown text and each badge's bytes and kind.
pub fn mention_bubble(body: &str, mentions: &[UserMention], title: impl Fn(&str) -> Option<String>) -> (String, Vec<(Range<usize>, MentionKind)>) {
    let (text, marks) = bubble_marks(body, mentions, title);
    (text, marks.into_iter().map(|m| (m.range, m.kind)).collect())
}

/// What resting on a session badge says.
pub fn session_tip(id: &str, title: Option<&str>) -> String {
    match title {
        Some(t) => opencode_i18n::t!("rune.session.mention.openTitled", title = t, id = id),
        None => opencode_i18n::t!("rune.session.mention.open", id = id),
    }
}

/// The `on_link` id a session badge reports, and back.
pub fn session_link(id: &str) -> String {
    format!("session:{id}")
}

pub fn session_of_link(link: &str) -> Option<&str> {
    link.strip_prefix("session:")
}

/// Message `id`'s bubble text and its mention badges: pills styled by kind, a session's with
/// its icon, and a link per session badge (opening it when the host can).
pub fn bubble_badges<S: TimelineHost>(s: &S, p: &Pal, id: &str, body: &str) -> (String, Vec<(Range<usize>, PillStyle)>, Vec<rune::Link>) {
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
    let pills = marks
        .into_iter()
        .map(|m| {
            let style = mention_style(p, m.kind);
            (m.range, if m.session.is_some() { style.icon(&SESSION) } else { style })
        })
        .collect();
    (text, pills, links)
}

// ---------------------------------------------------------------------------------------------
// Quote cards

pub fn quote_preview(text: &str) -> String {
    text.chars().take(PREVIEW).collect()
}

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
    }
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
    let title = text(f.comment.clone()).key("title").font(CARD_TITLE).color(p.text).truncate_fade().w_full();
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
    let title = comment.map_or_else(|| quote.clone(), str::to_owned);
    let label = if comment.is_some() { quote } else { opencode_i18n::t!("ui.commentCard.quote").to_owned() };
    let mut title = text(title).key("title").font(CARD_TITLE).color(p.text).truncate_fade().w_full();
    let tip = tooltip(q);
    if comment.is_none() {
        title = title.help_when_truncated(tip.clone());
    }
    let line = |key: &str, glyph, label: String, color| {
        row().key(key).w_full().h(14.0).gap(4.0).center_y().min_w(0.0)
            .child(icon(glyph, 12.0).color(color))
            .child(text(label).font(CARD_SUBTITLE).color(color).truncate_fade())
    };
    let mut subtitle = el().key("subtitle").w_full().h(14.0);
    if hint {
        let edit = if comment.is_some() { opencode_i18n::t!("ui.commentCard.editComment") } else { opencode_i18n::t!("ui.commentCard.addComment") };
        subtitle = subtitle
            .child(line("quote-line", &OUTLINE_QUOTE, label, p.muted).group_opacity(1.0, 0.0))
            .child(line("comment-line", &COMMENT, edit.to_owned(), p.text).absolute(0.0, 0.0).group_opacity(0.0, 1.0));
    } else {
        subtitle = subtitle.child(line("quote-line", &OUTLINE_QUOTE, label, p.muted));
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

/// A sent prompt's comment cards (`UserMessageComments`), when it has any; under its words.
pub fn quotes_on_message<S: TimelineHost>(s: &S, p: &Pal, id: &str, body: &str) -> Option<El> {
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
            row().key("show-more").role("button").h(24.0).px(9.0).center_y().radius(4.0).hover(p.hover).press(p.pressed)
                .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                    tx.instant();
                    s.open_quote_more(id.clone());
                })
                .child(text(opencode_i18n::t!("ui.common.showMore")).font(BUTTON).color(p.muted)),
        );
    }
    Some(c)
}


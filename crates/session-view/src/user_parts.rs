//! User-bubble inline parts shared across timeline hosts: `@` mention badges and quote cards.

use std::ops::Range;

use opencode_kit::mention::{self, MentionKind, UserMention};
use opencode_kit::model::Quote;
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

/// The kind of a composer pill by its id (`agent:`, `skill:`, `file:`, `session:`).
pub fn mention_kind_of(id: &str) -> Option<MentionKind> {
    Some(match id.split_once(':')?.0 {
        "agent" => MentionKind::Agent,
        "skill" => MentionKind::Skill,
        "file" => MentionKind::File,
        "session" => MentionKind::Session,
        _ => return None,
    })
}

/// The composer's pill look by id. Pills that aren't mentions keep Rune's default.
pub fn mention_pill_style(p: &Pal) -> impl Fn(&str) -> PillStyle + 'static {
    let p = *p;
    move |id: &str| mention_kind_of(id).map_or_else(PillStyle::new, |k| mention_style(&p, k))
}

/// A bubble's text and badges: the message's recorded mentions (those still on an `@` in the
/// text, as an echo's may not be once the server's copy lands), and its `@session:<id>`
/// tokens shown as `@<title>` when the session is known. `title` names a session by id.
pub fn mention_bubble(body: &str, mentions: &[UserMention], title: impl Fn(&str) -> Option<String>) -> (String, Vec<(Range<usize>, MentionKind)>) {
    let mut marks: Vec<(Range<usize>, MentionKind, Option<String>)> =
        mentions.iter().filter(|m| body.get(m.range.clone()).is_some_and(|t| t.starts_with('@'))).map(|m| (m.range.clone(), m.kind, None)).collect();
    for (r, id) in mention::session_tokens(body) {
        if !marks.iter().any(|(m, _, _)| m.start < r.end && r.start < m.end) {
            marks.push((r, MentionKind::Session, title(id).map(|t| format!("@{t}"))));
        }
    }
    marks.sort_by_key(|(r, _, _)| r.start);
    let (mut out, mut ranges, mut at) = (String::with_capacity(body.len()), Vec::new(), 0);
    for (r, kind, shown) in marks {
        if r.start < at {
            continue;
        }
        out.push_str(&body[at..r.start]);
        let start = out.len();
        out.push_str(shown.as_deref().unwrap_or(&body[r.clone()]));
        ranges.push((start..out.len(), kind));
        at = r.end;
    }
    out.push_str(&body[at..]);
    (out, ranges)
}

/// Message `id`'s bubble text and styled mention pills.
pub fn bubble_badges<S: TimelineHost>(s: &S, p: &Pal, id: &str, body: &str) -> (String, Vec<(Range<usize>, PillStyle)>) {
    let (text, marks) = mention_bubble(body, &mention::user_mentions(id), |sid| s.session_title(sid));
    (text, marks.into_iter().map(|(r, k)| (r, mention_style(p, k))).collect())
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

/// A quote's two-line card.
pub fn quote_card(p: &Pal, q: &Quote, hint: bool, active: bool, wide: bool) -> El {
    let comment = q.comment();
    let quote = quote_preview(&q.text);
    let title = comment.map_or_else(|| quote.clone(), str::to_owned);
    let label = if comment.is_some() { quote } else { "Quote".to_owned() };
    let mut title = text(title).key("title").font(CARD_TITLE).color(p.text).truncate().w_full();
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
        let edit = if comment.is_some() { "Edit comment" } else { "Add comment" };
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
        .border(0.5, if active { p.q.border_strong } else { p.border })
        .child(title)
        .child(subtitle);
    c = if wide { c.w_full() } else { c.w(160.0) };
    if comment.is_some() && !active { c.help(tip) } else { c }
}

/// A sent prompt's quote cards (`UserMessageComments`), when it has any.
pub fn quotes_on_message<S: TimelineHost>(s: &S, p: &Pal, id: &str, body: &str) -> Option<El> {
    let quotes = opencode_kit::model::user_quotes(id);
    if quotes.is_empty() {
        return None;
    }
    let bounded = !body.trim().is_empty();
    let open = !bounded || s.quote_more_open(id) || quotes.len() <= BOUNDED;
    let shown = if open { &quotes[..] } else { &quotes[..BOUNDED] };
    let mut c = col().key("quotes").w_full().gap(8.0);
    c = if bounded { c.max_w(318.0).mt(8.0) } else { c.max_w(320.0) };
    for (i, q) in shown.iter().enumerate() {
        c = c.child(quote_card(p, q, false, false, true).key(format!("quote-{i}")));
    }
    if !open {
        let id = id.to_owned();
        c = c.child(
            row().key("show-more").role("button").h(24.0).px(9.0).center_y().radius(4.0).hover(p.hover).press(p.pressed)
                .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                    tx.instant();
                    s.open_quote_more(id.clone());
                })
                .child(text("Show more").font(BUTTON).color(p.muted)),
        );
    }
    Some(c)
}

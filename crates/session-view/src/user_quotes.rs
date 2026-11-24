//! A sent prompt's quotes and comments inline in its bubble, after its words
//! (`packages/session-ui/src/message/user-message-quotes.tsx`, `message-part.css`'s
//! `user-message-quotes`; opencode#65897): each quote behind a bar, at most three lines until a
//! press opens it, with its comment under it; a file or a note as one line behind the bar.
//! Desktop ships this to local and dev builds first (`inline-quotes.ts`); the others keep the
//! comment cards (`user_parts::quotes_on_message`).

use opencode_kit::model::{FileComment, NoteComment, PromptComment, Quote};
use rune::{El, Span, Tx, col, el, icon, rich, row, text};

use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{BUBBLE, readable};

/// `-webkit-line-clamp: 3`.
const CLAMP: usize = 3;

/// The bar and the room after it (`border-inline-start: 2px`, `padding-inline-start: 8px`).
const BAR: f32 = 2.0;
const BAR_GAP: f32 = 8.0;

fn open_key(id: &str, i: usize) -> String {
    format!("quote-open:{id}:{i}")
}

/// Prompt `id`'s quotes and comments as the bubble holds them, when it has any. `pending`: a
/// steer the server hasn't delivered, which reads fainter.
pub fn view<S: TimelineHost>(s: &S, p: &Pal, id: &str, pending: bool) -> Option<El> {
    let comments = opencode_kit::model::user_comments(id);
    if comments.is_empty() {
        return None;
    }
    // What a quote's text wraps at: the bubble's words' width, less the bar.
    let width = crate::fold::text_width(crate::fold::viewport_width(), false) - BAR - BAR_GAP;
    let mut out = col().key("quotes").gap(8.0).min_w(0.0);
    for (i, c) in comments.iter().enumerate() {
        let (quoted, comment) = match c {
            PromptComment::Quote(q) => (quote::<S>(s, p, id, i, q, pending, width), q.comment().map(str::to_owned)),
            PromptComment::File(f) => (Some(behind_bar(p, pending, file(p, f), true)), Some(f.comment.clone())),
            PromptComment::Note(n) => (Some(behind_bar(p, pending, note(p, n), true)), Some(n.comment.clone())),
        };
        let comment = comment.filter(|c| !c.trim().is_empty()).map(|c| remark(p, pending, c.trim()));
        out = out.child(col().key(format!("quote-{i}")).gap(4.0).min_w(0.0).children(quoted).children(comment));
    }
    Some(out)
}

/// `user-message-quote`: `inner` behind the 2 pt `border-strong` bar (`border-base` while pending).
/// A quote of one line (`single`) has its bar run 1 past the line (`[data-single-line]`:
/// `padding-bottom: 1px`, opencode#66149): Inter centres capitals on the line, so lowercase
/// reads about 1 low against the bar, and this puts the text half a point above the bar's
/// middle, between the two. A wrapped quote sits evenly between the bar's ends already.
fn behind_bar(p: &Pal, pending: bool, inner: El, single: bool) -> El {
    let bar = el().key("bar").w(BAR).self_stretch().fill(if pending { p.border } else { p.q.border_strong });
    let inner = if single { el().key("line").min_w(0.0).shrink().pb(1.0).child(inner) } else { inner };
    row().key("quote").gap(BAR_GAP).min_w(0.0).child(bar).child(inner)
}

/// `user-message-quote-text`: the quoted words, muted, clamped to three lines until a press
/// opens them; only a quote that runs past the clamp is a button. Blank lines around a selection
/// carry nothing, and a quote of only whitespace shows just its comment.
fn quote<S: TimelineHost>(s: &S, p: &Pal, id: &str, i: usize, q: &Quote, pending: bool, width: f32) -> Option<El> {
    let quoted = q.text.trim();
    if quoted.is_empty() {
        return None;
    }
    let key = open_key(id, i);
    let open = s.expanded().contains(&key);
    // Laid out, not `fold::lines`: that answers its hard line breaks alone for a text too short
    // to pass the prompt's twelve-line fold, and a quote of four wrapped lines is one such.
    let (_, height) = crate::fold::measure(quoted, width);
    let lines = quoted.split('\n').count().max((height / BUBBLE.line_px()).round() as usize);
    let expandable = open || lines > CLAMP;
    let ink = if pending { p.faint } else { p.muted };
    let mut words = readable(p, text(quoted.to_owned()).key("text").font(BUBBLE).color(ink).wrap_anywhere()).dir_by_paragraph().shrink().min_w(0.0);
    if !open {
        words = words.max_lines(CLAMP);
    }
    if !expandable {
        return Some(behind_bar(p, pending, words, lines <= 1));
    }
    // `[data-expandable]:hover, :focus-visible`: the words take the base ink.
    let row_key = format!("u-{id}");
    let button = col()
        .key("quote-text")
        .role("button")
        .label(quoted.to_owned())
        .expanded(open)
        .focusable(true)
        .shrink()
        .min_w(0.0)
        .radius(4.0)
        .cursor(rune::Cursor::Pointer)
        .hover_group_focus()
        .hover_motion(rune::Anim::INSTANT, rune::Anim::INSTANT)
        .instant_changes()
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            crate::disclosure::toggle_expanded(s, &key);
            tx.keep_on_screen(row_key.clone());
        })
        .child(words.group_color(p.text));
    Some(behind_bar(p, pending, button, false))
}

/// `user-message-quote-reference` for a file: its icon, then its name and lines (`:118-124`,
/// `:42`, nothing for a whole file), left to right.
fn file(p: &Pal, f: &FileComment) -> El {
    let label = format!("{}{}", crate::user_parts::filename_truncated(&f.path, 40), f.lines_label());
    row()
        .key("reference")
        .gap(6.0)
        .center_y()
        .min_w(0.0)
        .child(icon(rune_theme_opencode::file_icons::file(&f.path), 14.0))
        .child(text(label).font(BUBBLE).color(p.muted).dir(rune::Direction::Ltr).truncate())
}

/// `user-message-quote-reference` for a note: its icon, its title (else its label), and its
/// subtitle, faint.
fn note(p: &Pal, n: &NoteComment) -> El {
    let glyph = match n.icon.as_deref() {
        Some("select-element") => &rune_theme_opencode::icons::SELECT_ELEMENT,
        _ => &rune_theme_opencode::icons::GLOBE,
    };
    let title = n.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).unwrap_or(&n.label);
    let detail = n.subtitle.as_deref().map(str::trim).filter(|t| !t.is_empty());
    row()
        .key("reference")
        .gap(6.0)
        .center_y()
        .min_w(0.0)
        .child(icon(glyph, 14.0).color(p.muted))
        .child(text(title.to_owned()).font(BUBBLE).color(p.muted).dir(rune::Direction::Ltr).truncate())
        .children(detail.map(|d| text(d.to_owned()).key("detail").font(BUBBLE).color(p.faint).single_line()))
}

/// `user-message-quote-comment`: the comment in the bubble's ink, its `@path` mentions (which
/// travel with the prompt as attached files) in the accent (`[data-highlight="file"]`).
fn remark(p: &Pal, pending: bool, comment: &str) -> El {
    let mut spans = Vec::new();
    let mut at = 0;
    for word in comment.split_inclusive(char::is_whitespace) {
        let bare = word.trim_end();
        if let Some(i) = bare.find('@').filter(|&i| bare.len() > i + 1) {
            spans.push(Span::new(at + i, at + bare.len()).color(p.accent));
        }
        at += word.len();
    }
    readable(p, rich(comment.to_owned(), spans).key("comment").font(BUBBLE).color(if pending { p.muted } else { p.text }).wrap_anywhere()).dir_by_paragraph()
}

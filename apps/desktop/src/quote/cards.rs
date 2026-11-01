//! Quotes as cards (Desktop's `CommentCard`): in the composer, where a click opens the quote's
//! comment editor (`quote-comment.tsx`), and in the user bubble that sent them.

use rune::{Align, Cx, El, Key, Side, TextEdit, TextStyle, Transition, Tx, col, el, icon, row, spacer, text, text_input};

use opencode_kit::model::Quote;

use crate::App;
use crate::attachments::{CARD_SUBTITLE, CARD_TITLE, remove_button};
use crate::palette::Pal;
use crate::quote::{COMMENT, OUTLINE_QUOTE, Pending};

const FIELD: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
const META: TextStyle = TextStyle::new(11.0, 530.0).lh(16.0).tracking(0.05);
const BUTTON: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
/// A card shows this much of its quote.
const PREVIEW: usize = 200;
const TOOLTIP: usize = 600;
/// A bubble shows this many cards before Show more.
const BOUNDED: usize = 5;
const EDITOR_W: f32 = 380.0;

fn preview(text: &str) -> String {
    text.chars().take(PREVIEW).collect()
}

/// The card's tooltip: the comment over the quote (Desktop's `quoteTooltip`).
fn tooltip(q: &Quote) -> String {
    let text: String = if q.text.chars().count() > TOOLTIP { format!("{}…", q.text.chars().take(TOOLTIP).collect::<String>()) } else { q.text.clone() };
    match q.comment() {
        Some(c) => format!("{c}\n\n{text}"),
        None => text,
    }
}

/// A quote's two-line card. Its title is the comment, or the quote without one; its subtitle the
/// quote mark over "Quote", or the quote under a comment. `hint` is the composer's: hovered, it
/// offers to add or edit the comment.
fn card(p: &Pal, q: &Quote, hint: bool, active: bool, wide: bool) -> El {
    let comment = q.comment();
    let quote = preview(&q.text);
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

/// The composer's quote cards, before its files (`editor.tsx`): a click toggles the quote's
/// comment editor, and a round remove button shows on hover.
pub fn chips(s: &App, p: &Pal) -> Vec<El> {
    s.quote.items.iter().map(|item| chip(s, p, item)).collect()
}

fn chip(s: &App, p: &Pal, item: &Pending) -> El {
    let key = item.key;
    let active = s.quote.editing == Some(key);
    let body = card(p, &item.quote, true, active, false)
        .role("button")
        .label("Quote")
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            if s.quote.editing == Some(key) { close_editor(s) } else { open_editor(s, key) }
        });
    let remove = remove_button(p, "quote", 160.0, move |s| s.quote.remove(key));
    el().key(chip_key(key)).hover_group().child(body).child(remove)
}

pub(crate) fn chip_key(key: u64) -> String {
    format!("quote-chip-{key}")
}

pub fn open_editor(s: &mut App, key: u64) {
    let Some(item) = s.quote.items.iter().find(|q| q.key == key) else { return };
    let mut draft = TextEdit::new(item.quote.comment.as_deref().unwrap_or(""));
    draft.focused = true;
    s.quote.draft = draft;
    s.quote.editing = Some(key);
    s.composer.focused = false;
}

fn close_editor(s: &mut App) {
    s.quote.editing = None;
    s.quote.draft.focused = false;
    s.composer.focused = true;
}

/// Enter or Comment: the comment, even an empty one, goes on the quote.
fn submit(s: &mut App) {
    let Some(key) = s.quote.editing else { return };
    let comment = s.quote.draft.text.clone();
    if let Some(item) = s.quote.items.iter_mut().find(|q| q.key == key) {
        item.quote.comment = Some(comment);
    }
    close_editor(s);
}

/// Whether the draft still says what the quote's comment does.
fn unchanged(s: &App, key: u64) -> bool {
    let was = s.quote.items.iter().find(|q| q.key == key).and_then(|q| q.quote.comment.as_deref()).unwrap_or("");
    s.quote.draft.text.trim() == was.trim()
}

/// A press outside the editor takes the field's focus, which closes an unchanged editor; one
/// with a new comment stays open.
pub fn poll(s: &mut App) -> bool {
    let Some(key) = s.quote.editing else { return false };
    if !s.quote.items.iter().any(|q| q.key == key) || (!s.quote.draft.focused && unchanged(s, key)) {
        s.quote.editing = None;
        return true;
    }
    false
}

/// The open quote's comment editor, above its card (below when there's no room above).
pub fn editor(s: &App, p: &Pal, cx: &mut Cx<App>) -> Option<El> {
    let key = s.quote.editing?;
    let item = s.quote.items.iter().find(|q| q.key == key)?;
    let window = cx.layout_size("zoom").map_or(f32::INFINITY, |[w, _]| w / s.zoom.max(1e-3));
    let small = |key: &str, label: &str| {
        row().key(key).role("button").label(label.to_owned()).h(24.0).px(9.0).center_y().radius(4.0)
    };
    let cancel = small("quote-comment-cancel", "Cancel")
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(|s: &mut App, tx: &mut Tx| { tx.instant(); close_editor(s) })
        .child(text("Cancel").font(BUTTON).color(p.text));
    let submit_button = small("quote-comment-submit", "Comment")
        .fill(p.contrast)
        .hover(p.contrast.mix(p.text, 0.08))
        .on_press_tx(|s: &mut App, tx: &mut Tx| { tx.instant(); submit(s) })
        .child(text("Comment").font(BUTTON).color(p.q.contrast_text));
    let field = text_input(&s.quote.draft, FIELD)
        .key("quote-comment-field")
        .color(p.text)
        .w_full()
        .min_h(64.0)
        .placeholder("Add a comment about this quote", p.faint)
        .caret(p.text, p.selection)
        .on_edit(|s: &mut App| &mut s.quote.draft)
        .on_input_key(|s: &mut App, ev, tx| {
            if ev.key == Key::Escape {
                tx.instant();
                close_editor(s);
                return true;
            }
            false
        })
        .on_submit(|s: &mut App, tx: &mut Tx| { tx.instant(); submit(s) });
    let footer = row()
        .key("quote-comment-footer")
        .w_full()
        .gap(4.0)
        .center_y()
        // Its buttons leave the field focused, so an outside press is what closes it.
        .keeps_text_selection()
        .child(row().key("quote-comment-quote").shrink().min_w(0.0).gap(4.0).center_y()
            .child(icon(&OUTLINE_QUOTE, 16.0).color(p.faint))
            .child(text(preview(&item.quote.text)).font(META).color(p.faint).truncate_fade()))
        .child(spacer())
        .child(row().gap(8.0).center_y().child(cancel).child(submit_button));
    let popover = col()
        .key("quote-comment")
        .anchored(chip_key(key), Side::Top, Align::Start, 8.0)
        .w(EDITOR_W.min(window - 16.0))
        .padding(12.0)
        .gap(12.0)
        .radius(6.0)
        .fill(p.layer1)
        .border(0.5, p.ring)
        .shadow(2.0, 4.0, p.shadow_far)
        .shadow(1.0, 2.0, p.shadow)
        .transition(Transition::none())
        .motion(rune::Spring::NONE)
        // Empty space in it takes the press instead of what's under it.
        .on_press(|_: &mut App| {})
        .child(field)
        .child(footer);
    Some(popover)
}

/// A sent prompt's quote cards (`UserMessageComments`), when it has any. Under its words
/// (`body`) they sit inside the bubble, the first five until Show more; a prompt of only quotes
/// shows every card in the bubble's place.
pub fn on_message(s: &App, p: &Pal, id: &str, body: &str) -> Option<El> {
    let quotes = opencode_kit::model::user_quotes(id);
    if quotes.is_empty() {
        return None;
    }
    let bounded = !body.trim().is_empty();
    let open = !bounded || s.quote.more.contains(id) || quotes.len() <= BOUNDED;
    let shown = if open { &quotes[..] } else { &quotes[..BOUNDED] };
    let mut c = col().key("quotes").w_full().gap(8.0);
    c = if bounded { c.max_w(318.0).mt(8.0) } else { c.max_w(320.0) };
    for (i, q) in shown.iter().enumerate() {
        c = c.child(card(p, q, false, false, true).key(format!("quote-{i}")));
    }
    if !open {
        let id = id.to_owned();
        c = c.child(
            row().key("show-more").role("button").h(24.0).px(9.0).center_y().radius(4.0).hover(p.hover).press(p.pressed)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| { tx.instant(); s.quote.more.insert(id.clone()); })
                .child(text("Show more").font(BUTTON).color(p.muted)),
        );
    }
    Some(c)
}

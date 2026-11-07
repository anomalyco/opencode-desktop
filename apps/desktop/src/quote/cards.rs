//! Quotes as cards (Desktop's `CommentCard`): in the composer, where a click opens the quote's
//! comment editor (`quote-comment.tsx`), and in the user bubble that sent them.

use rune::{Align, Cx, El, Key, Side, TextEdit, TextStyle, Transition, Tx, col, el, icon, row, spacer, text, text_input};

use crate::App;
use crate::attachments::remove_button;
use crate::palette::Pal;
use crate::quote::{OUTLINE_QUOTE, Pending};
pub use session_view::user_parts::{quote_card as card, quote_preview as preview};

const FIELD: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
const META: TextStyle = TextStyle::new(11.0, 530.0).lh(16.0).tracking(0.05);
const BUTTON: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
const EDITOR_W: f32 = 380.0;

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
        .box_shadow(Pal::ring_out(p.ring))
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

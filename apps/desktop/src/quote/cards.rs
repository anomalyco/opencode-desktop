//! Quotes as cards (Desktop's `CommentCard`): in the composer, where a click opens the quote's
//! comment editor (`quote-comment.tsx`), and in the user bubble that sent them.

use rune::{El, Tx, el};

use crate::App;
use crate::attachments::remove_button;
use crate::palette::Pal;
use crate::quote::Pending;
pub use session_view::user_parts::quote_card as card;

/// The composer's quote cards, before its files (`editor.tsx`): a click toggles the quote's
/// comment editor, and a round remove button shows on hover.
pub fn chips(s: &App, p: &Pal) -> Vec<El> {
    s.quote.items.iter().map(|item| chip(s, p, item)).collect()
}

fn chip(s: &App, p: &Pal, item: &Pending) -> El {
    let key = item.key;
    let active = s.quote.comment.key() == Some(key);
    let body = card(p, &item.quote, true, active, false)
        .role("button")
        .label(opencode_i18n::t!("session.quote.add"))
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            if s.quote.comment.key() == Some(key) { close_editor(s) } else { open_editor(s, key) }
        });
    let remove = remove_button(p, "quote", 160.0, move |s| s.quote.remove(key));
    el().key(chip_key(key)).hover_group().child(body).child(remove)
}

pub(crate) fn chip_key(key: u64) -> String {
    format!("quote-chip-{key}")
}

/// Opens the quote's comment editor from its card (`quoteComment.set(key)`), with no place in
/// the timeline.
pub fn open_editor(s: &mut App, key: u64) {
    crate::quote::editor::open(s, key, None);
}

fn close_editor(s: &mut App) {
    crate::quote::editor::close(s, false);
}
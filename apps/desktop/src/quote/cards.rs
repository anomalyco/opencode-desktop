//! Comments as cards (Desktop's `CommentCard`): in the composer, where a click opens a quote's
//! comment editor (`quote-comment.tsx`) or reveals a file comment in the view it was written in
//! (`toggleContext`, `openComment`), and in the user bubble that sent them.

use rune::{El, Tx, el};

use crate::App;
use crate::attachments::remove_button;
use crate::palette::Pal;
use crate::quote::Pending;
pub use session_view::user_parts::comment_card as card;

/// The composer's comment cards, before its files (`editor.tsx`), each with a round remove
/// button on hover.
pub fn chips(s: &App, p: &Pal) -> Vec<El> {
    s.quote.items.iter().map(|item| chip(s, p, item)).collect()
}

fn chip(s: &App, p: &Pal, item: &Pending) -> El {
    let key = item.key;
    if item.comment.file().is_some() {
        let active = s.line_comments.active_card == Some(key);
        let body = card(p, &item.comment, false, active, false)
            .role("button")
            .on_press_tx(move |s: &mut App, tx: &mut Tx| s.open_line_comment_card(key, tx));
        let remove = remove_button(p, "comment", 160.0, move |s| s.remove_line_comment_card(key));
        return el().key(chip_key(key)).hover_group().child(body).child(remove);
    }
    let active = s.quote.comment.key() == Some(key);
    let body = card(p, &item.comment, true, active, false)
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

//! Comments as cards (Desktop's `CommentCard`): in the composer, where a click opens a quote's
//! comment editor (`quote-comment.tsx`) or reveals a file comment in the view it was written in
//! (`toggleContext`, `openComment`), and in the user bubble that sent them.

use rune::{El, Tx, el};
use session_view::composer::attachments::remove_button;
use session_view::palette::Pal;

pub use session_view::user_parts::comment_card as card;

use super::{Pending, QuoteHost};

/// The composer's comment cards, before its files (`editor.tsx`), each with a round remove
/// button on hover.
pub fn chips<H: QuoteHost + 'static>(s: &H, p: &Pal) -> Vec<El> {
    s.quotes().items.iter().map(|item| chip::<H>(s, p, item)).collect()
}

fn chip<H: QuoteHost + 'static>(s: &H, p: &Pal, item: &Pending) -> El {
    let key = item.key;
    if item.comment.file().is_some() {
        let active = s.active_line_comment_card() == Some(key);
        let body = card(p, &item.comment, false, active, false)
            .role("button")
            .on_press_tx(move |s: &mut H, tx: &mut Tx| on_file_card_press(s, key, tx));
        let remove = remove_button(p, "comment", 160.0, move |s: &mut H| on_file_card_remove(s, key));
        return el().key(chip_key(key)).hover_group().child(body).child(remove);
    }
    let active = s.quotes().comment.key() == Some(key);
    let body = card(p, &item.comment, true, active, false)
        .role("button")
        .label_static(opencode_i18n::t!("session.quote.add"))
        .on_press_tx(move |s: &mut H, tx: &mut Tx| on_quote_card_press(s, key, tx));
    let remove = remove_button(p, "quote", 160.0, move |s: &mut H| on_quote_card_remove(s, key));
    el().key(chip_key(key)).hover_group().child(body).child(remove)
}

fn on_file_card_press(host: &mut dyn QuoteHost, key: u64, tx: &mut Tx) {
    host.open_line_comment_card(key, tx);
}

fn on_file_card_remove(host: &mut dyn QuoteHost, key: u64) {
    host.remove_line_comment_card(key);
}

fn on_quote_card_press(host: &mut dyn QuoteHost, key: u64, tx: &mut Tx) {
    tx.instant();
    if host.quotes().comment.key() == Some(key) {
        close_editor(host);
    } else {
        open_editor(host, key);
    }
}

fn on_quote_card_remove(host: &mut dyn QuoteHost, key: u64) {
    host.quotes_mut().remove(key);
}

pub fn chip_key(key: u64) -> String {
    format!("quote-chip-{key}")
}

/// Opens the quote's comment editor from its card (`quoteComment.set(key)`), with no place in
/// the timeline.
pub fn open_editor(host: &mut dyn QuoteHost, key: u64) {
    super::editor::open(host, key, None);
}

/// Closes the quote's comment editor without focusing the composer.
pub fn close_editor(host: &mut dyn QuoteHost) {
    super::editor::close(host, false);
}

//! The storybook's Quote story (Timeline › Quote): the finished session with part of its first
//! prompt selected, so the Quote button stands over it. Q quotes it into the composer, M quotes
//! it with the comment editor open, L selects the whole reply (the button goes under it), W the
//! prompt's words again, N sends, C clears, R replays.
//! `--playground Timeline/Quote --script apps/desktop/scripts/quote.json` drags over the first
//! prompt, quotes it, and shows the card in the composer.

use rune::{Play, Tx};

use crate::App;
use crate::stories::{Trigger, replay};

pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'w', label: "Select the prompt's words", run: |s, tx| select_words(s, tx) },
    Trigger { key: 'l', label: "Select the reply", run: |_, tx| tx.select_all_text(REPLY) },
    Trigger { key: 'q', label: "Quote", run: |s, tx| crate::quote::add(s, false, tx) },
    Trigger { key: 'm', label: "Quote and comment", run: |s, tx| crate::quote::add(s, true, tx) },
    Trigger { key: 'n', label: "Send", run: |s, tx| s.send(tx) },
    Trigger { key: 'c', label: "Clear", run: |_, tx| tx.clear_text_selection() },
];

/// The reply with the watchdog's code, its row's key.
const REPLY: &str = "t-msg_001a162c36130003fixture00-0";
/// The session's first prompt.
const PROMPT: &str = "msg_001a162c32a80001fixture00";
const WORDS: std::ops::Range<usize> = 4..38;

fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    tx.after(0.2, |_, tx| tx.scroll_into_view(format!("u-{PROMPT}")));
    tx.after(0.6, |s, tx| select_words(s, tx));
}

fn select_words(_: &mut App, tx: &mut Tx) {
    tx.select_text(format!("text-{PROMPT}"), WORDS);
}

//! The storybook's Quote story (Timeline › Quote): the finished session with part of its first
//! prompt selected, so the Quote button stands over it. Q quotes it into the composer, M quotes
//! it with the comment editor open, L selects the whole reply (the button goes under it), W the
//! prompt's words again, N sends, C clears, R replays.

use rune::Tx;
use rune_knobs::Action;

use crate::App;
use crate::stories::replay;

pub const KEYS: &[Action<App>] = &[
    Action { cap: "R", label: "Replay", run: replay },
    Action { cap: "W", label: "Select the prompt's words", run: select_words },
    Action { cap: "L", label: "Select the reply", run: |_, tx| tx.select_all_text(REPLY) },
    Action { cap: "Q", label: "Quote", run: |s, tx| crate::quote::add(s, false, tx) },
    Action { cap: "M", label: "Quote and comment", run: |s, tx| crate::quote::add(s, true, tx) },
    Action { cap: "N", label: "Send", run: |s, tx| s.send(tx) },
    Action { cap: "C", label: "Clear", run: |_, tx| tx.clear_text_selection() },
];

/// The reply with the watchdog's code, its row's key.
const REPLY: &str = "t-msg_001a162c36130003fixture00-0";
/// The session's first prompt.
const PROMPT: &str = "msg_001a162c32a80001fixture00";
const WORDS: std::ops::Range<usize> = 4..38;

pub fn open(s: &mut App, tx: &mut Tx) {
    s.finish_stream();
    s.play.after(0.2, |_, tx| tx.scroll_into_view(format!("u-{PROMPT}")), tx);
    s.play.after(0.6, select_words, tx);
}

fn select_words(_: &mut App, tx: &mut Tx) {
    tx.select_text(format!("text-{PROMPT}"), WORDS);
}

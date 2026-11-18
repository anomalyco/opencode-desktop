//! The storybook's Quote stories (Timeline › Quote): the finished session with part of its
//! first prompt selected, so the Quote button stands over it. Q quotes it into the composer, M
//! quotes it with the comment editor open, L selects the whole reply (the button goes under
//! it), W the prompt's words again, N sends, C clears, R replays.
//! `--playground Timeline/Quote --script apps/desktop/scripts/quote.json` drags over the first
//! prompt, quotes it, and shows the card in the composer.
//!
//! Timeline › Quote scrolls away (t1564): a prompt quoted with its comment open, the first prompt
//! (variant Top) or the last (variant Bottom). The editor scrolls with its text the whole way,
//! out under the header or the composer, and back with it. A glides the text away, B back.
//! `--script apps/desktop/scripts/quote-away-top.json` (or `-bottom`) swipes the text out and
//! back as a trackpad does.

use crate::fixture_ids::{LAST_PROMPT, PROMPT, REPLY_ROW as REPLY};
use rune::{Play, Tx};

use crate::App;
use crate::stories::{Trigger, replay, scroll_key};

pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'w', label: "Select the prompt's words", run: |s, tx| select_words(s, tx) },
    Trigger { key: 'l', label: "Select the reply", run: |_, tx| tx.select_all_text(REPLY) },
    Trigger { key: 'q', label: "Quote", run: |s, tx| crate::quote::add(s, false, tx) },
    Trigger { key: 'm', label: "Quote and comment", run: |s, tx| crate::quote::add(s, true, tx) },
    Trigger { key: 'n', label: "Send", run: |s, tx| s.send(tx) },
    Trigger { key: 'c', label: "Clear", run: |_, tx| tx.clear_text_selection() },
];

const AWAY_KEYS: &[Trigger] = &[
    replay(scrolls_away),
    Trigger { key: 'a', label: "Scroll the text away", run: |s, tx| away(s, tx) },
    Trigger { key: 'b', label: "Scroll back to the text", run: |s, tx| back(s, tx) },
];

const WORDS: std::ops::Range<usize> = 4..38;
/// "Nice. Can you add a test for the backoff".
const LAST_WORDS: std::ops::Range<usize> = 0..41;

fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    tx.after(0.2, |_, tx| tx.scroll_into_view(format!("u-{PROMPT}")));
    tx.after(0.6, |s, tx| select_words(s, tx));
}

fn select_words(_: &mut App, tx: &mut Tx) {
    tx.select_text(format!("text-{PROMPT}"), WORDS);
}

/// Variant Top quotes the first prompt, Bottom the last.
fn bottom(s: &App) -> bool {
    *s.variant == 1
}

fn scrolls_away(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    if bottom(s) {
        tx.after(0.2, |s, tx| tx.scroll_center_in_instant(scroll_key(s), format!("u-{LAST_PROMPT}")));
        tx.after(0.5, |_, tx| tx.select_text(format!("text-{LAST_PROMPT}"), LAST_WORDS));
    } else {
        tx.after(0.2, |_, tx| tx.scroll_into_view(format!("u-{PROMPT}")));
        tx.after(0.5, |s, tx| select_words(s, tx));
    }
    tx.after(1.4, |s, tx| crate::quote::add(s, true, tx));
}

/// The timeline glides to the end it doesn't show the quoted prompt at.
fn away(s: &mut App, tx: &mut Tx) {
    tx.scroll_to(scroll_key(s), 0.0, if bottom(s) { 0.0 } else { f32::MAX });
}

fn back(s: &mut App, tx: &mut Tx) {
    tx.scroll_center_in(scroll_key(s), format!("u-{}", if bottom(s) { LAST_PROMPT } else { PROMPT }));
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Quote", KEYS),
    crate::stories::Story::session("Timeline/Quote scrolls away", AWAY_KEYS)
        .knobs(&[crate::quote::place::GROUP])
        .variants(&[("Top", 0), ("Bottom", 1)]),
]);

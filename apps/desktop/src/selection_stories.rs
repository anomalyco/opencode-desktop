//! The storybook's Selection story (Timeline › Selection): the finished session, scrolled to
//! the reply with a code block, and then the whole reply selected, as a drag across it would.
//! The highlight is instant. S selects the reply, A everything on screen, W a word of the
//! prompt, P lands a new prompt below (the selection stays), C clears, R replays.

use crate::fixture_ids::{PROMPT, REPLY_ROW as REPLY};
use rune::{Play, Tx};

use crate::App;
use crate::stories::{Trigger, emit, replay, sid};

pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 's', label: "Select the reply", run: |s, tx| select_reply(s, tx) },
    Trigger { key: 'a', label: "Select all on screen", run: |_, tx| tx.select_all_text("timeline") },
    Trigger { key: 'w', label: "Select a word", run: |_, tx| tx.select_text(format!("text-{PROMPT}"), WORD) },
    Trigger { key: 'p', label: "A prompt lands below", run: |s, tx| prompt(s, tx) },
    Trigger { key: 'c', label: "Clear", run: |_, tx| tx.clear_text_selection() },
];

const WORD: std::ops::Range<usize> = 40..46;

fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    tx.after(0.2, |_, tx| tx.scroll_into_view(REPLY));
    tx.after(0.6, |s, tx| select_reply(s, tx));
}

fn select_reply(_: &mut App, tx: &mut Tx) {
    tx.select_all_text(REPLY);
}

fn prompt(s: &mut App, tx: &mut Tx) {
    let id = format!("msg_zzzzselect{:04}", s.transcripts.get(&sid(s)).map_or(0, |t| t.messages.len()));
    emit(s, "session.prompted", serde_json::json!({ "messageID": id, "text": "Does the watchdog also cover a wake from sleep?" }), tx);
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Selection", KEYS),
]);

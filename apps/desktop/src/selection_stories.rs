//! The storybook's Selection story (Timeline › Selection): the finished session, scrolled to
//! the reply with a code block, and then the whole reply selected, as a drag across it would.
//! The highlight is instant. S selects the reply, A everything on screen, W a word of the
//! prompt, P lands a new prompt below (the selection stays), C clears, R replays.

use rune::Tx;
use rune_knobs::Action;

use crate::App;
use crate::stories::{emit, replay, sid};

pub const KEYS: &[Action<App>] = &[
    Action { cap: "R", label: "Replay", run: replay },
    Action { cap: "S", label: "Select the reply", run: select_reply },
    Action { cap: "A", label: "Select all on screen", run: |_, tx| tx.select_all_text("timeline") },
    Action { cap: "W", label: "Select a word", run: |_, tx| tx.select_text(format!("text-{PROMPT}"), WORD) },
    Action { cap: "P", label: "A prompt lands below", run: prompt },
    Action { cap: "C", label: "Clear", run: |_, tx| tx.clear_text_selection() },
];

/// The reply with the watchdog's code, its row's key.
const REPLY: &str = "t-msg_001a162c36130003fixture00-0";
/// The session's first prompt, and "sleeps" in it.
const PROMPT: &str = "msg_001a162c32a80001fixture00";
const WORD: std::ops::Range<usize> = 40..46;

pub fn open(s: &mut App, tx: &mut Tx) {
    s.finish_stream();
    s.play.after(0.2, |_, tx| tx.scroll_into_view(REPLY), tx);
    s.play.after(0.6, select_reply, tx);
}

fn select_reply(_: &mut App, tx: &mut Tx) {
    tx.select_all_text(REPLY);
}

fn prompt(s: &mut App, tx: &mut Tx) {
    let id = format!("msg_zzzzselect{:04}", s.transcripts.get(&sid(s)).map_or(0, |t| t.messages.len()));
    emit(s, "session.prompted", serde_json::json!({ "messageID": id, "text": "Does the watchdog also cover a wake from sleep?" }), tx);
}

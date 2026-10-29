//! The storybook's Mentions story (Composer › Mentions): a sent prompt whose bubble shows a badge
//! for each kind of mention (agent, skill, file, folder, reference, session), and a draft in the
//! composer with the same badges. P lands the prompt again, R replays.

use crate::fixture_ids::{SIXTH as OTHER};
use rune::{Play, TextEdit, Tx};
use serde_json::json;

use crate::App;
use crate::stories::{Trigger, emit, replay, sid};

pub const KEYS: &[Trigger] = &[replay(open), Trigger { key: 'p', label: "The prompt lands", run: |s, tx| prompt(s, tx) }];


fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    s.composer = draft();
    s.composer.focused = true;
    prompt(s, tx);
}

/// "Ask @explore and @opencode about @src/stream.rs in @src/ui/, like @<session> did".
fn draft() -> TextEdit {
    let mut e = TextEdit::new("");
    e.insert("Ask ");
    e.insert_pill("agent:explore", "@explore");
    e.insert(" and ");
    e.insert_pill("skill:opencode", "@opencode");
    e.insert(" about ");
    e.insert_pill("file:src/stream.rs", "@src/stream.rs");
    e.insert(" in ");
    e.insert_pill("file:src/ui/", "@src/ui/");
    e.insert(", like ");
    e.insert_pill(format!("session:{OTHER}"), "@Migrate config loader to the new schema");
    e.insert(" did");
    e
}

/// The sent prompt: its text and the mentions the server records for it.
fn prompt(s: &mut App, tx: &mut Tx) {
    let text = format!("Have @explore use @opencode on @src/stream.rs, @src/ui/ and @docs, as in @session:{OTHER}");
    let at = |m: &str| {
        let start = text.find(m).unwrap_or(0);
        json!({ "start": start, "end": start + m.len(), "text": m })
    };
    let n = s.transcripts.get(&sid(s)).map_or(0, |t| t.messages.len());
    emit(
        s,
        "session.prompted",
        json!({
            "messageID": format!("msg_zzzzmention{n:04}"),
            "text": text,
            "agents": [{ "name": "explore", "mention": at("@explore") }],
            "skills": [{ "id": "opencode", "name": "OpenCode", "mention": at("@opencode") }],
            "files": [
                { "uri": "file:///repo/src/stream.rs", "name": "stream.rs", "mime": "text/plain", "mention": at("@src/stream.rs") },
                { "uri": "file:///repo/src/ui", "name": "ui", "mime": "text/plain", "mention": at("@src/ui/") },
                { "uri": "file:///repo/docs", "name": "docs", "mime": "application/x-directory", "mention": at("@docs") },
            ],
        }),
        tx,
    );
}

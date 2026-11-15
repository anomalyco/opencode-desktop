//! Timeline › Slack links (scene `slack-links`): two prompts that link Slack messages. The
//! fixture's Slack (`fixture.json`'s `slack`) reads the first, which shows `#gooey` with its card
//! under it, and can't read the second, which stays a "Slack message" chip. R replays.

use opencode_kit::model::parse_messages;
use rune::Play;
use serde_json::json;

use crate::App;
use crate::stories::{replay, sid};

/// The message `fixture.json`'s `slack` finds, and the one it can't read.
const FOUND: &str = "https://acme.slack.com/archives/C07GOOEY001/p1792659100412659?thread_ts=1792659090.379729&cid=C07GOOEY001";
const HIDDEN: &str = "https://acme.slack.com/archives/C07PRIVATE01/p1792659160118833";

/// The open session with the two prompts and their replies after its finished turn.
pub(crate) fn scene(s: &mut App) {
    crate::scenes::open_scene(s);
    s.finish_stream();
    let id = sid(s);
    let model = json!({ "id": "claude-opus-5-5", "providerID": "anthropic", "variant": "high" });
    let reply = |id: &str, at: i64, text: &str| {
        json!({ "id": id, "type": "assistant", "agent": "build", "model": model, "finish": "stop",
            "time": { "created": at, "completed": at + 4_000 }, "content": [{ "type": "text", "text": text }] })
    };
    let messages = parse_messages(&json!({ "data": [
        { "id": "msg_zzzzslacklinks0", "type": "user", "time": { "created": 1_792_659_200_000_i64 }, "text": format!("{FOUND} handle this") },
        reply("msg_zzzzslacklinks1", 1_792_659_202_000, "Read the thread. Dax wants the footer hover on messages reworked so it stops looking off in edge cases."),
        { "id": "msg_zzzzslacklinks2", "type": "user", "time": { "created": 1_792_659_260_000_i64 }, "text": format!("{HIDDEN} and also take a look at this one") },
        reply("msg_zzzzslacklinks3", 1_792_659_262_000, "That one is in a channel I can't read with your Slack access, so I only have the link."),
    ]}));
    if let Some(t) = s.transcripts.get_mut(&id) {
        t.messages.extend(messages);
    }
}

fn open(s: &mut App, tx: &mut Play<App>) {
    tx.instant();
    scene(s);
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Slack links", &[replay(open)]),
]);

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [("slack-links", scene)]);

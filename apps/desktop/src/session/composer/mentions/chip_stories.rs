//! Timeline › Session chips (scene `session-chips`): a reply naming its subagent's session id
//! as plain text and in inline code, one the server doesn't have, and the same id in a code
//! block. The known ids read as the session's title; the rest stay as written. R replays.

use opencode_kit::model::Message;
use rune::Play;

use crate::App;
use crate::fixture_ids::CHILD;
use crate::stories::{replay, sid};

const ASK: &str = "msg_zzzzsessionchips0";
const REPLY: &str = "msg_zzzzsessionchips1";
/// A well-formed id no session has.
pub(crate) const GONE: &str = "ses_fx999900aaaaaaaaaaaaaaaaaa";

pub(crate) fn words() -> String {
    format!(
        "The review ran in {CHILD} and came back clean; its notes are in `{CHILD}`. The first attempt, {GONE}, was deleted.\n\nTo keep its transcript:\n\n```sh\nopencode export {CHILD}\n```\n"
    )
}

/// The open session with Kit's question and the finished reply.
pub(crate) fn scene(s: &mut App) {
    crate::scenes::open_scene(s);
    s.finish_stream();
    let id = sid(s);
    if let Some(t) = s.transcripts.get_mut(&id) {
        t.messages.push(Message::User { id: ASK.into(), text: "Did the review pass?".into() });
    }
    crate::genui::demo::stream(s, REPLY, &words(), 0.0);
    while let Some((_, ty, data)) = s.playback.pop_front() {
        s.apply_event(&ty, &data);
    }
}

fn open(s: &mut App, tx: &mut Play<App>) {
    tx.instant();
    scene(s);
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Session chips", &[replay(open)]),
]);

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [("session-chips", scene)]);

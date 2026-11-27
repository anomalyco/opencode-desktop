//! Composer › Queue row width: a busy session with three queued prompts whose words test where a
//! row's text ends. The first fits the row at rest but not beside the hovered actions, the second
//! runs past the row, and the third carries two attachments after its words.
//!
//! Composer › Queue row sending: the same rows with the first one held as it is while the server
//! has not answered a change to it: its words faint, the ring in place of its actions.

use std::sync::Arc;

use opencode_kit::model::{Attachment, Delivery, InboxItem};
use rune::Play;

use super::{QueueHost, base_rows, machine, rows};
use crate::App;
use crate::stories::{Trigger, replay, sid};

const ROWS: [(&str, usize); 3] = [
    ("Then write up what changed in the PR description and link both follow-up issues in it too", 0),
    ("Reminder: timer set 30m ago for Sun, Nov 15 at 9:13 PM. Next: a rolling review of the queue, the composer and the timeline rows, then a summary", 0),
    ("Compare these two screenshots and tell me which spacing reads better", 2),
];

const KEYS: &[Trigger] = &[replay(open)];
const SENDING_KEYS: &[Trigger] = &[replay(open_sending)];

fn open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    let id = sid(s);
    s.running.insert(id.clone());
    s.queue_ops_mut().remove(&id);
    let inbox = s.inboxes.entry(id).or_default();
    for (k, (text, files)) in ROWS.into_iter().enumerate() {
        let files = (0..files).map(|n| Attachment { mime: "image/png".into(), name: format!("shot-{n}.png"), bytes: Arc::from(Vec::new()), media: None }).collect();
        inbox.upsert(InboxItem { id: format!("msg_zzwidth{k}"), text: text.into(), delivery: Delivery::Queue, files });
    }
}

/// The first row's Undo is asked for and its write never leaves, as if the server held it, so the
/// row stays pending. (A fixture run answers a change in the step that asks for it.)
fn open_sending(s: &mut App, tx: &mut Play<App>) {
    open(s, tx);
    let id = sid(s);
    hold_undo(s, &id, "msg_zzwidth0", ROWS[0].0);
}

/// Asks for the Undo of `session`'s queued prompt `id` and drops the write the machine hands back.
fn hold_undo(s: &mut App, session: &str, id: &str, text: &str) {
    let (current, base, now) = (rows(&*s, session), base_rows(&*s, session), *s.clock);
    let undo = machine::Event::UndoRequested { id: id.into(), text: text.into(), files: Vec::new(), shell: false, now };
    let _ = s.queue_ops_mut().entry(session.to_owned()).or_default().step(undo, &current, &base);
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Composer/Queue row width", KEYS),
    crate::stories::Story::session("Composer/Queue row sending", SENDING_KEYS),
]);

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    // The `queue-3` scene with its first row held while its change is sent (a story's stage is
    // empty in a run without the storybook, so this is the one a headless release run can open).
    ("queue-sending", |s| {
        super::scene(s, 3, false);
        let crate::Route::Session { id, .. } = (*s.route).clone() else { return };
        hold_undo(s, &id, "msg_zzqueued0", "Then write up what changed in the PR description");
    }),
]);
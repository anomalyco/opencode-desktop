//! Composer › Queue row width: a busy session with three queued prompts whose words test where a
//! row's text ends. The first fits the row at rest but not beside the hovered actions, the second
//! runs past the row, and the third carries two attachments after its words.

use std::sync::Arc;

use opencode_kit::model::{Attachment, Delivery, InboxItem};
use rune::Play;

use crate::App;
use crate::stories::{Trigger, replay, sid};

const ROWS: [(&str, usize); 3] = [
    ("Then write up what changed in the PR description and link both follow-up issues in it too", 0),
    ("Reminder: timer set 30m ago for Sun, Nov 15 at 9:13 PM. Next: a rolling review of the queue, the composer and the timeline rows, then a summary", 0),
    ("Compare these two screenshots and tell me which spacing reads better", 2),
];

const KEYS: &[Trigger] = &[replay(open)];

fn open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    let id = sid(s);
    s.running.insert(id.clone());
    let inbox = s.inboxes.entry(id).or_default();
    for (k, (text, files)) in ROWS.into_iter().enumerate() {
        let files = (0..files).map(|n| Attachment { mime: "image/png".into(), name: format!("shot-{n}.png"), bytes: Arc::from(Vec::new()), media: None }).collect();
        inbox.upsert(InboxItem { id: format!("msg_zzwidth{k}"), text: text.into(), delivery: Delivery::Queue, files });
    }
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Composer/Queue row width", KEYS),
]);

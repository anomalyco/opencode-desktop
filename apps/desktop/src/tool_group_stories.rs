//! The storybook's Long tool group story (Timeline › Long tool group): a turn that used 40
//! tools, its group near the timeline's end as in Kit's capture (t988). O opens or shuts it with
//! its header held where it is; S scrolls into the open group so its header sticks to the top;
//! R replays open, scroll, then shut from the stuck header.

use opencode_kit::model::{Assistant, Message, Part, ToolStatus};
use rune::{Play, Tx};

use crate::App;
use crate::stories::{Trigger, replay, sid};

pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'o', label: "Open or shut the group", run: |s, tx| flip(s, tx) },
    Trigger { key: 's', label: "Scroll into the group", run: |s, tx| scroll(s, tx) },
];

/// The group's row key (`turns::fold` keys a group by its first tool's id).
pub const GROUP: &str = "c-call_fold_00";
const TOOLS: usize = 40;

/// Ends session `session` with a prompt, a turn that used [`TOOLS`] tools, and its answer.
pub fn add_long_group(s: &mut App, session: &str) {
    let Some(t) = s.transcripts.get_mut(session) else { return };
    let names = ["read", "grep", "shell", "glob", "read", "execute", "skill"];
    let parts = (0..TOOLS)
        .map(|k| {
            let name = names[k % names.len()];
            Part::Tool {
                id: format!("call_fold_{k:02}"),
                name: name.into(),
                status: ToolStatus::Completed,
                title: format!("src/net/stream_{k:02}.rs"),
                input: serde_json::Value::Null,
                files: vec![],
                metadata: serde_json::Value::Null,
            }
        })
        .collect();
    t.messages.push(Message::User { id: "msg_zzfold0".into(), text: "Look through the reconnect code again before you merge.".into() });
    t.messages.push(Message::Assistant(Assistant { id: "msg_zzfold1".into(), completed: Some(1), parts, ..Default::default() }));
    let answer = vec![Part::Text { text: "Read it all again: the watchdog and the backoff hold up.".into() }];
    t.messages.push(Message::Assistant(Assistant { id: "msg_zzfold2".into(), completed: Some(2), parts: answer, ..Default::default() }));
}

fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    let id = sid(s);
    add_long_group(s, &id);
    tx.after(0.6, |s, tx| flip(s, tx));
    tx.after(1.6, |s, tx| scroll(s, tx));
    tx.after(2.8, |s, tx| flip(s, tx));
}

/// Opens or shuts the group, as a press on its header does.
fn flip(s: &mut App, tx: &mut Tx) {
    crate::disclosure::toggle_group(GROUP.into())(s, tx);
}

/// Scrolls the open group's twentieth step to the middle of the timeline, under its stuck header.
fn scroll(s: &mut App, tx: &mut Tx) {
    if s.expanded.contains(GROUP) {
        tx.scroll_key_to("tool-call_fold_20", 160.0);
    }
}

const KNOBS: &[rune_knobs::Group] = &[rune_knobs::Group::new("Tool group", "stage", &[crate::disclosure::FOLD_SPEED])];

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Long tool group", KEYS).knobs(KNOBS),
]);

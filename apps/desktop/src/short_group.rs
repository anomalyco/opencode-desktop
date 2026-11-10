//! Kit's capture of a short tool group shutting (t1107): one thought and one shell above five
//! prompts sent while it ran, and the next step still thinking. `--scene short-group`.

use opencode_kit::model::{Assistant, Message, Part, ToolStatus};
use serde_json::json;

use crate::App;

/// The group Kit pressed.
#[cfg(test)]
pub const GROUP: &str = "c-msg_zzshort1-0";

/// Kit's capture: a turn of one thought and one shell, five prompts queued under it, and the
/// next step still thinking.
pub fn add_short_group(app: &mut App, sid: &str) {
    let t = app.transcripts.get_mut(sid).expect("session");
    t.messages.push(Message::User { id: "msg_zzshort0".into(), text: "Look at the Scrawl".into() });
    let parts = vec![
        Part::Reasoning { text: "Reading the capture first.".into(), started: 0, completed: Some(15_000) },
        Part::Tool { id: "call_zzshort".into(), name: "shell".into(), title: "ls captures".into(), status: ToolStatus::Completed, input: json!({}), files: vec![], metadata: json!(null) },
    ];
    t.messages.push(Message::Assistant(Assistant { id: "msg_zzshort1".into(), completed: Some(1), parts, ..Default::default() }));
    for (k, w) in ["weird animations", "when pasting", "and then it coming in", "with truncated item...", "bad"].iter().enumerate() {
        t.messages.push(Message::User { id: format!("msg_zzshort2{k}"), text: (*w).into() });
    }
    let thinking = vec![Part::Reasoning { text: "Looking".into(), started: 0, completed: None }];
    t.messages.push(Message::Assistant(Assistant { id: "msg_zzshort3".into(), parts: thinking, ..Default::default() }));
    app.running.insert(sid.into());
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("short-group", |s| {
        crate::open_scene(s);
        s.finish_stream();
        if let crate::Route::Session { id, .. } = s.route.clone() {
            add_short_group(s, &id);
        }
    }),
]);

//! Kit's capture of a short tool group shutting (t1107): one thought and one shell above five
//! prompts sent while it ran, and the next step still thinking. `--scene short-group`.

use std::collections::HashSet;

use opencode_kit::model::{Assistant, Message, Part, ToolStatus, Transcript};
use serde_json::json;

/// Messages for the short-group capture: one thought and one shell, five queued prompts, and a
/// still-thinking next step.
pub fn messages() -> Vec<Message> {
    let parts = vec![
        Part::Reasoning { text: "Reading the capture first.".into(), started: 0, completed: Some(15_000) },
        Part::Tool {
            id: "call_zzshort".into(),
            name: "shell".into(),
            title: "ls captures".into(),
            status: ToolStatus::Completed,
            input: json!({}),
            files: vec![],
            metadata: json!(null),
        },
    ];
    let mut out = vec![
        Message::User { id: "msg_zzshort0".into(), text: "Look at the Scrawl".into() },
        Message::Assistant(Assistant { id: "msg_zzshort1".into(), completed: Some(1), parts, ..Default::default() }),
    ];
    for (k, w) in ["weird animations", "when pasting", "and then it coming in", "with truncated item...", "bad"].iter().enumerate() {
        out.push(Message::User { id: format!("msg_zzshort2{k}"), text: (*w).into() });
    }
    let thinking = vec![Part::Reasoning { text: "Looking".into(), started: 0, completed: None }];
    out.push(Message::Assistant(Assistant { id: "msg_zzshort3".into(), parts: thinking, ..Default::default() }));
    out
}

/// Appends the short-group capture to `transcript` and marks `sid` running.
pub fn apply(transcript: &mut Transcript, running: &mut HashSet<String>, sid: &str) {
    transcript.messages.extend(messages());
    running.insert(sid.to_owned());
}

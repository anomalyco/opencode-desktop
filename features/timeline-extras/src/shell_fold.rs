//! Kit's capture of a tool group that opened out of sight (p277): a reply whose first part is
//! one shell call, then a long answer, the session still working. `--scene shell-fold`.

use std::collections::HashSet;

use opencode_kit::model::{Assistant, Message, Part, ToolStatus, Transcript};
use serde_json::json;

/// What the shell printed, so pressed open it shows a console as in Kit's capture (t1484).
pub const LOG: &str = "4f1c2ab Fold tool rows open on the group's spring\n9e03d71 Hold a pressed header where it shows\n31b8c0e Keep the console's output capped\nd2a6f94 Name the build machine in the answer\n7c5e019 Start the three programs";

/// Messages for the shell-fold capture: one shell call, a long answer, and a still-thinking step.
pub fn messages() -> Vec<Message> {
    let answer = (1..=12)
        .map(|n| format!("Program {n} is a small thing to run in your terminal, built on a build machine from the latest `next`, so we can look at it together."))
        .collect::<Vec<_>>()
        .join("\n\n");
    let parts = vec![
        Part::Tool {
            id: "call_zzshellfold".into(),
            name: "shell".into(),
            title: "git log --oneline -5".into(),
            status: ToolStatus::Completed,
            input: json!({ "command": "git log --oneline -5" }),
            files: vec![],
            metadata: json!({ "output": LOG }),
        },
        Part::Text { text: format!("I'm building three things for you to run in your terminal.\n\n{answer}") },
    ];
    let thinking = vec![Part::Reasoning { text: "Waiting on the builds".into(), started: 0, completed: None }];
    vec![
        Message::User { id: "msg_zzshellfold0".into(), text: "Build me some programs to look at".into() },
        Message::Assistant(Assistant { id: "msg_zzshellfold1".into(), completed: Some(1), parts, ..Default::default() }),
        Message::Assistant(Assistant { id: "msg_zzshellfold2".into(), parts: thinking, ..Default::default() }),
    ]
}

/// Appends the shell-fold turn to `transcript` and marks `sid` running.
pub fn apply(transcript: &mut Transcript, running: &mut HashSet<String>, sid: &str) {
    transcript.messages.extend(messages());
    running.insert(sid.to_owned());
}

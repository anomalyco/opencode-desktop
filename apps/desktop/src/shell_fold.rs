//! Kit's capture of a tool group that opened out of sight (p277): a reply whose first part is
//! one shell call, then a long answer, the session still working. `--scene shell-fold`.

use opencode_kit::model::{Assistant, Message, Part, ToolStatus};
use serde_json::json;

use crate::App;

/// What the shell printed, so pressed open it shows a console as in Kit's capture (t1484).
const LOG: &str = "4f1c2ab Fold tool rows open on the group's spring\n9e03d71 Hold a pressed header where it shows\n31b8c0e Keep the console's output capped\nd2a6f94 Name the build machine in the answer\n7c5e019 Start the three programs";

/// A turn of one shell call and a long answer, the next step still thinking.
pub fn add_shell_fold(app: &mut App, sid: &str) {
    let t = app.transcripts.get_mut(sid).expect("session");
    t.messages.push(Message::User { id: "msg_zzshellfold0".into(), text: "Build me some programs to look at".into() });
    let answer = (1..=12)
        .map(|n| format!("Program {n} is a small thing to run in your terminal, built on a build machine from the latest `next`, so we can look at it together."))
        .collect::<Vec<_>>()
        .join("\n\n");
    let parts = vec![
        Part::Tool { id: "call_zzshellfold".into(), name: "shell".into(), title: "git log --oneline -5".into(), status: ToolStatus::Completed, input: json!({ "command": "git log --oneline -5" }), files: vec![], metadata: json!({ "output": LOG }) },
        Part::Text { text: format!("I'm building three things for you to run in your terminal.\n\n{answer}") },
    ];
    t.messages.push(Message::Assistant(Assistant { id: "msg_zzshellfold1".into(), completed: Some(1), parts, ..Default::default() }));
    let thinking = vec![Part::Reasoning { text: "Waiting on the builds".into(), started: 0, completed: None }];
    t.messages.push(Message::Assistant(Assistant { id: "msg_zzshellfold2".into(), parts: thinking, ..Default::default() }));
    app.running.insert(sid.into());
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("shell-fold", |s| {
        crate::open_scene(s);
        s.finish_stream();
        if let crate::Route::Session { id, .. } = (*s.route).clone() {
            add_shell_fold(s, &id);
        }
    }),
]);

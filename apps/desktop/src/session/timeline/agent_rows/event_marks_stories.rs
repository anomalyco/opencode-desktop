//! The storybook's Event marks story (Timeline › Event marks, t1582): every state a subagent,
//! background shell or session notice can show, at rest, as bare marks. Subagents: started,
//! updated, failed, stopped and one with no session to open, then their reports. Shells: started,
//! ran, failed, a group with one failure and one where all failed, then their reports, one a
//! command over three lines. Notices:
//! instructions, a restart, a job and a plain notice beside the dividers they share a timeline
//! with. Rows that open something take a rounded fill under the pointer; the rest take none.

use opencode_kit::model::{Assistant, Finished, Message, ModelRef, Notice, NoticeKind, Outcome, Part, SessionInfo, ToolStatus, tool_error_metadata};
use rune::Play;
use serde_json::{Value, json};

use crate::App;
use crate::stories::{Trigger, replay, sid};

pub const VARIANTS: &[(&str, usize)] = &[("Subagents", 0), ("Shells", 1), ("Notices", 2)];
pub const KEYS: &[Trigger] = &[replay(open)];

fn child(k: usize) -> String {
    format!("ses_story_mark_{k:02}")
}

fn text(t: &str) -> Part {
    Part::Text { text: t.into() }
}

fn tool(k: usize, name: &str, status: ToolStatus, input: Value, metadata: Value) -> Part {
    Part::Tool { id: format!("call_story_mark_{k:02}"), name: name.into(), status, title: String::new(), input, files: Vec::new(), metadata }
}

fn failed(message: &str, mut extra: Value) -> Value {
    let mut m = tool_error_metadata(message);
    if let (Some(m), Some(extra)) = (m.as_object_mut(), extra.as_object_mut()) {
        m.append(extra);
    }
    m
}

fn finished(agent: Option<&str>, outcome: Outcome, data: &str, shell: Option<&str>, child: Option<String>) -> NoticeKind {
    NoticeKind::Finished(Finished { agent: agent.map(Into::into), subagent: agent.is_some(), outcome, data: Some(data.into()), required: outcome == Outcome::Error, shell: shell.map(Into::into), child })
}

fn subagents(s: &mut App, parent: &str) -> (Vec<Part>, Vec<NoticeKind>) {
    let from = s.session(parent).cloned().unwrap_or_default();
    let tasks = ["Audit print invariants", "Trace field order", "Build the sample project", "Repeat the full audit"];
    for (k, title) in tasks.iter().enumerate() {
        s.sessions.push(SessionInfo { id: child(k), parent: Some(parent.into()), project: from.project.clone(), directory: from.directory.clone(), title: (*title).into(), agent: "general".into(), ..Default::default() });
    }
    let call = |k: usize, agent: &str, title: &str, status, metadata: Value| tool(k, "subagent", status, json!({ "agent": agent, "description": title, "background": true }), metadata);
    let parts = vec![
        text("The parser and printer agree on the order. One question remains: does the generated code preserve it?"),
        call(0, "general", tasks[0], ToolStatus::Completed, json!({ "sessionID": child(0), "status": "running" })),
        tool(1, "subagent", ToolStatus::Completed, json!({ "agent": "general", "description": "Check the remaining decoders", "sessionID": child(0) }), json!({ "sessionID": child(0), "status": "running" })),
        text("The audit found one case worth a closer look."),
        call(2, "explore", tasks[1], ToolStatus::Completed, json!({ "sessionID": child(1), "status": "running" })),
        call(3, "general", tasks[2], ToolStatus::Error, failed("The provider is rate limiting this key", json!({ "sessionID": child(2) }))),
        call(4, "general", tasks[3], ToolStatus::Error, failed("Command cancelled", json!({ "sessionID": child(3) }))),
        call(5, "explore", "Summarize the decoder notes", ToolStatus::Completed, json!({ "status": "running" })),
    ];
    let notices = vec![
        finished(Some("explore"), Outcome::Completed, tasks[1], None, Some(child(1))),
        finished(Some("general"), Outcome::Error, tasks[2], None, Some(child(2))),
        finished(Some("general"), Outcome::Cancelled, tasks[3], None, Some(child(3))),
    ];
    (parts, notices)
}

fn shells() -> (Vec<Part>, Vec<NoticeKind>) {
    let bg = |k: usize, command: &str| tool(k, "shell", ToolStatus::Completed, json!({ "command": command, "background": true }), json!({ "status": "running", "shellID": format!("sh_mark_{k}") }));
    let ran = |k: usize, command: &str| tool(k, "shell", ToolStatus::Completed, json!({ "command": command }), json!({ "exit": 0 }));
    let fail = |k: usize, command: &str| tool(k, "shell", ToolStatus::Error, json!({ "command": command }), failed("exit 1", json!({ "exit": 1 })));
    let parts = vec![
        text("Two commands run in the background while the agent checks the output."),
        bg(0, "make build"),
        bg(1, "make format"),
        text("One check ran in the foreground."),
        ran(2, "make check-types"),
        text("Then the example failed to compile."),
        fail(3, "make example"),
        text("Grouped: two of three succeeded."),
        ran(4, "make format"),
        ran(5, "make manual"),
        fail(6, "make example"),
        text("Grouped: both failed."),
        fail(7, "make test-a"),
        fail(8, "make test-b"),
    ];
    let notices = vec![
        // A command over three lines, as an agent's scripted shell sends it: its row shows one.
        finished(None, Outcome::Completed, "O=~/tools/ops; M=\"Commits to src/ and seed/ are open again\"\nfor s in ses_a ses_b ses_c; do ops send \"$s\" \"$M\"; done\nskulk in 2h --name clean-swarm --do \"Run the cleanliness sweep\"", Some("sh_mark_0"), None),
        finished(None, Outcome::Error, "make format", Some("sh_mark_1"), None),
        finished(None, Outcome::Cancelled, "make watch", Some("sh_mark_9"), None),
    ];
    (parts, notices)
}

fn notices() -> (Vec<Part>, Vec<NoticeKind>) {
    let model = |id: &str, variant: &str| ModelRef { provider: "anthropic".into(), id: id.into(), variant: variant.into() };
    let parts = vec![text("These are session notices, not links: the pointer adds nothing over them.")];
    let notices = vec![
        NoticeKind::Agent { agent: "plan".into(), previous: Some("build".into()) },
        NoticeKind::Model { model: model("claude-opus-5-5", "high"), previous: Some(model("claude-sonnet-5-5", "")) },
        NoticeKind::Moved { directory: "examples/parser".into() },
        NoticeKind::Instructions { sources: vec!["AGENTS.md".into()] },
        NoticeKind::Restart,
        NoticeKind::Job { id: "cm_story_done".into(), description: "Script".into(), script: Some(Outcome::Completed), required: false },
        NoticeKind::Job { id: "cm_story_fail".into(), description: "Script".into(), script: Some(Outcome::Error), required: true },
        NoticeKind::Job { id: "cm_story_stop".into(), description: "Script".into(), script: Some(Outcome::Cancelled), required: false },
        NoticeKind::Job { id: "job_story".into(), description: "Background job reported progress".into(), script: None, required: false },
        NoticeKind::Text("Session notice".into()),
    ];
    (parts, notices)
}

fn open(s: &mut App, _tx: &mut Play<App>) {
    s.finish_stream();
    let parent = sid(s);
    let (parts, kinds) = match *s.fixture_run.variant {
        0 => subagents(s, &parent),
        1 => shells(),
        _ => notices(),
    };
    let answer = Assistant { id: "msg_story_marks".into(), completed: Some(*s.now_ms), parts, ..Default::default() };
    let Some(t) = s.transcripts.get_mut(&parent) else { return };
    t.messages.push(Message::Assistant(answer));
    for (k, kind) in kinds.into_iter().enumerate() {
        t.messages.push(Message::Notice(Notice { id: format!("msg_story_mark_notice_{k:02}"), kind }));
    }
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Timeline/Event marks", KEYS).variants(VARIANTS),
]);

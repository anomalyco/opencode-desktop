//! The storybook's Todo pill story (Composer › Todo pill): the agent works through a six-step
//! plan, the pill naming the todo in hand and its list open over the composer. O opens and
//! closes the list, N moves the plan on a step (its tick grows in), T runs two at once, S starts thirty subagents
//! beside it, F finishes the plan (the pill goes), R replays.

use opencode_kit::model::{Assistant, Message, Part, ToolStatus};
use rune::Play;
use serde_json::json;

use crate::App;
use crate::stories::{Trigger, replay, sid};

pub const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'o', label: "Open or close the list", run: |s, _| s.todo_open = !s.todo_open },
    Trigger { key: 'n', label: "Next todo", run: |s, _| advance(s) },
    Trigger { key: 't', label: "Two in progress", run: |s, _| two_at_once(s) },
    Trigger { key: 's', label: "Thirty subagents beside it", run: |s, _| crate::capsule_stories::run_thirty(s) },
    Trigger { key: 'f', label: "Finish the plan", run: |s, _| set_step(s, PLAN.len()) },
];

pub const PLAN: &[&str] = &[
    "Read the old loader and list every key it accepts",
    "Write the new schema with defaults",
    "Map old keys to new ones in a migration step, keeping unknown keys as warnings instead of errors",
    "Keep reading the old file for one release",
    "Update the loader tests for both shapes",
    "Note the change in the changelog",
];

const PART: &str = "call_story_todowrite";

/// Two done, one in progress, the changelog dropped, and the list open.
fn open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    let id = sid(s);    s.running.insert(id);
    set_step(s, 2);
    s.todo_open = true;
}

/// The plan with the first `step` todos done (the changelog cancelled once it's reached) and the
/// next one in progress, as the session's latest `todowrite` call.
pub fn set_step(s: &mut App, step: usize) {
    let todos: Vec<_> = PLAN
        .iter()
        .enumerate()
        .map(|(k, c)| {
            let status = match k {
                5 if step >= 2 => "cancelled",
                k if k < step => "completed",
                k if k == step => "in_progress",
                _ => "pending",
            };
            json!({ "content": c, "status": status })
        })
        .collect();
    set_plan(s, json!({ "todos": todos }));
}

fn advance(s: &mut App) {
    let id = sid(s);
    let todos = s.transcripts.get(&id).map(|t| crate::todos::session_todos(t, None)).unwrap_or_default();
    let at = todos.iter().position(|t| t.status == crate::todos::Status::InProgress).unwrap_or(PLAN.len());
    set_step(s, (at + 1).min(PLAN.len()));
}

fn two_at_once(s: &mut App) {
    let todos: Vec<_> = PLAN
        .iter()
        .enumerate()
        .map(|(k, c)| json!({ "content": c, "status": match k { 0 | 1 => "completed", 2 | 3 => "in_progress", _ => "pending" } }))
        .collect();
    set_plan(s, json!({ "todos": todos }));
}

/// Puts `input` on the open session as its latest finished `todowrite` call.
pub fn set_plan(s: &mut App, input: serde_json::Value) {
    let id = sid(s);
    let Some(t) = s.transcripts.get_mut(&id) else { return };
    let call = Part::Tool { id: PART.into(), name: "todowrite".into(), status: ToolStatus::Completed, title: String::new(), input, files: Vec::new(), metadata: json!({}) };
    for m in &mut t.messages {
        if let Message::Assistant(a) = m {
            a.parts.retain(|p| !matches!(p, Part::Tool { id, .. } if id == PART));
        }
    }
    match t.messages.iter_mut().rev().find_map(|m| if let Message::Assistant(a) = m { Some(a) } else { None }) {
        Some(a) => a.parts.push(call),
        None => t.messages.push(Message::Assistant(Assistant { id: "msg_story_todos".into(), completed: None, parts: vec![call], ..Default::default() })),
    }
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story::session("Composer/Todo pill", KEYS).knobs(&[crate::todos::GROUP, crate::popover::motion::GROUP]),
]);

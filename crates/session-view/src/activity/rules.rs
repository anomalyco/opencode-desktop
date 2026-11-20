//! What the timeline shows of a call, where, and how: Desktop's New timeline's rules
//! (`session-ui/src/timeline-next/rules.ts`). Each tool has a category (the Timeline detail
//! setting that places it), a mode (what the agent was doing, which names its group: Explored,
//! Edited, Ran, Used) and a layout (a line of activity, a file, or work that runs on).

use opencode_kit::model::ToolStatus;
use serde_json::Value;

use crate::detail::Category;

/// What the agent was doing with a call (`ToolMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Explore,
    Edit,
    Run,
    Tool,
}

/// How a call shows when it stands on its own (`ToolLayout`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// One line of activity.
    Line,
    /// A file's name and changes, opening to its diff.
    File,
    /// Work that runs on: a subagent, or a shell started in the background. Started together,
    /// such calls count in one events row ("Started 2 subagents", `rules.ts:22-23`).
    Event,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    pub name: &'static str,
    pub category: Category,
    pub mode: Mode,
    pub layout: Layout,
    /// Shows only once settled, and never folds into a group (a question).
    pub alone: bool,
    /// Never shows (to-dos).
    pub hidden: bool,
}

/// A tool's current name: sessions migrated from v1 keep the old ones (`renamed`).
pub fn current_name(name: &str) -> &str {
    match name {
        "bash" => "shell",
        "task" => "subagent",
        "apply_patch" | "multiedit" => "patch",
        other => other,
    }
}

/// Call `name`'s rule (`toolRule`). A shell the agent started in the background reports back
/// like a subagent, so it lays out as an event.
pub fn rule(name: &str, input: &Value) -> Rule {
    let name = current_name(name);
    let r = |name: &'static str, category, mode, layout| Rule { name, category, mode, layout, alone: false, hidden: false };
    let explore = |n| r(n, Category::Tools, Mode::Explore, Layout::Line);
    let file = |n| r(n, Category::Edit, Mode::Edit, Layout::File);
    match name {
        "read" => explore("read"),
        "list" => explore("list"),
        "glob" => explore("glob"),
        "grep" => explore("grep"),
        "webfetch" => explore("webfetch"),
        "websearch" => explore("websearch"),
        "skill" => explore("skill"),
        "edit" => file("edit"),
        "write" => file("write"),
        "patch" => file("patch"),
        "shell" if input["background"] == true => r("shell", Category::Shell, Mode::Run, Layout::Event),
        "shell" => r("shell", Category::Shell, Mode::Run, Layout::Line),
        "execute" => r("execute", Category::Shell, Mode::Run, Layout::Line),
        "subagent" => r("subagent", Category::Subagents, Mode::Tool, Layout::Event),
        "question" => Rule { alone: true, ..r("question", Category::Tools, Mode::Tool, Layout::Line) },
        "todowrite" => Rule { hidden: true, ..r("todowrite", Category::Tools, Mode::Tool, Layout::Line) },
        _ => r("", Category::Tools, Mode::Tool, Layout::Line),
    }
}

/// Whether the user stopped a call, by stopping the turn or cancelling its command
/// (`toolStopped`): the shell reports a cancelled command as "Command cancelled".
pub fn stopped(status: &ToolStatus, metadata: &Value) -> bool {
    if *status != ToolStatus::Error {
        return false;
    }
    let message = opencode_kit::model::tool_error(metadata).unwrap_or("");
    message == "Command cancelled" || crate::timeline::interrupted(message)
}

/// The changes a script made to files, in order, one per change (`scriptChanges`,
/// `session-ui/src/timeline/facts.ts`): its `metadata.files` while it runs and once it ends in the
/// foreground. A script that failed or was stopped keeps the changes it made, which really
/// happened; one moved to the background, or run as a job, has none. Until a script finishes, its
/// `background` flag says only that it can move there. Entries that don't decode are dropped
/// here, so every reader numbers the changes alike.
pub fn script_changes<'a>(name: &str, status: &ToolStatus, metadata: &'a Value) -> Vec<&'a Value> {
    if name != "execute" || *status == ToolStatus::Pending {
        return Vec::new();
    }
    if metadata.get("jobID").is_some() || (*status == ToolStatus::Completed && metadata["background"] == true) {
        return Vec::new();
    }
    metadata["files"].as_array().map(|a| a.iter().filter(|f| decodes(f)).collect()).unwrap_or_default()
}

/// `{file, additions?, deletions?, status?: added|deleted|modified, patch?}`, with a path.
fn decodes(f: &Value) -> bool {
    let Some(o) = f.as_object() else { return false };
    let optional = |k: &str, ok: fn(&Value) -> bool| o.get(k).is_none_or(ok);
    o.get("file").and_then(Value::as_str).is_some_and(|p| !p.is_empty())
        && optional("additions", Value::is_number)
        && optional("deletions", Value::is_number)
        && optional("status", |v| matches!(v.as_str(), Some("added" | "deleted" | "modified")))
        && optional("patch", Value::is_string)
}

/// Whether a call failed (`toolFailed`): a call the user stopped did not.
pub fn failed(name: &str, status: &ToolStatus, metadata: &Value) -> bool {
    !stopped(status, metadata) && crate::detail::failed(current_name(name), status, metadata)
}

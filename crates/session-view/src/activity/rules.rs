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
/// foreground, each with its offset in that array, which only grows, so it names the change for
/// good. A script that failed or was stopped keeps the changes it made, which really happened;
/// one moved to the background, or run as a job, has none. Until a script finishes, its
/// `background` flag says only that it can move there. Entries that don't decode are dropped.
pub fn script_changes<'a>(name: &str, status: &ToolStatus, metadata: &'a Value) -> impl Iterator<Item = (usize, &'a Value)> + 'a {
    let shown = shows_changes(name, status, metadata);
    metadata["files"].as_array().filter(|_| shown).into_iter().flatten().enumerate().filter(|(_, f)| decodes(f))
}

/// The change at offset `raw` of a script's `metadata.files`, where [`script_changes`] lists it.
pub fn script_change_at<'a>(name: &str, status: &ToolStatus, metadata: &'a Value, raw: usize) -> Option<&'a Value> {
    metadata["files"].get(raw).filter(|f| shows_changes(name, status, metadata) && decodes(f))
}

fn shows_changes(name: &str, status: &ToolStatus, metadata: &Value) -> bool {
    name == "execute"
        && *status != ToolStatus::Pending
        && metadata.get("jobID").is_none()
        && !(*status == ToolStatus::Completed && metadata["background"] == true)
}

/// A change's line count (`additions`, `deletions`) as a whole number: `1.0` is 1, and anything
/// that isn't a count is 0.
pub fn count(v: &Value) -> u64 {
    v.as_u64().or_else(|| v.as_f64().filter(|f| f.is_finite() && *f >= 0.0).map(|f| f as u64)).unwrap_or(0)
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

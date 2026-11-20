//! Lenient JSON parsing for projects, sessions, message parts, and transcript pages.

use serde_json::Value;

use super::{
    Assistant, Compaction, Message, ModelRef, Part, Project, PromptComment, Retry, SessionInfo,
    StepError, Tokens, ToolStatus, i, keep_tool_error, keep_tool_output, keep_tool_ran, model_of,
    notice, parse_attachments, s, set_user_comments, set_user_created, set_user_files, strings,
    tool,
};

pub fn parse_projects(v: &Value) -> Vec<Project> {
    let list = v.as_array().or_else(|| v["data"].as_array());
    list.into_iter().flatten().filter_map(crate::project::parse_project).collect()
}

pub fn parse_session(p: &Value) -> SessionInfo {
    SessionInfo {
        id: s(&p["id"]),
        parent: p["parentID"].as_str().map(str::to_owned),
        project: s(&p["projectID"]),
        title: s(&p["title"]),
        directory: s(&p["location"]["directory"]),
        updated: i(&p["time"]["updated"]),
        model: ModelRef::parse(&p["model"]),
        cost: p["cost"].as_f64().unwrap_or(0.0),
        idle: p["time"]["idle"].as_i64(),
        viewed: p["time"]["viewed"].as_i64(),
        busy: p["time"]["busy"].as_i64(),
        agent: s(&p["agent"]),
        created: i(&p["time"]["created"]),
        outcome: p["outcome"].as_str().map(str::to_owned),
        revert: p["revert"]["messageID"].as_str().map(str::to_owned),
        archived: p["time"]["archived"].as_i64(),
        metadata: p["metadata"].clone(),
    }
}

pub fn parse_sessions(v: &Value) -> Vec<SessionInfo> {
    v["data"].as_array().or_else(|| v.as_array()).into_iter().flatten().map(parse_session).collect()
}

pub(super) fn tool_title(name: &str, input: &Value) -> String {
    let get = |k: &str| input[k].as_str().map(str::to_owned);
    let short = |p: String| if p.split(crate::path::SEPARATORS).count() > 3 { crate::path::tail(&p, 2).to_owned() } else { p };
    let t = match name {
        "shell" | "bash" => get("description").or_else(|| get("command")),
        "read" | "write" | "edit" => get("filePath").or_else(|| get("path")).map(short),
        "grep" | "glob" => get("pattern"),
        "task" | "subagent" => get("description").or_else(|| get("prompt")),
        "webfetch" => get("url"),
        "skill" => get("name").or_else(|| get("id")),
        _ => None,
    };
    let t = t.unwrap_or_default();
    let line = t.lines().next().unwrap_or("").trim().to_owned();
    if line.chars().count() > 120 { line.chars().take(119).collect::<String>() + "…" } else { line }
}

fn tool_status(v: &Value) -> ToolStatus {
    match v.as_str().unwrap_or("") {
        "completed" => ToolStatus::Completed,
        "error" | "failed" => ToolStatus::Error,
        "running" => ToolStatus::Running,
        _ => ToolStatus::Pending,
    }
}

fn parse_part(c: &Value) -> Option<Part> {
    Some(match c["type"].as_str()? {
        "text" => Part::Text { text: s(&c["text"]) },
        "reasoning" => Part::Reasoning {
            text: s(&c["text"]),
            started: i(&c["time"]["created"]),
            completed: c["time"]["completed"].as_i64(),
        },
        "tool" => {
            let name = s(&c["name"]);
            let input = c["state"]["input"].clone();
            let mut metadata = c["state"]["metadata"].clone();
            keep_tool_error(&mut metadata, &c["state"]["error"]);
            keep_tool_output(&mut metadata, &c["state"]["content"]);
            keep_tool_ran(&mut metadata, &c["time"]);
            Part::Tool {
                id: s(&c["id"]),
                title: tool_title(&name, &input),
                name,
                status: tool_status(&c["state"]["status"]),
                input,
                files: parse_attachments(&c["state"]["content"]),
                metadata,
            }
        }
        _ => return None,
    })
}

pub fn parse_message(m: &Value) -> Option<Message> {
    let id = s(&m["id"]);
    Some(match m["type"].as_str()? {
        "user" => {
            let files = parse_attachments(&m["files"]);
            if !files.is_empty() {
                set_user_files(&id, files);
            }
            let text = s(&m["text"]);
            crate::mention::record(&id, &text, m);
            if let Some(at) = m["time"]["created"].as_i64() {
                set_user_created(&id, at);
            }
            Message::User { text: quoted_text(&id, text, &m["metadata"]), id }
        }
        "assistant" => {
            let mut parts = Vec::new();
            let mut text_phases = Vec::new();
            for c in m["content"].as_array().into_iter().flatten() {
                if let Some(part) = parse_part(c) {
                    if matches!(part, Part::Text { .. })
                        && let Some(phase) = super::TextPhase::parse(&c["native"]["phase"])
                    {
                        text_phases.push((parts.len(), phase));
                    }
                    parts.push(part);
                }
            }
            Message::Assistant(Assistant {
                id,
                completed: m["time"]["completed"].as_i64(),
                parts,
                text_phases,
                cost: m["cost"].as_f64().unwrap_or(0.0),
                tokens: m["tokens"].is_object().then(|| Tokens::parse(&m["tokens"])),
                error: m["error"]["message"].as_str().map(str::to_owned),
                failure: StepError::parse(&m["error"]),
                retry: Retry::parse(&m["retry"]),
                created: i(&m["time"]["created"]),
                first_token: m["time"]["firstToken"].as_i64(),
                streamed: m["time"]["streamed"].as_i64(),
                model: model_of(&m["model"]),
                agent: m["agent"].as_str().filter(|a| !a.is_empty()).map(str::to_owned),
                changed: strings(&m["snapshot"]["files"]),
            })
        }
        "shell" => Message::Note {
            text: format!("$ {}", s(&m["command"])),
            failed: m["status"] == "timeout" || (m["status"] == "exited" && m["exit"].as_i64().is_some_and(|e| e != 0)),
            step: Box::new(tool::shell_step(&id, m)),
            id,
        },
        "compaction" => Message::Compaction(Compaction::parse(m)),
        "widget" => return super::widget::parse(m),
        _ => return notice::parse(m),
    })
}

/// A user message's text as its bubble shows it (Desktop's `readPromptPresentation`): without
/// the `Attached file:` lines and quotes it carries for the model, which show as cards over it
/// (see `mention::user_path_attachments` and [`super::user_comments`]); the typed text when they
/// aren't at its end as Desktop puts them (`composer/request.ts`).
fn quoted_text(id: &str, text: String, metadata: &Value) -> String {
    let Some((display, comments)) = crate::quote::parse_metadata(metadata) else { return text };
    let refs = crate::mention::path_attachments(metadata).unwrap_or_default();
    if comments.is_empty() && refs.is_empty() {
        return text;
    }
    let tail: Vec<String> = refs.iter().map(|r| format!("Attached file: `{}`", r.path)).chain(comments.iter().map(PromptComment::prompt_text)).collect();
    let tail = tail.join("\n");
    set_user_comments(id, comments);
    match text.strip_suffix(tail.as_str()) {
        Some(head) => head.strip_suffix('\n').unwrap_or(head).to_owned(),
        None => {
            // The recorded mention offsets index the raw text, not this one.
            crate::mention::set_user_mentions(id, Vec::new());
            display
        }
    }
}

/// Messages in chronological order from one `desc` page.
pub fn parse_messages(v: &Value) -> Vec<Message> {
    let mut list: Vec<&Value> = v["data"].as_array().into_iter().flatten().collect();
    // By `time.created`, then id. A steered prompt keeps the id it was sent with but takes its
    // delivery time, so it sorts after the work it waited behind, as the server orders it.
    list.sort_by(|a, b| (i(&a["time"]["created"]), a["id"].as_str()).cmp(&(i(&b["time"]["created"]), b["id"].as_str())));
    list.into_iter().filter_map(parse_message).collect()
}

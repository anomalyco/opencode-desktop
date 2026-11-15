//! Subagents and background shells read as events rather than tool rows: started, ran,
//! finished and stopped, counted by kind (`timeline-next/events.ts`). Titles and commands stay
//! in the background panel above the composer; an events row lists them once opened.

use opencode_kit::model::{Notice, NoticeKind, Outcome, ToolStatus};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Subagent,
    Shell,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Verb {
    Started,
    Ran,
    Finished,
    Stopped,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub verb: Verb,
    pub kind: Kind,
    pub failed: bool,
    /// What opening the row lists: a subagent's task or a shell's command.
    pub title: Option<String>,
    pub agent: Option<String>,
    pub session: Option<String>,
    pub shell: Option<String>,
}

fn text(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

/// What a subagent or background shell call says happened (`toolEvent`). A call still being
/// written has nothing to count yet. `title`, the call's own title, stands in for a subagent's
/// task where its input names none.
pub fn tool_event(name: &str, title: &str, status: &ToolStatus, input: &Value, metadata: &Value) -> Option<Event> {
    let kind = if super::rules::current_name(name) == "shell" { Kind::Shell } else { Kind::Subagent };
    if *status == ToolStatus::Pending {
        return None;
    }
    let subagent = kind == Kind::Subagent;
    let item = |verb, failed| Event {
        verb,
        kind,
        failed,
        title: text(if subagent { &input["description"] } else { &input["command"] }).or_else(|| (subagent && !title.is_empty()).then(|| title.to_owned())),
        agent: if subagent { text(&input["agent"]).or_else(|| text(&input["subagent_type"])) } else { None },
        session: if subagent { opencode_kit::model::subagent_session(input, metadata).map(str::to_owned) } else { None },
        shell: if subagent { None } else { text(&metadata["shellID"]) },
    };
    if *status == ToolStatus::Running {
        return Some(item(Verb::Started, false));
    }
    let background = if subagent { metadata["status"] != "completed" } else { metadata["status"] == "running" || input["background"] == true };
    let verb = if background { Verb::Started } else { Verb::Ran };
    if *status == ToolStatus::Error {
        if super::rules::stopped(status, metadata) {
            return Some(item(Verb::Stopped, false));
        }
        return Some(item(verb, true));
    }
    Some(item(verb, false))
}

/// The outcome a background subagent or shell reports when it finishes (`noticeEvent`).
pub fn notice_event(n: &Notice) -> Option<Event> {
    let NoticeKind::Finished(f) = &n.kind else { return None };
    let (verb, failed) = match f.outcome {
        Outcome::Cancelled => (Verb::Stopped, false),
        Outcome::Error => (Verb::Finished, true),
        Outcome::Completed => (Verb::Finished, false),
    };
    Some(Event {
        verb,
        kind: if f.subagent { Kind::Subagent } else { Kind::Shell },
        failed,
        title: f.data.clone().filter(|t| !t.is_empty()),
        agent: f.agent.clone(),
        session: f.child.clone(),
        shell: f.shell.clone(),
    })
}

/// Which subagent or shell an event is about, where it says (`workKey`).
pub fn work_key(e: &Event) -> Option<(Kind, &str)> {
    match e.kind {
        Kind::Shell => e.shell.as_deref(),
        Kind::Subagent => e.session.as_deref(),
    }
    .map(|id| (e.kind, id))
}

/// One verb of a run of events, with how many of each kind.
pub struct Bucket {
    pub verb: Verb,
    pub kinds: Vec<(Kind, usize)>,
}

/// A run of events as one label (`summarizeEvents`): each verb once, in the order it first
/// happened, with how many of each kind, and how many failed overall.
pub fn summarize(events: &[Event]) -> (Vec<Bucket>, usize) {
    let mut out: Vec<Bucket> = Vec::new();
    for e in events {
        let k = match out.iter().position(|b| b.verb == e.verb) {
            Some(k) => k,
            None => {
                out.push(Bucket { verb: e.verb, kinds: Vec::new() });
                out.len() - 1
            }
        };
        match out[k].kinds.iter_mut().find(|(kind, _)| *kind == e.kind) {
            Some((_, n)) => *n += 1,
            None => out[k].kinds.push((e.kind, 1)),
        }
    }
    (out, events.iter().filter(|e| e.failed).count())
}

/// "Started", "Ran", "Finished", "Stopped" (`verbKeys`).
pub fn verb_word(v: Verb) -> &'static str {
    use opencode_i18n::t;
    match v {
        Verb::Started => t!("ui.timeline.event.started"),
        Verb::Ran => t!("ui.timeline.activity.ran"),
        Verb::Finished => t!("ui.timeline.event.finished"),
        Verb::Stopped => t!("ui.timeline.event.stopped"),
    }
}

/// "2 subagents", "1 shell" (`kindKeys`).
pub fn kind_count(kind: Kind, n: usize) -> String {
    match kind {
        Kind::Subagent => opencode_i18n::plural!("ui.timeline.event.subagents", n),
        Kind::Shell => opencode_i18n::plural!("ui.timeline.event.shells", n),
    }
}

/// How a group's label counts work that runs on beside its other calls: "started 1 shell"
/// (`launchKeys`).
pub fn launch_count(verb: Verb, kind: Kind, n: usize) -> String {
    use opencode_i18n::plural;
    match (verb, kind) {
        (Verb::Started, Kind::Shell) => plural!("ui.timeline.activity.startedShells", n),
        (Verb::Started, Kind::Subagent) => plural!("ui.timeline.activity.startedSubagents", n),
        (Verb::Ran, Kind::Shell) => plural!("ui.timeline.activity.commands", n),
        (Verb::Ran, Kind::Subagent) => plural!("ui.timeline.activity.subagents", n),
        (Verb::Finished, Kind::Shell) => plural!("ui.timeline.activity.finishedShells", n),
        (Verb::Finished, Kind::Subagent) => plural!("ui.timeline.activity.finishedSubagents", n),
        (Verb::Stopped, Kind::Shell) => plural!("ui.timeline.activity.stoppedShells", n),
        (Verb::Stopped, Kind::Subagent) => plural!("ui.timeline.activity.stoppedSubagents", n),
    }
}

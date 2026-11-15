//! Subagents and background shells read as events rather than tool rows
//! (`timeline-next/events.ts`, as of anomalyco/opencode 0cfe2c2610, #64017). Each subagent event
//! is a row of its own; background shells are counted by verb in one events row, which lists
//! their commands once opened.

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
    /// The parent sent a follow-up to a subagent it had started: a call naming its session.
    Updated,
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
    let follow_up = subagent && text(&input["sessionID"]).is_some();
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
        return Some(item(if follow_up { Verb::Updated } else { Verb::Started }, false));
    }
    let background = if subagent { metadata["status"] != "completed" } else { metadata["status"] == "running" || input["background"] == true };
    let verb = if background { Verb::Started } else { Verb::Ran };
    if *status == ToolStatus::Error {
        if super::rules::stopped(status, metadata) {
            return Some(item(Verb::Stopped, false));
        }
        return Some(item(verb, true));
    }
    if follow_up {
        return Some(item(Verb::Updated, false));
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

/// One verb of a run of shells, with how many.
pub struct Bucket {
    pub verb: Verb,
    pub count: usize,
}

/// Background shells counted by verb in the order they happened, and how many failed
/// (`summarizeShells`). Subagents have rows of their own, so they never count.
pub fn summarize_shells(events: &[Event]) -> (Vec<Bucket>, usize) {
    let mut out: Vec<Bucket> = Vec::new();
    let shells = || events.iter().filter(|e| e.kind == Kind::Shell);
    for e in shells().filter(|e| e.verb != Verb::Updated) {
        match out.iter_mut().find(|b| b.verb == e.verb) {
            Some(b) => b.count += 1,
            None => out.push(Bucket { verb: e.verb, count: 1 }),
        }
    }
    (out, shells().filter(|e| e.failed).count())
}

/// "Started", "Ran", "Finished", "Stopped" (`verbKeys`).
pub fn verb_word(v: Verb) -> &'static str {
    use opencode_i18n::t;
    match v {
        Verb::Started => t!("ui.timeline.event.started"),
        Verb::Ran => t!("ui.timeline.activity.ran"),
        Verb::Finished => t!("ui.timeline.event.finished"),
        Verb::Stopped => t!("ui.timeline.event.stopped"),
        Verb::Updated => t!("ui.timeline.subagent.updated"),
    }
}

/// What a subagent row says happened, after its task (`TimelineEventItem`'s `status`): Failed,
/// Updated, Completed, Started or Stopped.
pub fn subagent_status(e: &Event) -> &'static str {
    use opencode_i18n::t;
    match e.verb {
        _ if e.failed => t!("ui.toolErrorCard.failed"),
        Verb::Ran | Verb::Finished => t!("ui.timeline.subagent.completed"),
        v => verb_word(v),
    }
}

/// How a group's label counts background shells beside its other calls: "started 1 shell"
/// (`launchKeys`).
pub fn launch_count(verb: Verb, n: usize) -> String {
    use opencode_i18n::plural;
    match verb {
        Verb::Started | Verb::Updated => plural!("ui.timeline.activity.startedShells", n),
        Verb::Ran => plural!("ui.timeline.activity.commands", n),
        Verb::Finished => plural!("ui.timeline.activity.finishedShells", n),
        Verb::Stopped => plural!("ui.timeline.activity.stoppedShells", n),
    }
}

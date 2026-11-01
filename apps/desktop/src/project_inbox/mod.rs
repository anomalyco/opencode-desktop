//! Projects you talk to (t1191): any session can be marked a project. The subagents it starts
//! after the mark are its tasks, and they feed an inbox above the sidebar's tabs: Needs you (an
//! open question or permission anywhere in the task), Done (went idle and not read since), then
//! a line per project with how many of its tasks are working. A task's own subagents are its
//! helpers: they fold into its row. E marks a task read; right-click › Close archives it.
//!
//! The mark is `metadata.seerProject = { id, since }` on the project session, on the server. Its
//! children inherit the metadata, so a session is the project only when `id` is its own.
//!
//! Tasks you can reply to (t1213): a project can also start a task as a top-level session with
//! the Seer plugin's `task_start`, tagged `metadata.seerTask = { projectID, state, summary, … }`.
//! The task reports `ready`, `done`, `blocked` or `working`; the plugin marks one that went idle
//! without reporting `stopped`. Such a task has the composer, but no tab.

mod stories;
#[cfg(test)]
mod tests;
mod view;

use std::collections::HashSet;

use opencode_kit::client::{Request, Write};
use opencode_kit::model::SessionInfo;
use rune::Tx;
use serde_json::{Value, json};

use crate::{App, Route, Source};

pub use view::section;

#[derive(Default)]
pub struct State {
    /// What's been asked of the server already: a project's sessions (`p:`), a running task's
    /// open requests (`r:`).
    asked: HashSet<String>,
    /// The next session "New project" creates becomes a project.
    pub mark_next: bool,
    /// The task opened from Done: it stays in the list while it's open, until E.
    kept: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Needs,
    Done,
}

/// What a task said when it ended its turn (`seerTask.state`); a subagent task never says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Said {
    Nothing,
    Done,
    Blocked,
    Stopped,
}

/// One task in the inbox, under the project session it belongs to.
pub struct Row<'a> {
    pub info: &'a SessionInfo,
    pub kind: Kind,
    pub said: Said,
    pub project: &'a str,
}

/// `metadata.seerTask`: the project that started a session as a task, and where the task says it
/// stands ("working", "ready", "done", "blocked" or "stopped").
pub struct SeerTask<'a> {
    pub project: &'a str,
    pub state: &'a str,
}

pub trait TaskInfo {
    fn seer_task(&self) -> Option<SeerTask<'_>>;
    /// A task's own session: top-level and tagged. Its helpers inherit the tag but have a parent.
    fn is_seer_task(&self) -> bool;
}

impl TaskInfo for SessionInfo {
    fn seer_task(&self) -> Option<SeerTask<'_>> {
        let m = &self.metadata["seerTask"];
        Some(SeerTask { project: m["projectID"].as_str()?, state: m["state"].as_str().unwrap_or("working") })
    }

    fn is_seer_task(&self) -> bool {
        self.parent.is_none() && self.seer_task().is_some()
    }
}

fn marked(i: &SessionInfo) -> Option<i64> {
    i.seer_project().filter(|(id, _)| *id == i.id).map(|m| m.1)
}

impl App {
    /// The project sessions and when each was marked, newest first.
    pub fn project_marks(&self) -> Vec<(&str, i64)> {
        self.sessions.iter().filter(|i| i.archived.is_none()).filter_map(|i| Some((i.id.as_str(), marked(i)?))).collect()
    }

    pub fn is_project(&self, id: &str) -> bool {
        self.session(id).and_then(marked).is_some()
    }

    /// The task `id` is or helps: a top-level session a project started with `task_start`, or
    /// the session in its chain started straight under a project after the project was marked.
    pub fn lead_of<'a>(&'a self, id: &'a str) -> Option<&'a str> {
        self.ancestors(id).find(|a| {
            self.session(a).is_some_and(|i| {
                i.is_seer_task() || i.parent.as_deref().and_then(|p| self.session(p)).and_then(marked).is_some_and(|since| i.created >= since)
            })
        })
    }

    /// The project session `id` is a task (or helper) of.
    pub fn project_of_task(&self, id: &str) -> Option<&str> {
        let lead = self.session(self.lead_of(id)?)?;
        match lead.parent.as_deref() {
            Some(p) => Some(p),
            None => Some(lead.seer_task()?.project),
        }
    }

    /// A task with its own session: it has the composer, and no tab.
    pub fn is_task_session(&self, id: &str) -> bool {
        self.session(id).is_some_and(TaskInfo::is_seer_task)
    }

    /// What one page reads and asks about: a task with its helpers, else the whole tab.
    pub fn scope_of<'a>(&'a self, id: &'a str) -> &'a str {
        self.lead_of(id).unwrap_or_else(|| self.root_of(id))
    }

    fn task_asks(&self, lead: &str) -> bool {
        self.requests.iter().any(|(s, _)| self.lead_of(s) == Some(lead))
    }

    /// Needs you first, then Done, newest first in each.
    pub fn inbox_rows(&self) -> Vec<Row<'_>> {
        if self.project_marks().is_empty() {
            return vec![];
        }
        let kept = |id: &str| self.project_inbox.kept.as_deref() == Some(id) && self.route == Route::Session(id.to_owned());
        let mut rows: Vec<Row> = self
            .sessions
            .iter()
            .filter(|i| i.archived.is_none() && self.lead_of(&i.id) == Some(i.id.as_str()))
            .filter_map(|info| {
                let project = self.project_of_task(&info.id)?;
                let said = match info.seer_task().map(|t| t.state) {
                    Some("done") => Said::Done,
                    Some("blocked") => Said::Blocked,
                    Some("stopped") => Said::Stopped,
                    // Waiting on its own background work: still working.
                    Some("working") if !self.running.contains(&info.id) && !self.task_asks(&info.id) => return None,
                    _ => Said::Nothing,
                };
                let waiting = !self.running.contains(&info.id) && (info.unread() || kept(&info.id));
                let kind = if self.task_asks(&info.id) || (said == Said::Blocked && waiting) {
                    Kind::Needs
                } else if waiting {
                    Kind::Done
                } else {
                    return None;
                };
                Some(Row { info, kind, said, project })
            })
            .collect();
        rows.sort_by_key(|r| (r.kind != Kind::Needs, std::cmp::Reverse(r.info.idle.unwrap_or(r.info.updated).max(r.info.updated))));
        rows
    }

    /// A project's tasks running now (helpers count for their task) or waiting on their own
    /// background work, not counting those waiting on you.
    pub fn project_working(&self, project: &str) -> usize {
        let waiting = self.sessions.iter().filter(|i| i.archived.is_none() && i.seer_task().is_some_and(|t| t.state == "working") && i.is_seer_task());
        let leads: HashSet<&str> = self.running.iter().filter_map(|id| self.lead_of(id)).chain(waiting.map(|i| i.id.as_str())).collect();
        leads.into_iter().filter(|l| self.project_of_task(l) == Some(project) && !self.task_asks(l)).count()
    }

    /// "Make project" / "Stop being a project": `metadata.seerProject` on the server.
    pub fn toggle_project(&mut self, id: &str, tx: &mut Tx) {
        tx.instant();
        let Some(info) = self.session(id) else { return };
        let mut metadata = if info.metadata.is_object() { info.metadata.clone() } else { json!({}) };
        metadata["seerProject"] = if marked(info).is_some() { Value::Null } else { json!({ "id": id, "since": self.now_ms }) };
        if self.write(Write::Metadata { session: id.to_owned(), metadata: metadata.clone() })
            && let Some(i) = self.sessions.iter_mut().find(|i| i.id == id)
        {
            i.metadata = metadata;
        }
    }

    /// E: the task is read. It comes back to Done by itself once it goes idle again.
    pub fn mark_task_read(&mut self, id: &str, tx: &mut Tx) {
        tx.instant();
        if self.project_inbox.kept.as_deref() == Some(id) {
            self.project_inbox.kept = None;
        }
        let Some(idle) = self.session(id).filter(|i| i.unread()).and_then(|i| i.idle) else { return };
        if self.write(Write::View { session: id.to_owned(), idle })
            && let Some(i) = self.sessions.iter_mut().find(|i| i.id == id)
        {
            i.viewed = i.viewed.max(Some(idle));
        }
    }

    /// Right-click › Close: archived on the server, it leaves the inbox for good.
    pub fn close_task(&mut self, id: &str, tx: &mut Tx) {
        tx.instant();
        if !self.write(Write::Archive(id.to_owned())) {
            return;
        }
        let now = self.now_ms;
        if let Some(i) = self.sessions.iter_mut().find(|i| i.id == id) {
            i.archived = Some(now);
        }
    }

    /// "New project": a new session in a chosen folder, marked as it opens.
    pub fn new_project(&mut self, tx: &mut Tx) {
        self.project_inbox.mark_next = true;
        self.open_project_dialog(tx);
    }
}

/// `poll`: asks the server for what the inbox needs that the first reads don't bring: each
/// project's sessions with their subagents, and the open requests of running tasks.
pub fn sync(s: &mut App) -> bool {
    if s.launch.waiting || s.project_marks().is_empty() {
        return false;
    }
    let mut asks: Vec<(String, Request)> = s
        .project_marks()
        .iter()
        .filter_map(|(id, _)| s.session(id))
        .filter(|i| !i.project.is_empty())
        .map(|i| (format!("p:{}", i.project), Request::ProjectSessions(i.project.clone())))
        .collect();
    s.project_inbox.asked.retain(|k| !k.starts_with("r:") || s.running.contains(&k[2..]));
    for id in &s.running {
        if s.lead_of(id).is_some() {
            asks.push((format!("r:{id}"), Request::Pending(id.clone())));
        }
    }
    let Source::Live { requests: Some(r), .. } = &s.source else { return false };
    for (key, req) in asks {
        if s.project_inbox.asked.insert(key) {
            r.send(req).ok();
        }
    }
    false
}

/// Marks and archives from other clients.
pub fn event(s: &mut App, ty: &str, data: &Value, sid: &str) -> bool {
    let now = s.now_ms;
    let Some(i) = s.sessions.iter_mut().find(|i| i.id == sid) else { return false };
    match ty {
        "session.archived" | "session.unarchived" => i.archived = (ty == "session.archived").then_some(now),
        "session.metadata.updated" => i.metadata = data["metadata"].clone(),
        _ => return false,
    }
    true
}

/// The event stream reconnected: list the projects' sessions again.
pub fn reconnected(s: &mut App) {
    s.project_inbox.asked.clear();
}

/// A session this app just created and opened: "New project" marks it.
pub fn created(s: &mut App, id: &str, tx: &mut Tx) {
    if std::mem::take(&mut s.project_inbox.mark_next) && !s.is_project(id) {
        s.toggle_project(id, tx);
    }
}

/// Opening a task reads it on the server too, so every client's inbox agrees.
pub fn opened(s: &mut App, id: &str) {
    let Some(lead) = s.lead_of(id).map(str::to_owned) else { return };
    let Some(idle) = s.session(&lead).filter(|i| i.unread()).and_then(|i| i.idle) else { return };
    if lead == id && !s.running.contains(id) {
        s.project_inbox.kept = Some(lead.clone());
    }
    if s.live() && s.stage.is_none() {
        s.write(Write::View { session: lead, idle });
    }
}

rune_register::register!(crate::commands::COMMANDS: crate::commands::Cmd = [
    crate::commands::Cmd::new("project.new", "New project…", |s, tx| s.new_project(tx))
        .describe("Pick a folder, start a project in it"),
]);

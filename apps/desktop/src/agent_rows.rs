//! How a subagent call's row in the timeline is going: its latest steps, how long it has run,
//! and how it ended, for the ⌘⇧K Subagent rows styles (`agent_rows_view.rs`). Today's row
//! (`subagents::card`) needs none of it.

use std::collections::{HashMap, HashSet};

use rune::Tx;
use serde_json::Value;

use opencode_kit::model::{Message, Part, ToolStatus};

use crate::{App, Route};

/// Steps a row keeps, newest last.
const KEEP: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Style {
    /// Today's card, a port of Desktop Beta's: the agent, its task with "(background)", a
    /// spinner while it runs.
    Today,
    /// One line: the task, what it's doing now or how it ended, its type and its run time.
    Status,
    /// Agents started together: a quiet "3 agents" line with their counts, then a box each.
    Boxes,
    /// A box each, stacked so neighbours share an edge, with no header.
    Stacked,
    /// One card with a live row per agent: no header, no dividers.
    Card,
    /// One line, "3 agents · Read live.ts", that opens to a row per agent.
    Summary,
    /// A chip per agent, under one line with the latest step from any of them.
    Chips,
}

impl Style {
    /// Whether agents started together draw as one group.
    pub fn groups(self) -> bool {
        !matches!(self, Style::Today | Style::Status)
    }
}

pub const NAMES: [&str; 7] = ["Today", "Live status", "Boxes", "Stacked", "One card", "Summary", "Chips"];

pub fn style(s: &App) -> Style {
    match s.knobs.get(&crate::knobs::SUBAGENT_ROWS) as usize {
        1 => Style::Status,
        2 => Style::Boxes,
        3 => Style::Stacked,
        4 => Style::Card,
        5 => Style::Summary,
        6 => Style::Chips,
        _ => Style::Today,
    }
}

/// What the event stream says about subagents beyond running or not.
#[derive(Default)]
pub struct Live {
    /// Each session's latest steps, newest last, and how many it has taken.
    steps: HashMap<String, (Vec<String>, usize)>,
    /// Subagents that ended while we watched: whether they failed, when, and the error.
    ended: HashMap<String, (bool, i64, Option<String>)>,
    /// The last poll's clock, to tell when a second has passed.
    polled: i64,
    /// Counts what changed, so a session's last change can be told from another's.
    tick: u64,
    /// When each session's line last changed, on `tick`.
    moved: HashMap<String, u64>,
    /// Summaries opened to a row per agent, by their first call.
    pub open: HashSet<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    Running,
    Done,
    Failed,
}

/// One call's run, as the live styles draw it.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub state: State,
    /// The latest steps, newest last ("Read backoff.ts"), and how many came before them.
    pub steps: Vec<String>,
    pub taken: usize,
    /// "12s": how long it has run, or ran, and the same in milliseconds.
    pub time: Option<String>,
    pub ms: i64,
    /// When its line last changed, to find a group's latest.
    pub moved: u64,
    /// Its answer's first line, once done.
    pub result: Option<String>,
    /// What went wrong, once failed.
    pub error: Option<String>,
}

impl Run {
    /// The one line that says where it is: its step while running, else its result or error.
    pub fn line(&self) -> String {
        match self.state {
            State::Running => self.steps.last().cloned().unwrap_or_else(|| "Starting".to_owned()),
            State::Done => self.result.clone().unwrap_or_else(|| "Done".to_owned()),
            State::Failed => self.error.clone().unwrap_or_else(|| "Failed".to_owned()),
        }
    }
}

/// "Read backoff.ts", "Grep reconnect", "Shell bun test": a tool call as a one-line step.
pub fn step_of(name: &str, input: &Value) -> String {
    let get = |k: &str| input[k].as_str().map(str::to_owned);
    let target = match name {
        "shell" | "bash" => get("description").or_else(|| get("command")),
        "read" | "write" | "edit" => get("filePath").or_else(|| get("path")).map(|p| p.rsplit('/').next().unwrap_or("").to_owned()),
        "grep" | "glob" => get("pattern"),
        "task" | "subagent" => get("description"),
        "webfetch" => get("url"),
        "skill" => get("name"),
        _ => None,
    };
    let verb = crate::turns::tool_title(name);
    match target.map(|t| t.lines().next().unwrap_or("").trim().to_owned()).filter(|t| !t.is_empty()) {
        Some(t) => format!("{verb} {t}"),
        None => verb.to_owned(),
    }
}

fn first_line(text: &str) -> Option<String> {
    text.lines().map(str::trim).find(|l| !l.is_empty()).map(|l| l.trim_start_matches(['#', '*', '-', ' ']).to_owned())
}

impl App {
    /// Keeps [`Live`] up to date from one event; true when what it shows changed.
    pub fn note_agent(&mut self, ty: &str, data: &Value, sid: &str) -> bool {
        if sid.is_empty() || self.session(sid).is_none_or(|i| i.parent.is_none()) {
            return false;
        }
        self.agent_rows.tick += 1;
        let tick = self.agent_rows.tick;
        let step = match ty {
            "session.tool.called" => step_of(data["name"].as_str().unwrap_or(""), &data["input"]),
            "session.reasoning.started" => "Thinking".to_owned(),
            "session.execution.started" => {
                let had = self.agent_rows.ended.remove(sid).is_some();
                return self.agent_rows.steps.remove(sid).is_some() || had;
            }
            "session.execution.succeeded" | "session.execution.failed" | "session.execution.interrupted" => {
                let error = data["error"]["message"].as_str().or(data["error"].as_str()).map(str::to_owned);
                self.agent_rows.ended.insert(sid.to_owned(), (ty != "session.execution.succeeded", self.now_ms, error));
                self.agent_rows.moved.insert(sid.to_owned(), tick);
                return true;
            }
            _ => return false,
        };
        let (list, taken) = self.agent_rows.steps.entry(sid.to_owned()).or_default();
        if list.last() == Some(&step) {
            return false;
        }
        list.push(step);
        *taken += 1;
        self.agent_rows.moved.insert(sid.to_owned(), tick);
        if list.len() > KEEP {
            list.remove(0);
        }
        true
    }

    /// How the call (in `status`) that started `child` is going.
    pub fn agent_run(&self, child: Option<&str>, status: &ToolStatus) -> Run {
        let info = child.and_then(|c| self.session(c));
        let running = *status == ToolStatus::Running || child.is_some_and(|c| self.subtree_running(c));
        let ended = child.and_then(|c| self.agent_rows.ended.get(c)).filter(|_| !running);
        let failed = *status == ToolStatus::Error || ended.is_some_and(|e| e.0) || info.and_then(|i| i.outcome.as_deref()) == Some("failed");
        let state = if running { State::Running } else if failed { State::Failed } else { State::Done };
        let (steps, taken) = match child.and_then(|c| self.agent_rows.steps.get(c)) {
            Some((l, n)) => (l.clone(), n.saturating_sub(l.len())),
            None => (child.and_then(|c| self.last_tool(c)).into_iter().collect(), 0),
        };
        let ms = info.filter(|i| i.created > 0).map(|i| {
            let end = match ended {
                _ if running => self.now_ms,
                Some(e) => e.1,
                None => i.idle.unwrap_or(i.updated).max(i.created),
            };
            end - i.created
        });
        let time = ms.map(crate::subagents::elapsed);
        let moved = child.and_then(|c| self.agent_rows.moved.get(c)).copied().unwrap_or(0);
        let result = (state == State::Done).then(|| child.and_then(|c| self.answer(c))).flatten();
        let error = (state == State::Failed).then(|| ended.and_then(|e| e.2.as_deref()).and_then(first_line)).flatten();
        Run { state, steps, taken, time, ms: ms.unwrap_or(0), moved, result, error }
    }

    fn last_tool(&self, child: &str) -> Option<String> {
        self.transcripts.get(child)?.messages.iter().rev().find_map(|m| match m {
            Message::Assistant(a) => a.parts.iter().rev().find_map(|p| match p {
                Part::Tool { name, input, .. } => Some(step_of(name, input)),
                _ => None,
            }),
            _ => None,
        })
    }

    /// The first line of the child's last answer.
    fn answer(&self, child: &str) -> Option<String> {
        self.transcripts.get(child)?.messages.iter().rev().find_map(|m| match m {
            Message::Assistant(a) => a.parts.iter().rev().find_map(|p| match p {
                Part::Text { text } => first_line(text),
                _ => None,
            }),
            _ => None,
        })
    }
}

/// Wakes the app each second while a live style shows a running agent's time; true when a
/// second just passed.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let (now, last) = (s.now_ms, s.agent_rows.polled);
    s.agent_rows.polled = now;
    s.agent_rows.ended.retain(|_, e| now - e.1 < 3_600_000);
    let live = style(s) != Style::Today;
    let ticking = live && matches!(&s.route, Route::Session(id) if s.running.iter().any(|r| r != id && s.descends(r, id)));
    if !ticking {
        return false;
    }
    let next = (now.div_euclid(1000) + 1) * 1000;
    tx.wake_after(std::time::Duration::from_millis((next - now).max(1) as u64));
    now.div_euclid(1000) != last.div_euclid(1000)
}

#[cfg(test)]
#[path = "agent_rows_tests.rs"]
mod tests;

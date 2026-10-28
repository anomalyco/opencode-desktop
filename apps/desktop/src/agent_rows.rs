//! How a subagent call's row in the timeline is going: its latest steps, how long it has run,
//! and how it ended (`agent_rows_view.rs` draws it).

use std::collections::HashMap;

use rune::Tx;
use serde_json::Value;

use opencode_kit::model::{Message, Part, ToolStatus};

use crate::{App, Route};

/// Steps a row keeps, newest last.
const KEEP: usize = 3;

/// What the event stream says about subagents beyond running or not.
#[derive(Default)]
pub struct Live {
    /// Each session's latest steps, newest last, and how many it has taken.
    steps: HashMap<String, (Vec<String>, usize)>,
    /// Subagents that ended while we watched: whether they failed, when, and the error.
    ended: HashMap<String, (bool, i64, Option<String>)>,
    /// When each subagent's current run started, for runs that started while we watched.
    pub(crate) started: HashMap<String, i64>,
    /// The last poll's clock, to tell when a second has passed.
    polled: i64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    Running,
    Done,
    Failed,
}

/// One call's run, as its row draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub state: State,
    /// The latest steps, newest last ("Read backoff.ts"), and how many came before them.
    pub steps: Vec<String>,
    pub taken: usize,
    /// How long it has run, or ran, in ms.
    pub ms: Option<i64>,
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
        let step = match ty {
            "session.tool.called" => step_of(data["name"].as_str().unwrap_or(""), &data["input"]),
            "session.reasoning.started" => "Thinking".to_owned(),
            "session.execution.started" => {
                self.agent_rows.started.insert(sid.to_owned(), self.now_ms);
                let had = self.agent_rows.ended.remove(sid).is_some();
                return self.agent_rows.steps.remove(sid).is_some() || had;
            }
            "session.execution.succeeded" | "session.execution.failed" | "session.execution.interrupted" => {
                self.agent_rows.started.remove(sid);
                let error = data["error"]["message"].as_str().or(data["error"].as_str()).map(str::to_owned);
                self.agent_rows.ended.insert(sid.to_owned(), (ty != "session.execution.succeeded", self.now_ms, error));
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
            if running {
                return self.now_ms - self.run_started(&i.id, None);
            }
            let end = match ended {
                Some(e) => e.1,
                None => i.idle.unwrap_or(i.updated).max(i.created),
            };
            end - i.created
        });
        let result = (state == State::Done).then(|| child.and_then(|c| self.answer(c))).flatten();
        let error = (state == State::Failed).then(|| ended.and_then(|e| e.2.as_deref()).and_then(first_line)).flatten();
        Run { state, steps, taken, ms, result, error }
    }

    /// When `child`'s current run started, as Desktop dates it (`session/requests/background.ts`):
    /// the latest call to it (`call`, when it began running), else the child's creation. A long
    /// lived child that a newer call or message woke again started no earlier than it last went
    /// idle, and a start we saw live is exact.
    pub fn run_started(&self, child: &str, call: Option<i64>) -> i64 {
        let info = self.session(child);
        let created = info.map_or(0, |i| i.created);
        let idle = info.and_then(|i| i.idle).unwrap_or(0);
        let seen = self.agent_rows.started.get(child).copied().unwrap_or(0);
        created.max(idle).max(call.unwrap_or(0)).max(seen)
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

/// Wakes the app on each whole second of a running agent's time, counted from when it started
/// (not the wall clock's second), while a row shows it; true when one just passed.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let (now, last) = (s.now_ms, s.agent_rows.polled);
    s.agent_rows.polled = now;
    s.agent_rows.ended.retain(|_, e| now - e.1 < 3_600_000);
    let Route::Session(id) = &s.route else { return false };
    let starts: Vec<i64> = s.running.iter().filter(|r| *r != id && s.descends(r, id)).filter_map(|r| s.session(r)).map(|i| i.created).collect();
    if starts.is_empty() {
        return false;
    }
    let second = |t: i64, start: i64| (t - start).div_euclid(1000);
    let next = starts.iter().map(|&c| c + (second(now, c) + 1) * 1000).min().unwrap_or(now + 1000);
    tx.wake_after(std::time::Duration::from_millis((next - now).max(1) as u64));
    starts.iter().any(|&c| second(now, c) != second(last, c))
}

#[cfg(test)]
#[path = "agent_rows_tests.rs"]
mod tests;

//! Background agents beyond running or not: what each subagent is doing right now, how its run
//! ended, and which of their timeline cards are scrolled away. The ⌘⇧K Subagents style picks how
//! they're drawn (`agents_view.rs`); Desktop's way (`subagents.rs`) stays the default.

use std::collections::{HashMap, HashSet};

use rune::Tx;
use serde_json::Value;

use opencode_kit::model::{self, Message, Part};

use crate::subagents::Running;
use crate::{App, Route};

/// How long a finished agent shows its ✓ before it folds away, in milliseconds.
pub const HOLD_MS: i64 = 1600;
/// How long its fold takes before it's gone: the height spring, settled.
pub const FOLD_MS: i64 = 800;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Style {
    /// Desktop's: the timeline card and the "2 subagents" capsule with its list.
    Desktop,
    /// One row per agent docked over the composer: what it's doing and for how long.
    Stack,
    /// The timeline card itself goes live; a chip over the composer jumps back to it.
    Inline,
    /// The capsule goes live: the newest agent's step and run time, the others as a count.
    Island,
}

pub const NAMES: [&str; 4] = ["Desktop", "Live stack", "Inline cards", "Live capsule"];

pub fn style(s: &App) -> Style {
    match s.knobs.get(&crate::knobs::AGENTS_STYLE) as usize {
        1 => Style::Stack,
        2 => Style::Inline,
        3 => Style::Island,
        _ => Style::Desktop,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    Running,
    Done,
    Failed,
}

/// One subagent as the live styles draw it.
#[derive(Clone, Debug, PartialEq)]
pub struct Agent {
    pub run: Running,
    pub state: State,
    /// What it's doing ("Read backoff.ts"), or how it ended.
    pub step: String,
    /// Milliseconds it has run, or ran.
    pub ran: i64,
    /// Ended, held its ✓, and now folding away.
    pub folded: bool,
}

/// What the event stream says about subagents beyond running or not.
#[derive(Default)]
pub struct Live {
    /// Each session's latest step.
    steps: HashMap<String, String>,
    /// Subagents that ended while we watched: whether they failed, and when.
    ended: HashMap<String, (bool, i64)>,
    /// The timeline's agent cards outside its viewport, by row key (`a-<call>`).
    off: HashSet<String>,
    /// The live stack shows every row, past its first few.
    pub more: bool,
    /// The last poll's clock, to tell when a hold or a second has passed.
    polled: i64,
}

impl Live {
    pub fn step(&self, session: &str) -> Option<&str> {
        self.steps.get(session).map(String::as_str)
    }

    /// Records which agent cards lie outside the timeline's visible `range` of `keys`; true when
    /// that changed.
    pub fn seen<'a>(&mut self, keys: impl Iterator<Item = &'a str>, range: std::ops::Range<usize>) -> bool {
        let off: HashSet<String> = keys.enumerate().filter(|(i, k)| !range.contains(i) && k.starts_with("a-")).map(|(_, k)| k.to_owned()).collect();
        if off == self.off {
            return false;
        }
        self.off = off;
        true
    }

    /// Whether the timeline card in row `row` is scrolled away. A row it hasn't placed yet isn't.
    pub fn off(&self, row: &str) -> bool {
        self.off.contains(row)
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

impl App {
    /// Keeps [`Live`] up to date from one event, before the running set takes it; true when
    /// what it shows changed.
    pub fn note_agent(&mut self, ty: &str, data: &Value, sid: &str) -> bool {
        if sid.is_empty() {
            return false;
        }
        let step = match ty {
            "session.tool.called" => step_of(data["name"].as_str().unwrap_or(""), &data["input"]),
            "session.reasoning.started" => "Thinking".to_owned(),
            "session.text.started" => "Writing".to_owned(),
            "session.execution.started" => {
                let had = self.agents.ended.remove(sid).is_some();
                return self.agents.steps.remove(sid).is_some() || had;
            }
            "session.execution.succeeded" | "session.execution.failed" | "session.execution.interrupted" => {
                if !self.running.contains(sid) || self.session(sid).is_none_or(|i| i.parent.is_none()) {
                    return false;
                }
                self.agents.ended.insert(sid.to_owned(), (ty != "session.execution.succeeded", self.now_ms));
                return true;
            }
            _ => return false,
        };
        self.agents.steps.insert(sid.to_owned(), step.clone()) != Some(step)
    }

    /// A child's step: the stream's latest, else its transcript's last tool, else "Working".
    pub fn agent_step(&self, child: &str) -> String {
        if let Some(s) = self.agents.step(child) {
            return s.to_owned();
        }
        let last = self.transcripts.get(child).and_then(|t| {
            t.messages.iter().rev().find_map(|m| match m {
                Message::Assistant(a) => a.parts.iter().rev().find_map(|p| match p {
                    Part::Tool { name, input, .. } => Some(step_of(name, input)),
                    _ => None,
                }),
                _ => None,
            })
        });
        last.unwrap_or_else(|| "Working".to_owned())
    }

    /// The call in `parent`'s transcript that started `child`: its timeline row is `a-<call>`.
    pub fn call_of(&self, parent: &str, child: &str) -> Option<String> {
        self.transcripts.get(parent)?.messages.iter().rev().find_map(|m| match m {
            Message::Assistant(a) => a.parts.iter().find_map(|p| match p {
                Part::Tool { id, name, input, metadata, .. }
                    if matches!(name.as_str(), "task" | "subagent") && model::subagent_session(input, metadata) == Some(child) =>
                {
                    Some(id.clone())
                }
                _ => None,
            }),
            _ => None,
        })
    }

    /// How `child` ended while we watched, if it did and still shows: failed, and when.
    pub fn agent_ended(&self, child: &str) -> Option<(bool, i64)> {
        self.agents.ended.get(child).copied().filter(|_| !self.running.contains(child))
    }

    /// The subagents below `id` for the live styles, in the order they started: those running,
    /// and those that ended less than [`HOLD_MS`] + [`FOLD_MS`] ago.
    pub fn agents_of(&self, id: &str) -> Vec<Agent> {
        let now = self.now_ms;
        let mut ids: Vec<&str> = self.running.iter().map(String::as_str).filter(|r| *r != id && self.descends(r, id)).collect();
        ids.extend(
            self.agents
                .ended
                .iter()
                .filter(|(k, (_, at))| now - at < HOLD_MS + FOLD_MS && !self.running.contains(*k) && self.descends(k, id))
                .map(|(k, _)| k.as_str()),
        );
        self.subagents_named(id, ids.into_iter())
            .into_iter()
            .map(|run| {
                let (state, step, end) = match self.agent_ended(&run.session) {
                    Some((true, at)) => (State::Failed, "Failed".to_owned(), at),
                    Some((false, at)) => (State::Done, "Done".to_owned(), at),
                    None => (State::Running, self.agent_step(&run.session), now),
                };
                let ran = if run.started > 0 { end - run.started } else { 0 };
                let folded = state != State::Running && now - end >= HOLD_MS;
                Agent { run, state, step, ran, folded }
            })
            .collect()
    }
}

/// Wakes the app when a finished agent's ✓ has held long enough to fold, when its fold is done,
/// and each second a live style shows a run time; true when one of those just passed.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let (now, last) = (s.now_ms, s.agents.polled);
    s.agents.polled = now;
    s.agents.ended.retain(|_, (_, at)| now - *at < 60_000);
    let mut changed = false;
    let mut next: Option<i64> = None;
    let mut due = |at: i64| {
        if at <= now && at > last {
            changed = true;
        }
        if at > now {
            next = Some(next.map_or(at - now, |n: i64| n.min(at - now)));
        }
    };
    for (_, at) in s.agents.ended.values() {
        due(at + HOLD_MS);
        due(at + HOLD_MS + FOLD_MS);
    }
    let ticking = style(s) != Style::Desktop
        && matches!(&s.route, Route::Session(id) if s.running.iter().any(|r| r != id && s.descends(r, id)));
    if ticking {
        due((last.div_euclid(1000) + 1) * 1000);
        due((now.div_euclid(1000) + 1) * 1000);
    }
    if let Some(ms) = next {
        tx.wake_after(std::time::Duration::from_millis(ms.max(1) as u64));
    }
    changed
}

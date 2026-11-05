//! How a subagent call's row in the timeline is going: its latest steps, how long it has run,
//! and how it ended (`agent_rows_view.rs` draws it).

use std::collections::HashMap;

use rune::Tx;
use serde_json::Value;

use opencode_kit::model::{Assistant, Message, Part, ToolStatus};

use crate::{App, Route};

/// Steps a row shows, newest last.
const KEEP: usize = 3;
/// Steps a session's log keeps, so a steered call's newest card still finds its own.
const LOG: usize = 64;
/// Ends a subagent's log keeps.
const ENDS: usize = 16;

/// What the event stream says about subagents beyond running or not.
#[derive(Default)]
pub struct Live {
    /// Each session's latest steps, kept across its runs so each call's card finds its own.
    steps: HashMap<String, Log>,
    /// When each subagent's latest run started while we watched; unlike `started`, kept after it ends.
    began: HashMap<String, i64>,
    /// Subagents that ended while we watched: whether they failed, when, and the error.
    ended: HashMap<String, (bool, i64, Option<String>)>,
    /// When each subagent's runs ended while we watched, and whether they failed, oldest first.
    ends: HashMap<String, Vec<(i64, bool)>>,
    /// When each subagent's current run started, for runs that started while we watched.
    pub(crate) started: HashMap<String, i64>,
    /// When each subagent call began, by its tool part, for calls made while we watched.
    called: HashMap<String, i64>,
    /// The last poll's clock, to tell when a second has passed.
    polled: i64,
}

/// One session's latest steps and when each came, newest last, and the steps it let go of.
#[derive(Default)]
struct Log {
    list: Vec<(i64, String)>,
    dropped: usize,
    /// When the newest step it let go of came.
    dropped_at: i64,
}

pub use session_view::subagent_card::{Run, State};

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
        if ty == "session.tool.called"
            && matches!(data["name"].as_str(), Some("subagent" | "task"))
            && let Some(id) = data["id"].as_str()
        {
            self.agent_rows.called.entry(id.to_owned()).or_insert(self.now_ms);
        }
        // A session we don't know yet may be a subagent that started while the link was down.
        if sid.is_empty() || self.session(sid).is_some_and(|i| i.parent.is_none()) {
            return false;
        }
        let step = match ty {
            "session.tool.called" => step_of(data["name"].as_str().unwrap_or(""), &data["input"]),
            "session.reasoning.started" => "Thinking".to_owned(),
            "session.execution.started" => {
                self.agent_rows.started.insert(sid.to_owned(), self.now_ms);
                self.agent_rows.began.insert(sid.to_owned(), self.now_ms);
                let had = self.agent_rows.ended.remove(sid).is_some();
                return self.agent_rows.steps.contains_key(sid) || had;
            }
            "session.execution.succeeded" | "session.execution.failed" | "session.execution.interrupted" => {
                self.agent_rows.started.remove(sid);
                let failed = ty != "session.execution.succeeded";
                let error = data["error"]["message"].as_str().or(data["error"].as_str()).map(str::to_owned);
                self.agent_rows.ended.insert(sid.to_owned(), (failed, self.now_ms, error));
                let ends = self.agent_rows.ends.entry(sid.to_owned()).or_default();
                ends.push((self.now_ms, failed));
                if ends.len() > ENDS {
                    ends.remove(0);
                }
                return true;
            }
            _ => return false,
        };
        let began = self.agent_rows.began.get(sid).copied().unwrap_or(i64::MIN);
        let log = self.agent_rows.steps.entry(sid.to_owned()).or_default();
        // The same step again, in the same run.
        if log.list.last().is_some_and(|l| l.1 == step && l.0 >= began) {
            return false;
        }
        log.list.push((self.now_ms, step));
        if log.list.len() > LOG {
            let (at, _) = log.list.remove(0);
            log.dropped += 1;
            log.dropped_at = at;
        }
        true
    }

    /// Drops `child`'s step log, so its row reads its steps from a freshly loaded page.
    pub(crate) fn forget_agent_steps(&mut self, child: &str) {
        self.agent_rows.steps.remove(child);
    }

    /// How call `call` in `parent`, which started `child` or sent it more work, is going: only
    /// its own share of the child's work when other calls name the same child (`calls.rs`).
    pub fn agent_run(&self, parent: &str, call: &str, child: Option<&str>, status: &ToolStatus) -> Run {
        let window = child.and_then(|c| self.window(parent, call, c));
        if let (Some(c), Some(w)) = (child, &window)
            && let Some(next) = &w.next
        {
            return self.earlier_run(c, w, next, status);
        }
        let from = window.and_then(|w| w.from);
        let info = child.and_then(|c| self.session(c));
        let running = *status == ToolStatus::Running || child.is_some_and(|c| self.subtree_running(c));
        let ended = child.and_then(|c| self.agent_rows.ended.get(c)).filter(|_| !running);
        // A call to an agent that had finished, before its new run starts.
        let waking = ended.is_some_and(|e| from.is_some_and(|f| e.1 < f));
        let (running, ended) = (running || waking, ended.filter(|_| !waking));
        let failed = *status == ToolStatus::Error || ended.is_some_and(|e| e.0) || info.and_then(|i| i.outcome.as_deref()) == Some("failed");
        let state = if running { State::Running } else if failed { State::Failed } else { State::Done };
        // Its steps start with its own call or the child's latest run, whichever came later.
        let began = child.and_then(|c| self.agent_rows.began.get(c).copied());
        let since = from.max(began);
        let (steps, taken) = match child.and_then(|c| self.steps_in(c, since, None)).filter(|s| !s.0.is_empty()) {
            Some(s) => s,
            None => (child.and_then(|c| self.last_tool(c, from)).into_iter().collect(), 0),
        };
        let ms = info.filter(|i| i.created > 0).map(|i| {
            let start = i.created.max(from.unwrap_or(0));
            if running {
                return self.run_clock() - self.run_started(&i.id, None).max(start);
            }
            let end = match ended {
                Some(e) => e.1,
                None => i.idle.unwrap_or(i.updated).max(i.created),
            };
            end - start
        });
        let result = (state == State::Done).then(|| child.and_then(|c| self.answer(c, from, None))).flatten();
        let error = (state == State::Failed).then(|| ended.and_then(|e| e.2.as_deref()).and_then(first_line)).flatten();
        Run { state, steps, taken, ms, result, error }
    }

    /// `child`'s latest steps between `from` and `to` (either open), newest last, and how many
    /// came before them; None when we saw none.
    fn steps_in(&self, child: &str, from: Option<i64>, to: Option<i64>) -> Option<(Vec<String>, usize)> {
        let log = self.agent_rows.steps.get(child)?;
        let mine: Vec<&String> = log.list.iter().filter(|s| from.is_none_or(|f| s.0 >= f) && to.is_none_or(|t| s.0 < t)).map(|s| &s.1).collect();
        // Steps the log let go of came before every step it kept: count them when the window
        // reaches back past the newest of them.
        let dropped = if log.dropped > 0 && from.is_none_or(|f| f <= log.dropped_at) { log.dropped } else { 0 };
        let count = mine.len() + if mine.is_empty() { 0 } else { dropped };
        let shown: Vec<String> = mine[mine.len().saturating_sub(KEEP)..].iter().map(|s| (*s).clone()).collect();
        let n = shown.len();
        Some((shown, count - n))
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

    /// The child's last tool call since `from`.
    fn last_tool(&self, child: &str, from: Option<i64>) -> Option<String> {
        self.steps_of(child, from, None).find_map(|a| {
            a.parts.iter().rev().find_map(|p| match p {
                Part::Tool { name, input, .. } => Some(step_of(name, input)),
                _ => None,
            })
        })
    }

    /// The first line of the child's last answer between `from` and `to`.
    fn answer(&self, child: &str, from: Option<i64>, to: Option<i64>) -> Option<String> {
        self.steps_of(child, from, to).find_map(|a| {
            a.parts.iter().rev().find_map(|p| match p {
                Part::Text { text } => first_line(text),
                _ => None,
            })
        })
    }

    /// The child's steps that began between `from` and `to`, newest first: all of them when
    /// both are open.
    fn steps_of(&self, child: &str, from: Option<i64>, to: Option<i64>) -> impl Iterator<Item = &Assistant> {
        let open = from.is_none() && to.is_none();
        let messages = self.transcripts.get(child).map_or(&[][..], |t| &t.messages[..]);
        messages.iter().rev().filter_map(move |m| match m {
            Message::Assistant(a) if open || (a.created > 0 && from.is_none_or(|f| a.created >= f) && to.is_none_or(|t| a.created < t)) => Some(a),
            _ => None,
        })
    }
}

impl App {
    /// The clock running times count to: the app clock's last whole second. Every row, the
    /// subagent bar and the capsule's list tick together, exactly a second apart, whenever
    /// each run started.
    pub fn run_clock(&self) -> i64 {
        self.now_ms.div_euclid(1000) * 1000
    }

    /// Whether the open page shows a running time: a subagent at or below it runs.
    fn run_times_shown(&self) -> bool {
        let Route::Session(id) = &self.route else { return false };
        self.running.iter().any(|r| self.descends(r, id) && self.session(r).is_some_and(|i| i.parent.is_some()))
    }
}

/// Wakes the app once on each whole second of [`App::run_clock`] while the page shows a running
/// time; true when one just passed.
pub fn poll(s: &mut App, tx: &mut Tx) -> bool {
    let (now, last) = (s.now_ms, s.agent_rows.polled);
    s.agent_rows.polled = now;
    if !s.agent_rows.ended.is_empty() {
        s.agent_rows.ended.retain(|_, e| now - e.1 < 3_600_000);
    }
    if s.running.is_empty() || !s.run_times_shown() {
        return false;
    }
    let second = now.div_euclid(1000);
    tx.wake_after(std::time::Duration::from_millis(((second + 1) * 1000 - now) as u64));
    second != last.div_euclid(1000)
}

#[path = "agent_rows/calls.rs"]
mod calls;
#[path = "agent_rows/steered_stories.rs"]
pub(crate) mod steered_stories;
#[cfg(test)]
#[path = "agent_rows/steered_tests.rs"]
mod steered_tests;

#[cfg(test)]
#[path = "agent_rows_tests.rs"]
mod tests;

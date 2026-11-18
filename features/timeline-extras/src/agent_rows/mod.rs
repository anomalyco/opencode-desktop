//! How a subagent call's row in the timeline is going: its latest steps, how long it has run,
//! and how it ended (`session_view::subagent_card` draws it).

pub mod calls;

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use opencode_kit::model::{Assistant, Message, Part, SessionInfo, ToolStatus, Transcript};
use rune::Tx;
use serde_json::Value;

pub use session_view::subagent_card::{Run, State};

/// Steps a row shows, newest last.
const KEEP: usize = 3;
/// Steps a session's log keeps, so a steered call's newest card still finds its own.
const LOG: usize = 64;
/// Ends a subagent's log keeps.
const ENDS: usize = 16;

/// Object-safe host boundary for live subagent row tracking, steered-call windowing, and
/// shared whole-second run-clock ticking.
pub trait AgentRowsHost: opencode_kernel::AppEnv {
    fn agent_rows(&self) -> &Live;
    fn agent_rows_mut(&mut self) -> &mut Live;

    /// Metadata for session `id`, if loaded.
    fn session_info(&self, id: &str) -> Option<&SessionInfo>;
    /// Loaded transcript for session `id`, if any.
    fn transcript(&self, id: &str) -> Option<&Transcript>;
    /// Set of currently running session IDs.
    fn running_sessions(&self) -> &HashSet<String>;
}

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
    pub(crate) ends: HashMap<String, Vec<(i64, bool)>>,
    /// When each subagent's current run started, for runs that started while we watched.
    pub started: HashMap<String, i64>,
    /// When each subagent call began, by its tool part, for calls made while we watched.
    pub called: HashMap<String, i64>,
    /// The last poll's clock, to tell when a second has passed.
    polled: i64,
}

impl Live {
    /// Records when subagent tool call `id` began (also used by steered-subagent stories).
    pub fn record_call(&mut self, id: String, at_ms: i64) {
        self.called.insert(id, at_ms);
    }

    /// When subagent tool call `id` began, if observed live.
    pub fn call_started_at(&self, id: &str) -> Option<i64> {
        self.called.get(id).copied()
    }

    /// When subagent session `id`'s current live run started, if observed live.
    pub fn started_at(&self, id: &str) -> Option<i64> {
        self.started.get(id).copied()
    }

    /// Drops `child`'s step log so its row reads steps from a freshly loaded transcript.
    pub fn forget_steps(&mut self, child: &str) {
        self.steps.remove(child);
    }
}

/// One session's latest steps and when each came, newest last, and the steps it let go of.
#[derive(Default)]
struct Log {
    list: Vec<(i64, String)>,
    dropped: usize,
    /// When the newest step it let go of came.
    dropped_at: i64,
}

/// "Read backoff.ts", "Grep reconnect", "Shell bun test": a tool call as a one-line step.
pub fn step_of(name: &str, input: &Value) -> String {
    let get = |k: &str| input[k].as_str().map(str::to_owned);
    let target = match name {
        "shell" | "bash" => get("description").or_else(|| get("command")),
        "read" | "write" | "edit" => get("filePath").or_else(|| get("path")).map(|p| opencode_kit::path::basename(&p).to_owned()),
        "grep" | "glob" => get("pattern"),
        "task" | "subagent" => get("description"),
        "webfetch" => get("url"),
        "skill" => get("name"),
        _ => None,
    };
    let verb = session_view::turns::tool_title(name);
    match target.map(|t| t.lines().next().unwrap_or("").trim().to_owned()).filter(|t| !t.is_empty()) {
        Some(t) => format!("{verb} {t}"),
        None => verb.to_owned(),
    }
}

fn first_line(text: &str) -> Option<String> {
    text.lines().map(str::trim).find(|l| !l.is_empty()).map(|l| l.trim_start_matches(['#', '*', '-', ' ']).to_owned())
}

/// Keeps [`Live`] up to date from one event; true when what it shows changed.
pub fn note_agent(host: &mut dyn AgentRowsHost, ty: &str, data: &Value, sid: &str) -> bool {
    let now_ms = host.now_ms();
    if ty == "session.tool.called"
        && matches!(data["name"].as_str(), Some("subagent" | "task"))
        && let Some(id) = data["id"].as_str()
        && !host.agent_rows().called.contains_key(id)
    {
        host.agent_rows_mut().called.insert(id.to_owned(), now_ms);
    }
    // A session we don't know yet may be a subagent that started while the link was down.
    if sid.is_empty() || host.session_info(sid).is_some_and(|i| i.parent.is_none()) {
        return false;
    }
    let step = match ty {
        "session.tool.called" => step_of(data["name"].as_str().unwrap_or(""), &data["input"]),
        "session.reasoning.started" => opencode_i18n::t!("ui.sessionTurn.status.thinking").to_owned(),
        "session.execution.started" => {
            let rows = host.agent_rows_mut();
            rows.started.insert(sid.to_owned(), now_ms);
            rows.began.insert(sid.to_owned(), now_ms);
            let had = rows.ended.remove(sid).is_some();
            return rows.steps.contains_key(sid) || had;
        }
        "session.execution.succeeded" | "session.execution.failed" | "session.execution.interrupted" => {
            let rows = host.agent_rows_mut();
            rows.started.remove(sid);
            let failed = ty != "session.execution.succeeded";
            let error = data["error"]["message"].as_str().or(data["error"].as_str()).map(str::to_owned);
            rows.ended.insert(sid.to_owned(), (failed, now_ms, error));
            let ends = rows.ends.entry(sid.to_owned()).or_default();
            ends.push((now_ms, failed));
            if ends.len() > ENDS {
                ends.remove(0);
            }
            return true;
        }
        _ => return false,
    };
    let began = host.agent_rows().began.get(sid).copied().unwrap_or(i64::MIN);
    // The same step again, in the same run.
    if host.agent_rows().steps.get(sid).and_then(|log| log.list.last()).is_some_and(|l| l.1 == step && l.0 >= began) {
        return false;
    }
    let log = host.agent_rows_mut().steps.entry(sid.to_owned()).or_default();
    log.list.push((now_ms, step));
    if log.list.len() > LOG {
        let (at, _) = log.list.remove(0);
        log.dropped += 1;
        log.dropped_at = at;
    }
    true
}

/// Drops `child`'s step log, so its row reads its steps from a freshly loaded page.
pub fn forget_agent_steps(host: &mut dyn AgentRowsHost, child: &str) {
    if host.agent_rows().steps.contains_key(child) {
        host.agent_rows_mut().steps.remove(child);
    }
}

/// How call `call` in `parent`, which started `child` or sent it more work, is going: only
/// its own share of the child's work when other calls name the same child (`calls.rs`).
pub fn agent_run(host: &dyn AgentRowsHost, parent: &str, call: &str, child: Option<&str>, status: &ToolStatus) -> Run {
    let window = child.and_then(|c| calls::window(host, parent, call, c));
    if let (Some(c), Some(w)) = (child, &window)
        && let Some(next) = &w.next
    {
        return calls::earlier_run(host, c, w, next, status);
    }
    let from = window.and_then(|w| w.from);
    let info = child.and_then(|c| host.session_info(c));
    let running = *status == ToolStatus::Running || child.is_some_and(|c| subtree_running(host, c));
    let ended = child.and_then(|c| host.agent_rows().ended.get(c)).filter(|_| !running);
    // A call to an agent that had finished, before its new run starts.
    let waking = ended.is_some_and(|e| from.is_some_and(|f| e.1 < f));
    let (running, ended) = (running || waking, ended.filter(|_| !waking));
    let failed = *status == ToolStatus::Error || ended.is_some_and(|e| e.0) || info.and_then(|i| i.outcome.as_deref()) == Some("failed");
    let state = if running { State::Running } else if failed { State::Failed } else { State::Done };
    // Its steps start with its own call or the child's latest run, whichever came later.
    let began = child.and_then(|c| host.agent_rows().began.get(c).copied());
    let since = from.max(began);
    let (steps, taken) = match child.and_then(|c| steps_in(host, c, since, None)).filter(|s| !s.0.is_empty()) {
        Some(s) => s,
        None => (child.and_then(|c| last_tool(host, c, from)).into_iter().collect(), 0),
    };
    let ms = info.filter(|i| i.created > 0).map(|i| {
        let start = i.created.max(from.unwrap_or(0));
        if running {
            return run_clock(host) - run_started(host, &i.id, None).max(start);
        }
        let end = match ended {
            Some(e) => e.1,
            None => i.idle.unwrap_or(i.updated).max(i.created),
        };
        end - start
    });
    let result = (state == State::Done).then(|| child.and_then(|c| answer(host, c, from, None))).flatten();
    let error = (state == State::Failed).then(|| ended.and_then(|e| e.2.as_deref()).and_then(first_line)).flatten();
    Run { state, steps, taken, ms, result, error }
}

/// When `child`'s current run started, as Desktop dates it (`session/requests/background.ts`):
/// the latest call to it (`call`, when it began running), else the child's creation. A long
/// lived child that a newer call or message woke again started no earlier than it last went
/// idle, and a start we saw live is exact.
pub fn run_started(host: &dyn AgentRowsHost, child: &str, call: Option<i64>) -> i64 {
    let info = host.session_info(child);
    let created = info.map_or(0, |i| i.created);
    let idle = info.and_then(|i| i.idle).unwrap_or(0);
    let seen = host.agent_rows().started.get(child).copied().unwrap_or(0);
    created.max(idle).max(call.unwrap_or(0)).max(seen)
}

/// The clock running times count to: the host clock's last whole second. Every row, the
/// subagent bar and the capsule's list tick together, exactly a second apart, whenever
/// each run started.
pub fn run_clock(host: &dyn AgentRowsHost) -> i64 {
    host.now_ms().div_euclid(1000) * 1000
}

/// Wakes the host once on each whole second of [`run_clock`] while the page shows a running
/// time; true when one just passed.
pub fn poll(host: &mut dyn AgentRowsHost, tx: &mut Tx) -> bool {
    let (now, last) = (host.now_ms(), host.agent_rows().polled);
    // Only the second matters (below): the rows are written once a second, not every poll.
    if now.div_euclid(1000) != last.div_euclid(1000) {
        host.agent_rows_mut().polled = now;
    }
    if host.agent_rows().ended.values().any(|e| now - e.1 >= 3_600_000) {
        host.agent_rows_mut().ended.retain(|_, e| now - e.1 < 3_600_000);
    }
    if host.running_sessions().is_empty() || !run_times_shown(host) {
        return false;
    }
    let second = now.div_euclid(1000);
    let wait_ms = ((second + 1) * 1000 - now).max(0) as u64;
    tx.wake_after(Duration::from_millis(wait_ms));
    second != last.div_euclid(1000)
}

/// `child`'s latest steps between `from` and `to` (either open), newest last, and how many
/// came before them; None when we saw none.
pub(crate) fn steps_in(host: &dyn AgentRowsHost, child: &str, from: Option<i64>, to: Option<i64>) -> Option<(Vec<String>, usize)> {
    let log = host.agent_rows().steps.get(child)?;
    let mine: Vec<&String> = log.list.iter().filter(|s| from.is_none_or(|f| s.0 >= f) && to.is_none_or(|t| s.0 < t)).map(|s| &s.1).collect();
    // Steps the log let go of came before every step it kept: count them when the window
    // reaches back past the newest of them.
    let dropped = if log.dropped > 0 && from.is_none_or(|f| f <= log.dropped_at) { log.dropped } else { 0 };
    let count = mine.len() + if mine.is_empty() { 0 } else { dropped };
    let shown: Vec<String> = mine[mine.len().saturating_sub(KEEP)..].iter().map(|s| (*s).clone()).collect();
    let n = shown.len();
    Some((shown, count - n))
}

/// The child's last tool call since `from`.
fn last_tool(host: &dyn AgentRowsHost, child: &str, from: Option<i64>) -> Option<String> {
    steps_of(host, child, from, None).find_map(|a| {
        a.parts.iter().rev().find_map(|p| match p {
            Part::Tool { name, input, .. } => Some(step_of(name, input)),
            _ => None,
        })
    })
}

/// The first line of the child's last answer between `from` and `to`.
pub(crate) fn answer(host: &dyn AgentRowsHost, child: &str, from: Option<i64>, to: Option<i64>) -> Option<String> {
    steps_of(host, child, from, to).find_map(|a| {
        a.parts.iter().rev().find_map(|p| match p {
            Part::Text { text } => first_line(text),
            _ => None,
        })
    })
}

/// The child's steps that began between `from` and `to`, newest first: all of them when
/// both are open.
pub(crate) fn steps_of<'a>(host: &'a dyn AgentRowsHost, child: &str, from: Option<i64>, to: Option<i64>) -> impl Iterator<Item = &'a Assistant> {
    let open = from.is_none() && to.is_none();
    let messages = host.transcript(child).map_or(&[][..], |t| &t.messages[..]);
    messages.iter().rev().filter_map(move |m| match m {
        Message::Assistant(a) if open || (a.created > 0 && from.is_none_or(|f| a.created >= f) && to.is_none_or(|t| a.created < t)) => Some(a),
        _ => None,
    })
}

/// `id` and its loaded parents, up to 32 steps.
fn ancestors<'a>(host: &'a dyn AgentRowsHost, id: &'a str) -> impl Iterator<Item = &'a str> {
    std::iter::successors(Some(id), move |cur| host.session_info(cur)?.parent.as_deref()).take(33)
}

/// Whether `id` is `ancestor` or a session below it (up to 32 steps).
fn descends(host: &dyn AgentRowsHost, id: &str, ancestor: &str) -> bool {
    ancestors(host, id).take(32).any(|cur| cur == ancestor)
}

/// Whether `id` or anything below it is running.
fn subtree_running(host: &dyn AgentRowsHost, id: &str) -> bool {
    host.running_sessions().iter().any(|r| descends(host, r, id))
}

/// Whether the open page shows a running time: a subagent at or below it runs.
fn run_times_shown(host: &dyn AgentRowsHost) -> bool {
    let Some(id) = host.active_session_id() else { return false };
    host.running_sessions().iter().any(|r| descends(host, r, id) && host.session_info(r).is_some_and(|i| i.parent.is_some()))
}

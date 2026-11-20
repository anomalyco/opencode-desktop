//! The open session tree's running background jobs, as Desktop keeps them
//! (`app/src/session/requests/model.ts`): listed once when a session opens or the stream comes
//! back, then kept by `experimental.job.updated` events. Events that land while the list request
//! is in flight apply over its response, which may predate them (`mergeJobList` in
//! `requests/background.ts`, anomalyco/opencode#64966).
//!
//! A Code Mode script moved to the background runs as a `codemode` job: it shows as a script in
//! the running-work list until its job ends (anomalyco/opencode#64790).

use std::collections::BTreeMap;

use opencode_kit::model::{Message, Part, ToolStatus};
use serde_json::Value;

use crate::shells::{SubagentJob, SubagentRequest};
use crate::{SubagentsHost, ancestors};

/// What a job runs (`Job.Kind`), as far as the running-work list cares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Shell,
    /// A subagent job belongs to the child session it runs.
    Subagent { child: String },
    /// A Code Mode script.
    Script,
    /// Images, videos and kinds this app doesn't know.
    Other,
}

/// One running job (`Job.Info` with `status: "running"`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub id: String,
    pub kind: Kind,
    /// The session that started it.
    pub session: String,
    /// When it started, epoch ms.
    pub started: i64,
}

impl Job {
    /// The session whose tree the job runs in: a subagent's own child, or the session that started it.
    pub fn owner(&self) -> &str {
        match &self.kind {
            Kind::Subagent { child } => child,
            _ => &self.session,
        }
    }
}

/// A background script of the open session, for the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Script {
    /// Its job's id.
    pub id: String,
    /// Its code's first line, or the job's id.
    pub label: String,
    pub started: i64,
}

/// The list request's phase. `updates`: the events seen since it was sent, by job, `None` once
/// the job stopped running.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Load {
    #[default]
    Idle,
    Loading { attempt: u64, session: String, updates: BTreeMap<String, Option<Job>> },
}

/// The machine: the running jobs by id, the list request, and the counter its attempts come from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Jobs {
    running: BTreeMap<String, Job>,
    load: Load,
    issued: u64,
}

impl Jobs {
    pub fn running(&self) -> impl Iterator<Item = &Job> {
        self.running.values()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A session opened, or the stream came back while it was open.
    Opened { session: String },
    /// `experimental.job.updated`: the job runs (`Some`) or has ended (`None`).
    Updated { id: String, job: Option<Job> },
    Listed { attempt: u64, jobs: Vec<Job> },
    ListFailed { attempt: u64 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    List { attempt: u64, session: String },
}

pub fn step(state: &mut Jobs, event: Event) -> Vec<Effect> {
    match event {
        Event::Opened { session } => {
            state.issued += 1;
            let attempt = state.issued;
            state.load = Load::Loading { attempt, session: session.clone(), updates: BTreeMap::new() };
            vec![Effect::List { attempt, session }]
        }
        Event::Updated { id, job } => {
            if let Load::Loading { updates, .. } = &mut state.load {
                updates.insert(id.clone(), job.clone());
            }
            apply(&mut state.running, id, job);
            Vec::new()
        }
        Event::Listed { attempt, jobs } => {
            let Load::Loading { attempt: a, updates, .. } = &mut state.load else { return Vec::new() };
            if *a != attempt {
                return Vec::new();
            }
            let mut running: BTreeMap<String, Job> = jobs.into_iter().map(|j| (j.id.clone(), j)).collect();
            for (id, job) in std::mem::take(updates) {
                apply(&mut running, id, job);
            }
            state.running = running;
            state.load = Load::Idle;
            Vec::new()
        }
        Event::ListFailed { attempt } => {
            if matches!(state.load, Load::Loading { attempt: a, .. } if a == attempt) {
                state.load = Load::Idle;
            }
            Vec::new()
        }
    }
}

fn apply(running: &mut BTreeMap<String, Job>, id: String, job: Option<Job>) {
    match job {
        Some(job) => {
            running.insert(id, job);
        }
        None => {
            running.remove(&id);
        }
    }
}

/// Tells the machine `event` and runs what it asks for.
fn dispatch(host: &mut dyn SubagentsHost, event: Event) {
    let effects = step(&mut host.shells_mut().jobs, event);
    for e in effects {
        match e {
            Effect::List { attempt, session } => host.spawn_shell_request(SubagentJob::Server {
                request: SubagentRequest::ListJobs { attempt, session },
                sync_if_headless: true,
            }),
        }
    }
}

/// Reads the running jobs of `session`'s tree: when it opens, and again after a reconnect.
pub fn list(host: &mut dyn SubagentsHost, session: &str) {
    if session.is_empty() || !host.is_live() {
        return;
    }
    dispatch(host, Event::Opened { session: session.to_owned() });
}

/// `experimental.job.updated`.
pub fn event(host: &mut dyn SubagentsHost, ty: &str, data: &Value) -> bool {
    if ty != "experimental.job.updated" {
        return false;
    }
    let Some((id, job)) = parse(&data["info"]) else { return false };
    let known = host.shells().jobs.running.get(&id);
    if known == job.as_ref() && !matches!(host.shells().jobs.load, Load::Loading { .. }) {
        return false;
    }
    dispatch(host, Event::Updated { id, job });
    true
}

/// The list request landed (`Some`) or failed.
pub(crate) fn listed(host: &mut dyn SubagentsHost, attempt: u64, jobs: Option<Vec<Job>>) {
    let event = match jobs {
        Some(jobs) => Event::Listed { attempt, jobs },
        None => Event::ListFailed { attempt },
    };
    dispatch(host, event);
}

/// `GET /api/experimental/job`'s answer: its running jobs.
pub(crate) fn parse_list(v: &Value) -> Vec<Job> {
    v.get("data").unwrap_or(v).as_array().into_iter().flatten().filter_map(|i| parse(i)?.1).collect()
}

/// A `Job.Info` by its id: the job while it runs, `None` once it ended.
fn parse(info: &Value) -> Option<(String, Option<Job>)> {
    let id = info["id"].as_str().filter(|id| !id.is_empty())?.to_owned();
    if info["status"] != "running" {
        return Some((id, None));
    }
    let kind = match info["kind"].as_str() {
        Some("shell") => Kind::Shell,
        Some("subagent") => Kind::Subagent { child: info["childSessionID"].as_str().unwrap_or("").to_owned() },
        Some("codemode") => Kind::Script,
        _ => Kind::Other,
    };
    let job = Job { id: id.clone(), kind, session: info["sessionID"].as_str().unwrap_or("").to_owned(), started: info["time"]["started"].as_i64().unwrap_or(0) };
    Some((id, Some(job)))
}

/// The running jobs in `session`'s tree: its own and those of every session below it.
pub fn owned_by<'a>(host: &'a dyn SubagentsHost, session: &'a str) -> impl Iterator<Item = &'a Job> + 'a {
    host.shells().jobs.running().filter(move |j| ancestors(host, j.owner()).take(32).any(|a| a == session))
}

/// The open session's own scripts moved to the background, while their job runs, oldest first
/// (`requests/background.ts`'s `scripts`). A script's completed execute call carries its job's
/// id; its code's first line names it.
pub fn scripts(host: &dyn SubagentsHost, id: &str) -> Vec<Script> {
    let running: Vec<&Job> = host.shells().jobs.running().filter(|j| j.kind == Kind::Script && j.session == id).collect();
    if running.is_empty() {
        return Vec::new();
    }
    let mut named: BTreeMap<&str, Option<String>> = BTreeMap::new();
    for m in host.transcript(id).map_or(&[][..], |t| &t.messages[..]) {
        let Message::Assistant(a) = m else { continue };
        for p in &a.parts {
            if let Part::Tool { name, status: ToolStatus::Completed, input, metadata, .. } = p
                && name == "execute"
                && let Some(job) = metadata["jobID"].as_str()
            {
                named.insert(job, first_line(input["code"].as_str()));
            }
        }
    }
    let mut out: Vec<Script> = running
        .into_iter()
        .map(|j| Script { id: j.id.clone(), label: named.get(j.id.as_str()).cloned().flatten().unwrap_or_else(|| j.id.clone()), started: j.started })
        .collect();
    out.sort_by(|a, b| (a.started, &a.id).cmp(&(b.started, &b.id)));
    out
}

/// A script's first non-blank line, trimmed.
pub fn first_line(code: Option<&str>) -> Option<String> {
    code?.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_owned)
}

/// The commands of the shells a running script has started and not finished
/// (`executeShellCommands`): a script in the foreground covers the shells it runs.
pub fn script_shell_commands(metadata: &Value) -> Vec<&str> {
    metadata["toolCalls"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["tool"] == "shell" && c["status"] == "running")
        .filter_map(|c| c["input"]["command"].as_str())
        .collect()
}

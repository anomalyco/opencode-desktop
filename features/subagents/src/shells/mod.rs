//! A session's background shells, as Desktop's running-work capsule lists them
//! (`activity-chips.tsx`, `requests/background.ts`): the server's running shells whose
//! `metadata.sessionID` is the session and that no running call of the session's current turn
//! holds, from `GET /api/shell` and the `shell.*` events. The list itself is the machine in
//! `activity.rs`; this runs its effects via [`SubagentJob`] requests on the host and timers on
//! the host clock.

pub mod follow;
pub mod tail;
pub mod view;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use opencode_kit::client::{JobEndpoint, Server};
use opencode_kit::model::{Assistant, JobInfo, JobListQuery, JobTracker, Message, Part, ToolStatus};
use rune::Tx;
use serde_json::Value;

use crate::activity::{self, Activity, Effect, EndInfo, EndStatus, Event, Kind, Timer, Work};
use crate::jobs;
use crate::shells::follow::Followers;
use crate::shells::tail::{Page, TailEffect, TailEvent};
use crate::{SubagentsHost, running_subagents};

/// A running shell the server lists (`ShellInfo`).
#[derive(Clone, Debug, PartialEq)]
pub struct Shell {
    pub id: String,
    pub session: String,
    pub command: String,
    /// When it started, epoch ms.
    pub started: i64,
    /// The location it runs in, which its reads name.
    pub directory: String,
}

/// A background server request scheduled by the host.
#[derive(Clone, Debug)]
pub enum SubagentJob {
    /// Run `request` against the live [`Server`] via [`execute_request`], then deliver the
    /// resulting [`SubagentReply`] via [`apply_reply`].
    ///
    /// When `sync_if_headless` is `true` and the host is headless (startup snapshot in [`list`]),
    /// the host executes `request` inline and applies the reply immediately so frame 1 already
    /// sees the listed shells.
    Server { request: SubagentRequest, sync_if_headless: bool },
}

/// Off-thread server work executed by [`execute_request`].
#[derive(Clone, Debug)]
pub enum SubagentRequest {
    ListShells { directory: String },
    ReadPanelOutput { id: String, directory: String, attempt: u64, cursor: u64 },
    ReadFollowOutput { id: String, directory: String, generation: u64, attempt: u64, cursor: u64 },
    LoadEnded { id: String, directory: String, attempt: u64 },
    Stop { attempt: u64, target: StopTarget },
    /// Running jobs of `session`'s tree (`GET /api/experimental/job`).
    ListJobs { attempt: u64, session: String },
}

/// What a Stop ends.
#[derive(Clone, Debug)]
pub enum StopTarget {
    Shell { id: String, directory: String },
    /// A subagent stops with everything its tree still runs: its own run while running, plus
    /// every running job in its tree (`packages/app/src/session/composer/region.tsx`'s `stop`).
    Subagent { session: String, interrupt: bool, jobs: Vec<String> },
    Script { job: String },
}

/// Outcome of `GET /api/experimental/job` for [`SubagentReply::Jobs`].
#[derive(Clone, Debug)]
pub enum JobListReply {
    Supported(Vec<JobInfo>),
    /// Server answered 404/405/501 (not `SessionNotFoundError`): older server without jobs.
    Unsupported,
    /// Transient transport error or missing session; does not mark the server unsupported.
    Failed,
}

/// Result of a [`SubagentRequest`], applied on the UI thread by [`apply_reply`].
#[derive(Clone, Debug)]
pub enum SubagentReply {
    Listed { directory: String, shells: Vec<Shell> },
    PanelOutput { id: String, attempt: u64, page: Result<Page, bool> },
    FollowOutput { id: String, generation: u64, attempt: u64, page: Result<Page, bool> },
    Ended { attempt: u64, info: Option<EndInfo> },
    Stopped { attempt: u64, ok: bool },
    Jobs { attempt: u64, result: JobListReply },
    None,
}

/// A timer the list's machine started, due on the host clock (ms).
#[derive(Clone, Debug)]
enum Due {
    Machine { attempt: u64, timer: Timer },
    TailWait { attempt: u64 },
    FollowWait { id: String, generation: u64, attempt: u64 },
}

#[derive(Default)]
pub struct Shells {
    running: Vec<Shell>,
    listed: HashSet<String>,
    pub activity: Activity,
    /// The timeline consoles' followers.
    pub follow: Followers,
    /// Per-server running background job trackers (keyed by [`SubagentsHost::server_scope`]).
    pub jobs: BTreeMap<String, JobTracker>,
    /// The work the machine last heard of, so a change is told to it once.
    work: Work,
    timers: Vec<(i64, Due)>,
}

impl Shells {
    /// Currently tracked running shells across all locations.
    pub fn running(&self) -> &[Shell] {
        &self.running
    }

    /// Replaces the tracked running shells (useful for fixture or story setup).
    pub fn set_running(&mut self, shells: Vec<Shell>) {
        self.running = shells;
    }

    /// Appends one running shell (useful for fixture or story setup).
    pub fn push_running(&mut self, shell: Shell) {
        self.running.retain(|r| r.id != shell.id);
        self.running.push(shell);
    }

    /// Whether the activity list or a shell's panel is currently open.
    pub fn is_open(&self) -> bool {
        self.activity.is_open()
    }
}

/// The running shells of session `id` in the capsule, oldest first: those no running call of its
/// current turn holds (by shell id, or by command before the call names one). A foreground
/// running `execute` script also covers the running shells it started (`requests/background.ts`).
pub fn running_for<'a>(host: &'a dyn SubagentsHost, id: &str) -> Vec<&'a Shell> {
    let reply = host.transcript(id).and_then(|t| t.open_reply());
    let running = || {
        reply.into_iter().flat_map(|a| a.parts.iter()).filter_map(|p| match p {
            Part::Tool { name, status: ToolStatus::Running, input, metadata, .. } => Some((name.as_str(), input, metadata)),
            _ => None,
        })
    };
    let held: Vec<(Option<&str>, &str)> = running()
        .filter(|(name, ..)| *name == "shell")
        .map(|(_, input, metadata)| (metadata["shellID"].as_str(), input["command"].as_str().unwrap_or("")))
        .collect();
    let scripts: Vec<(i64, Vec<&str>)> = running()
        .filter(|(name, ..)| *name == "execute")
        .map(|(_, _, metadata)| (opencode_kit::model::tool_ran(metadata).unwrap_or(0), jobs::script_shell_commands(metadata)))
        .collect();
    let by_script = |sh: &Shell| scripts.iter().any(|(started, commands)| sh.started >= *started && commands.contains(&sh.command.as_str()));
    let mut out: Vec<&Shell> = host
        .shells()
        .running
        .iter()
        .filter(|sh| {
            sh.session == id
                && !held.iter().any(|(sid, cmd)| *sid == Some(sh.id.as_str()) || (!cmd.is_empty() && *cmd == sh.command))
                && !by_script(sh)
        })
        .collect();
    out.sort_by(|a, b| (a.started, &a.id).cmp(&(b.started, &b.id)));
    out
}

/// What runs in the open session's background now, as the machine sees it.
pub fn work(host: &dyn SubagentsHost) -> Work {
    let Some(id) = host.active_session_id() else { return Work::default() };
    Work {
        enabled: host.background_activity_enabled(),
        subagents: running_subagents(host, id).into_iter().map(|r| r.session).collect(),
        scripts: jobs::scripts(host, id).into_iter().map(|s| s.id).collect(),
        shells: running_for(host, id).into_iter().map(|sh| sh.id.clone()).collect(),
    }
}

/// Whether the capsule counts the shown shell as well: one that ended while its panel is open.
pub fn held(host: &dyn SubagentsHost) -> bool {
    activity::held(&host.shells().activity, &work(host))
}

/// Tells the list's machine `event` and runs what it asks for.
pub fn dispatch(host: &mut dyn SubagentsHost, event: Event) {
    let work = work(host);
    let effects = {
        let shells = host.shells_mut();
        shells.work = work.clone();
        let state = std::mem::take(&mut shells.activity);
        let (next, effects) = activity::step(state, event, &work);
        shells.activity = next;
        effects
    };
    for e in effects {
        run(host, e);
    }
}

/// Opens or shuts the list.
pub fn set_open(host: &mut dyn SubagentsHost, open: bool) {
    dispatch(host, Event::Toggled(open));
}

/// Reads the running shells of `directory`, once per location until the stream reconnects.
pub fn list(host: &mut dyn SubagentsHost, directory: &str) {
    if directory.is_empty() || !host.is_live() || host.shells().listed.contains(directory) {
        return;
    }
    host.shells_mut().listed.insert(directory.to_owned());
    host.spawn_shell_request(SubagentJob::Server {
        request: SubagentRequest::ListShells { directory: directory.to_owned() },
        sync_if_headless: true,
    });
}

/// After a reconnect every location is read again.
pub fn forget_lists(host: &mut dyn SubagentsHost) {
    if !host.shells().listed.is_empty() {
        host.shells_mut().listed.clear();
    }
}

/// `shell.created`, `shell.exited` and `shell.deleted`.
pub fn event(host: &mut dyn SubagentsHost, ty: &str, data: &Value) -> bool {
    match ty {
        "shell.created" => {
            let info = &data["info"];
            let session = info["metadata"]["sessionID"].as_str().unwrap_or("");
            let directory = data["location"]["directory"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| host.session_info(session).map(|i| i.directory.clone()))
                .unwrap_or_default();
            let Some(sh) = parse(info, &directory).filter(|_| info["status"] == "running") else { return false };
            let running = &mut host.shells_mut().running;
            running.retain(|r| r.id != sh.id);
            running.push(sh);
            true
        }
        "shell.exited" | "shell.deleted" => {
            let Some(id) = data["id"].as_str() else { return false };
            if !host.shells().running.iter().any(|r| r.id == id) {
                return false;
            }
            host.shells_mut().running.retain(|r| r.id != id);
            true
        }
        _ => false,
    }
}

/// Executes a [`SubagentRequest`] off the UI thread against the live [`Server`].
pub fn execute_request(srv: &Server, request: SubagentRequest) -> SubagentReply {
    match request {
        SubagentRequest::ListShells { directory } => {
            let path = format!("/api/shell?{}", location(&directory));
            match srv.get(&path) {
                Ok(v) => SubagentReply::Listed { shells: parse_list(&v, &directory), directory },
                Err(_) => SubagentReply::None,
            }
        }
        SubagentRequest::ReadPanelOutput { id, directory, attempt, cursor } => {
            let path = format!("/api/shell/{}/output?{}&cursor={cursor}", encode_component(&id), location(&directory));
            let page = srv.get(&path).map(|v| page_of(&v)).map_err(|e| e.status() == Some(404));
            SubagentReply::PanelOutput { id, attempt, page }
        }
        SubagentRequest::ReadFollowOutput { id, directory, generation, attempt, cursor } => {
            let path = format!("/api/shell/{}/output?{}&cursor={cursor}", encode_component(&id), location(&directory));
            let page = srv.get(&path).map(|v| page_of(&v)).map_err(|e| e.status() == Some(404));
            SubagentReply::FollowOutput { id, generation, attempt, page }
        }
        SubagentRequest::LoadEnded { id, directory, attempt } => {
            let path = format!("/api/shell/{}?{}", encode_component(&id), location(&directory));
            let info = srv.get(&path).ok().map(|v| {
                let d = v.get("data").unwrap_or(&v);
                // A shell still running when asked was removed by a Stop.
                let status = match d["status"].as_str() {
                    Some("exited") => EndStatus::Exited,
                    Some("timeout") => EndStatus::Timeout,
                    _ => EndStatus::Killed,
                };
                EndInfo { status, exit: d["exit"].as_i64(), completed: d["time"]["completed"].as_i64() }
            });
            SubagentReply::Ended { attempt, info }
        }
        SubagentRequest::Stop { attempt, target } => {
            let cancel = |job: &str| srv.cancel_job(job).is_ok();
            let ok = match target {
                StopTarget::Shell { id, directory } => srv.write("DELETE", &format!("/api/shell/{}?{}", encode_component(&id), location(&directory)), None).is_ok(),
                StopTarget::Subagent { session, interrupt, jobs } => {
                    let interrupted = !interrupt || srv.interrupt(&session).is_ok();
                    jobs.iter().fold(interrupted, |ok, job| cancel(job) && ok)
                }
                StopTarget::Script { job } => cancel(&job),
            };
            SubagentReply::Stopped { attempt, ok }
        }
        SubagentRequest::ListJobs { attempt, session } => {
            let result = match srv.list_jobs(&JobListQuery::running_tree(session)) {
                Ok(JobEndpoint::Supported(jobs)) => JobListReply::Supported(jobs),
                Ok(JobEndpoint::Unsupported) => JobListReply::Unsupported,
                Err(_) => JobListReply::Failed,
            };
            SubagentReply::Jobs { attempt, result }
        }
    }
}

/// Applies a completed [`SubagentReply`] back onto `host` on the UI thread.
pub fn apply_reply(host: &mut dyn SubagentsHost, reply: SubagentReply) {
    let now = host.now_ms();
    match reply {
        SubagentReply::Listed { directory, shells } => replace(host.shells_mut(), &directory, shells),
        SubagentReply::PanelOutput { id, attempt, page } => {
            if host.shells().activity.shown().is_some_and(|(sh, ..)| sh.id == id) {
                let e = match page {
                    Ok(page) => TailEvent::Read { attempt, page },
                    Err(true) => TailEvent::ReadMissing { attempt },
                    Err(false) => TailEvent::ReadFailed { attempt },
                };
                dispatch(host, Event::Tail(e));
            }
        }
        SubagentReply::Ended { attempt, info } => dispatch(host, Event::EndedSettled { attempt, info, now }),
        SubagentReply::FollowOutput { id, generation, attempt, page } => {
            let effects = host.shells_mut().follow.landed(&id, generation, attempt, page);
            run_follow(host, effects);
        }
        SubagentReply::Stopped { attempt, ok } => {
            if !ok {
                host.toast_fail(opencode_i18n::t!("common.requestFailed").to_owned(), String::new());
            }
            dispatch(host, Event::StopSettled { attempt, stopped: ok, now });
        }
        SubagentReply::Jobs { attempt, result } => jobs::listed(host, attempt, result),
        SubagentReply::None => {}
    }
}

/// Fires due timers, tells the machine when the work changed, syncs timeline console followers,
/// and ticks the run times each second while the list shows them.
pub fn poll(host: &mut dyn SubagentsHost, tx: &mut Tx) -> bool {
    let before = host.shells().activity.clone();
    let mut changed = false;
    let now = host.now_ms();
    // Every poll comes here: the shells are written only when a timer is due, the work changed,
    // a followed shell shows or one stops showing, or the clock passes a second.
    let (due, later): (Vec<_>, Vec<_>) = if host.shells().timers.iter().any(|(at, _)| *at <= now) {
        std::mem::take(&mut host.shells_mut().timers).into_iter().partition(|(at, _)| *at <= now)
    } else {
        (Vec::new(), Vec::new())
    };
    if !due.is_empty() {
        host.shells_mut().timers = later;
    }
    for (_, d) in due {
        match d {
            Due::Machine { attempt, timer: Timer::Stopping } => dispatch(host, Event::StopTimedOut { attempt }),
            Due::Machine { attempt, timer: Timer::Ending } => dispatch(host, Event::EndedTimedOut { attempt, now }),
            Due::TailWait { attempt } => dispatch(host, Event::Tail(TailEvent::WaitElapsed { attempt })),
            Due::FollowWait { id, generation, attempt } => {
                let effects = host.shells_mut().follow.elapsed(&id, generation, attempt);
                run_follow(host, effects);
            }
        }
    }
    if work(host) != host.shells().work {
        dispatch(host, Event::WorkSynced);
    }
    let shown = followed(host);
    if !shown.is_empty() || host.shells().follow.shows_any() {
        let effects = host.shells_mut().follow.sync(&shown);
        run_follow(host, effects);
    }
    if let Some(next) = host.shells().timers.iter().map(|(at, _)| *at).min() {
        tx.wake_after(Duration::from_millis((next - now).max(0) as u64));
    }
    // The list's run times tick on whole seconds while it's open over running work. Writing only
    // the host's dedicated activity-clock slot keeps `SHELLS` (and the timeline) clean (ours.md §3.2).
    let work = &host.shells().work;
    if host.shells().activity.is_open() && (!work.shells.is_empty() || !work.scripts.is_empty() || !work.subagents.is_empty()) {
        let second = now.div_euclid(1000);
        tx.wake_after(Duration::from_millis(((second + 1) * 1000 - now).max(0) as u64));
        if host.activity_clock_sec() != second {
            host.set_activity_clock_sec(second);
            changed = true;
        }
    }
    changed || host.shells().activity != before
}

fn assistant_step(m: &Message) -> Option<&Assistant> {
    match m {
        Message::Assistant(a) => Some(a),
        Message::Note { step, .. } => Some(step),
        _ => None,
    }
}

/// The open shell consoles of the open session that read their shell, each
/// `(shell, directory, running)` (`tools::shell_follow`).
fn followed(host: &dyn SubagentsHost) -> Vec<(String, String, bool)> {
    let Some(id) = host.active_session_id() else { return Vec::new() };
    let Some(t) = host.transcript(id) else { return Vec::new() };
    let directory = host.session_info(id).map_or("", |i| i.directory.as_str());
    let running = |shell: &str| host.shells().running.iter().any(|r| r.id == shell);
    let scan = || -> Vec<(usize, usize)> {
        t.messages
            .iter()
            .enumerate()
            .filter_map(|(mi, m)| assistant_step(m).map(|a| (mi, a)))
            .flat_map(|(mi, a)| a.parts.iter().enumerate().map(move |(pi, p)| (mi, pi, p)))
            .filter_map(|(mi, pi, p)| match p {
                Part::Tool { id, name, status, input, metadata, .. } if name == "shell" => {
                    let call = session_view::tools::Call { id, name, status, input, metadata };
                    // A saved shell has nothing to follow, running or not.
                    session_view::tools::shell_follow(&call, |_| false).map(|_| (mi, pi))
                }
                _ => None,
            })
            .collect()
    };
    // Which calls could follow a shell is the transcript's to say: found once per version of it,
    // not on every poll of a long session.
    let calls = match host.transcripts_version() {
        Some(version) => FOLLOWABLE.with_borrow_mut(|kept| {
            if kept.0.as_ref().is_none_or(|(sid, v)| sid != id || *v != version) {
                *kept = (Some((id.to_owned(), version)), scan());
            }
            kept.1.clone()
        }),
        None => scan(),
    };
    calls
        .into_iter()
        .filter_map(|(mi, pi)| match assistant_step(t.messages.get(mi)?)?.parts.get(pi)? {
            Part::Tool { id, name, status, input, metadata, .. } => {
                let call = session_view::tools::Call { id, name, status, input, metadata };
                let (shell, live) = session_view::tools::shell_follow(&call, running)?;
                host.is_shell_console_open(&call).then(|| (shell.to_owned(), directory.to_owned(), live))
            }
            _ => None,
        })
        .collect()
}

type FollowableCache = (Option<(String, u64)>, Vec<(usize, usize)>);

thread_local! {
    /// The open session's shell calls that can follow their shell, by message and part, and the
    /// session and transcripts version they were found at.
    static FOLLOWABLE: RefCell<FollowableCache> = const { RefCell::new((None, Vec::new())) };
}

/// Runs the reads and waits of the timeline consoles' followers.
fn run_follow(host: &mut dyn SubagentsHost, effects: Vec<follow::Effect>) {
    let now = host.now_ms();
    for e in effects {
        match e.effect {
            TailEffect::Wait { attempt, ms } => host.shells_mut().timers.push((now + ms, Due::FollowWait { id: e.id, generation: e.generation, attempt })),
            TailEffect::Read { attempt, cursor } => {
                send_request(
                    host,
                    SubagentRequest::ReadFollowOutput { id: e.id, directory: e.directory, generation: e.generation, attempt, cursor },
                );
            }
        }
    }
}

fn run(host: &mut dyn SubagentsHost, effect: Effect) {
    let now = host.now_ms();
    match effect {
        Effect::TimerStart { attempt, timer } => {
            let ms = if timer == Timer::Stopping { activity::STOP_TIMEOUT_MS } else { activity::END_TIMEOUT_MS };
            host.shells_mut().timers.push((now + ms, Due::Machine { attempt, timer }));
        }
        Effect::TimerCancel(None) => host.shells_mut().timers.retain(|(_, d)| !matches!(d, Due::Machine { .. })),
        Effect::TimerCancel(Some(a)) => host.shells_mut().timers.retain(|(_, d)| !matches!(d, Due::Machine { attempt, .. } if *attempt == a)),
        Effect::Tail(TailEffect::Wait { attempt, ms }) => host.shells_mut().timers.push((now + ms, Due::TailWait { attempt })),
        Effect::Tail(TailEffect::Read { attempt, cursor }) => {
            let Some((id, directory)) = host.shells().activity.shown().map(|(sh, ..)| (sh.id.clone(), sh.directory.clone())) else { return };
            send_request(host, SubagentRequest::ReadPanelOutput { id, directory, attempt, cursor });
        }
        Effect::LoadEnded { attempt, id } => {
            let directory = host.shells().activity.shown().map(|(sh, ..)| sh.directory.clone()).unwrap_or_default();
            send_request(host, SubagentRequest::LoadEnded { id, directory, attempt });
        }
        Effect::RunStop { attempt, key, kind } => {
            let target = match kind {
                Kind::Shell => {
                    let directory = host.shells().running.iter().find(|r| r.id == key).map(|r| r.directory.clone()).unwrap_or_default();
                    StopTarget::Shell { id: key, directory }
                }
                Kind::Subagent => {
                    let jobs = jobs::owned_by(host, &key).map(|j| j.id.clone()).collect();
                    StopTarget::Subagent { interrupt: host.running_sessions().contains(&key), session: key, jobs }
                }
                Kind::Script => StopTarget::Script { job: key },
            };
            send_request(host, SubagentRequest::Stop { attempt, target });
        }
    }
}

fn send_request(host: &mut dyn SubagentsHost, request: SubagentRequest) {
    if !host.is_live() {
        return;
    }
    host.spawn_shell_request(SubagentJob::Server { request, sync_if_headless: false });
}

fn location(directory: &str) -> String {
    format!("location%5Bdirectory%5D={}", encode_component(directory))
}

/// Percent-encodes a URL path segment or query parameter value.
fn encode_component(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn page_of(v: &Value) -> Page {
    let d = v.get("data").unwrap_or(v);
    Page { output: d["output"].as_str().unwrap_or("").to_owned(), cursor: d["cursor"].as_u64().unwrap_or(0), size: d["size"].as_u64().unwrap_or(0) }
}

fn replace(shells: &mut Shells, directory: &str, list: Vec<Shell>) {
    shells.running.retain(|r| r.directory != directory);
    shells.running.extend(list);
}

fn parse_list(v: &Value, directory: &str) -> Vec<Shell> {
    let dir = v["location"]["directory"].as_str().unwrap_or(directory);
    v["data"].as_array().into_iter().flatten().filter(|i| i["status"] == "running").filter_map(|i| parse(i, dir)).collect()
}

fn parse(info: &Value, directory: &str) -> Option<Shell> {
    Some(Shell {
        id: info["id"].as_str()?.to_owned(),
        session: info["metadata"]["sessionID"].as_str().unwrap_or("").to_owned(),
        command: info["command"].as_str().unwrap_or("").to_owned(),
        started: info["time"]["started"].as_i64().unwrap_or(0),
        directory: directory.to_owned(),
    })
}

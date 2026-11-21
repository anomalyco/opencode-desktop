//! The open session tree's running background jobs (`packages/app/src/session/requests/model.ts`,
//! `packages/app/src/session/requests/background.ts`, anomalyco/opencode#64790, #64966).
//!
//! Uses `opencode_kit::model::JobTracker` per server scope so in-flight `experimental.job.updated`
//! events merge over late HTTP list responses (`merge_job_list`), older servers returning
//! 404/405/501 cache `JobLoadPhase::Unsupported` until reconnect, and multi-server switches never
//! bleed jobs or capability state across servers.

use std::collections::BTreeMap;

use opencode_kit::model::{
    JobInfo, JobTracker, JobTrackerEffect, JobTrackerEvent, Message, Part, ToolStatus, is_job_event, parse_job_updated,
};
use serde_json::Value;

use crate::shells::{JobListReply, SubagentJob, SubagentRequest};
use crate::{SubagentsHost, ancestors};

/// A background Code Mode script of the open session, for the activity list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Script {
    /// Its job's id.
    pub id: String,
    /// The first non-blank line of its `execute` call's `code`, or its job id.
    pub label: String,
    pub started: i64,
}

/// Active server's job tracker (read-only).
pub fn tracker(host: &dyn SubagentsHost) -> Option<&JobTracker> {
    host.shells().jobs.get(host.server_scope())
}

fn dispatch(host: &mut dyn SubagentsHost, event: JobTrackerEvent) {
    let scope = host.server_scope().to_owned();
    let effects = host.shells_mut().jobs.entry(scope).or_default().step(event);
    for effect in effects {
        match effect {
            JobTrackerEffect::List { attempt, session } => host.spawn_shell_request(SubagentJob::Server {
                request: SubagentRequest::ListJobs { attempt, session },
                sync_if_headless: true,
            }),
        }
    }
}

/// Lists the running jobs in `session`'s tree when a session opens or reconnects.
pub fn list(host: &mut dyn SubagentsHost, session: &str) {
    if session.is_empty() || !host.is_live() {
        return;
    }
    if tracker(host).is_some_and(JobTracker::is_unsupported) {
        return;
    }
    dispatch(host, JobTrackerEvent::Opened { session: session.to_owned() });
}

/// Clears the active server's cached jobs and `Unsupported` state on reconnect so a restarted
/// or upgraded server is probed afresh.
pub fn reset(host: &mut dyn SubagentsHost) {
    let scope = host.server_scope().to_owned();
    if host.shells().jobs.get(&scope).is_some_and(|t| !t.is_empty() || t.is_loading() || t.is_unsupported()) {
        host.shells_mut().jobs.entry(scope).or_default().step(JobTrackerEvent::Reset);
    }
}

/// Applies one `experimental.job.updated` SSE event; returns whether tracked job state changed.
pub fn event(host: &mut dyn SubagentsHost, ty: &str, data: &Value) -> bool {
    if !is_job_event(ty) {
        return false;
    }
    let Some(info) = parse_job_updated(data) else { return false };
    let changes = tracker(host).map_or_else(|| info.is_running(), |t| t.would_change(&info));
    if !changes {
        return false;
    }
    dispatch(host, JobTrackerEvent::Updated(info));
    true
}

/// Applies the reply to `SubagentRequest::ListJobs`.
pub(crate) fn listed(host: &mut dyn SubagentsHost, attempt: u64, reply: JobListReply) {
    let event = match reply {
        JobListReply::Supported(jobs) => JobTrackerEvent::Listed { attempt, jobs },
        JobListReply::Unsupported => JobTrackerEvent::Unsupported { attempt },
        JobListReply::Failed => JobTrackerEvent::ListFailed { attempt },
    };
    dispatch(host, event);
}

/// Running jobs on the active server.
pub fn running(host: &dyn SubagentsHost) -> impl Iterator<Item = &JobInfo> {
    tracker(host).into_iter().flat_map(JobTracker::running)
}

/// Running jobs in `session`'s tree: its own and those of every descendant session.
pub fn owned_by<'a>(host: &'a dyn SubagentsHost, session: &'a str) -> impl Iterator<Item = &'a JobInfo> + 'a {
    running(host).filter(move |j| ancestors(host, j.owner()).take(32).any(|a| a == session))
}

/// The open session's own Code Mode scripts running in the background, oldest first
/// (`requests/background.ts`'s `scripts`). Each completed `execute` call carrying `metadata.jobID`
/// names the job by the first non-blank line of `input.code`. Every script runs as a job, so one
/// still blocking the turn has a running job too: only a job such a call moved counts.
pub fn scripts(host: &dyn SubagentsHost, id: &str) -> Vec<Script> {
    let running: Vec<&JobInfo> = running(host).filter(|j| j.is_codemode() && j.session_id == id).collect();
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
        .filter(|j| named.contains_key(j.id.as_str()))
        .map(|j| Script {
            id: j.id.clone(),
            label: named.get(j.id.as_str()).cloned().flatten().unwrap_or_else(|| j.id.clone()),
            started: j.time.started,
        })
        .collect();
    out.sort_by(|a, b| (a.started, &a.id).cmp(&(b.started, &b.id)));
    out
}

/// First non-blank line of a script, trimmed.
pub fn first_line(code: Option<&str>) -> Option<String> {
    code?.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_owned)
}

/// Commands of running shells inside a foreground `execute` call's `metadata.toolCalls`
/// (`executeShellCommands` in `requests/background.ts`).
pub fn script_shell_commands(metadata: &Value) -> Vec<&str> {
    metadata["toolCalls"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["tool"] == "shell" && c["status"] == "running")
        .filter_map(|c| c["input"]["command"].as_str())
        .collect()
}

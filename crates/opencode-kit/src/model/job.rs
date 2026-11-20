//! Background jobs (`packages/schema/src/job.ts`, `packages/protocol/src/groups/job.ts`,
//! `packages/app/src/session/requests/{background.ts,model.ts}`): tolerant parsing for every job
//! kind (`shell`, `subagent`, `image`, `video`, `codemode`, and future unknown kinds), the
//! `merge_job_list` race merge (anomalyco/opencode#64966), and the explicit [`JobTracker`] state
//! machine that tracks live jobs across `/api/experimental/job` loads, `experimental.job.updated`
//! events, and older servers that do not expose the endpoint.

use std::collections::BTreeMap;

use serde_json::Value;

use super::{Outcome, i, s};

/// Lifecycle state of a background job (`Job.Status` in `packages/schema/src/job.ts`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Completed,
    Error,
    Cancelled,
    /// Future status from a newer server; treated as non-running.
    Unknown(String),
}

impl JobStatus {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "running" => Self::Running,
            "completed" => Self::Completed,
            "error" => Self::Error,
            "cancelled" => Self::Cancelled,
            other => Self::Unknown(other.to_owned()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
            Self::Unknown(raw) => raw.as_str(),
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }

    /// Terminal outcome for timeline notices (`None` while running or unknown).
    pub fn to_outcome(&self) -> Option<Outcome> {
        match self {
            Self::Completed => Some(Outcome::Completed),
            Self::Error => Some(Outcome::Error),
            Self::Cancelled => Some(Outcome::Cancelled),
            Self::Running | Self::Unknown(_) => None,
        }
    }
}

/// Start and optional completion timestamps in Unix epoch milliseconds (`Job.Time`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JobTime {
    pub started: i64,
    pub completed: Option<i64>,
}

/// Kind-specific fields of a background job (`Job.Info` tagged union in
/// `packages/schema/src/job.ts`, including `codemode` from #64343 and `video` from #63666).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobKind {
    Shell { shell_id: String, command: String },
    Subagent { child_session_id: String, agent: String, description: String },
    Image { path: String, model: String },
    Video { path: String },
    CodeMode,
    /// Future job kind from a newer server; tolerated without dropping the job record.
    Unknown { kind: String },
}

impl JobKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Shell { .. } => "shell",
            Self::Subagent { .. } => "subagent",
            Self::Image { .. } => "image",
            Self::Video { .. } => "video",
            Self::CodeMode => "codemode",
            Self::Unknown { kind } => kind.as_str(),
        }
    }
}

/// Filter value for the `kind` query parameter on `GET /api/experimental/job`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKindFilter {
    Shell,
    Subagent,
    Image,
    Video,
    CodeMode,
}

impl JobKindFilter {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Subagent => "subagent",
            Self::Image => "image",
            Self::Video => "video",
            Self::CodeMode => "codemode",
        }
    }
}

/// Query parameters for `GET /api/experimental/job` (`Job.ListQuery`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobListQuery {
    pub session_id: Option<String>,
    pub kind: Option<JobKindFilter>,
    pub status: Option<JobStatus>,
    pub descendants: Option<bool>,
}

impl JobListQuery {
    /// Running jobs owned by `session_id` or any descendant subagent session.
    pub fn running_tree(session_id: impl Into<String>) -> Self {
        Self {
            session_id: Some(session_id.into()),
            kind: None,
            status: Some(JobStatus::Running),
            descendants: Some(true),
        }
    }
}

/// One background job reported by `GET /api/experimental/job` or `experimental.job.updated`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobInfo {
    pub id: String,
    pub session_id: String,
    pub status: JobStatus,
    pub title: Option<String>,
    pub time: JobTime,
    pub kind: JobKind,
}

impl JobInfo {
    /// The session whose tree owns this job: for a subagent job its child session, otherwise the
    /// session that started it (`requests/background.ts` #64790).
    pub fn owner(&self) -> &str {
        match &self.kind {
            JobKind::Subagent { child_session_id, .. } if !child_session_id.is_empty() => child_session_id,
            _ => &self.session_id,
        }
    }

    pub fn is_running(&self) -> bool {
        self.status.is_running()
    }

    pub fn is_codemode(&self) -> bool {
        matches!(self.kind, JobKind::CodeMode)
    }
}

/// Whether SSE event type `ty` is a background job update.
pub fn is_job_event(ty: &str) -> bool {
    ty == "experimental.job.updated"
}

/// Leniently parses one `Job.Info` JSON object; returns `None` only when `id` is missing or empty.
pub fn parse_job(v: &Value) -> Option<JobInfo> {
    let id = v["id"].as_str().filter(|id| !id.is_empty())?.to_owned();
    let session_id = s(&v["sessionID"]);
    let status = v["status"].as_str().map(JobStatus::parse).unwrap_or_else(|| JobStatus::Unknown(String::new()));
    let title = v["title"].as_str().filter(|t| !t.is_empty()).map(str::to_owned);
    let time = JobTime {
        started: i(&v["time"]["started"]),
        completed: opt_i(&v["time"]["completed"]),
    };
    let kind = match v["kind"].as_str().unwrap_or("") {
        "shell" => JobKind::Shell { shell_id: s(&v["shellID"]), command: s(&v["command"]) },
        "subagent" => JobKind::Subagent {
            child_session_id: s(&v["childSessionID"]),
            agent: s(&v["agent"]),
            description: s(&v["description"]),
        },
        "image" => JobKind::Image { path: s(&v["path"]), model: s(&v["model"]) },
        "video" => JobKind::Video { path: s(&v["path"]) },
        "codemode" => JobKind::CodeMode,
        other => JobKind::Unknown { kind: other.to_owned() },
    };
    Some(JobInfo { id, session_id, status, title, time, kind })
}

/// Parses the array returned by `GET /api/experimental/job` (`{ data: [...] }` or `[...]`).
pub fn parse_jobs(v: &Value) -> Vec<JobInfo> {
    v["data"]
        .as_array()
        .or_else(|| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(parse_job)
        .collect()
}

/// Parses an `experimental.job.updated` event's `data` payload (`{ info: Job.Info }`).
pub fn parse_job_updated(data: &Value) -> Option<JobInfo> {
    let info = data.get("info").filter(|v| v.is_object()).unwrap_or(data);
    parse_job(info)
}

fn opt_i(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

/// Applies `experimental.job.updated` events that arrived while `GET /api/experimental/job` was
/// in flight on top of its response, which may predate them (`mergeJobList` in
/// `packages/app/src/session/requests/background.ts`, anomalyco/opencode#64966).
pub fn merge_job_list(
    listed: impl IntoIterator<Item = JobInfo>,
    updates: &BTreeMap<String, JobInfo>,
) -> BTreeMap<String, JobInfo> {
    let mut jobs: BTreeMap<String, JobInfo> = listed
        .into_iter()
        .filter(JobInfo::is_running)
        .map(|job| (job.id.clone(), job))
        .collect();
    for (id, info) in updates {
        if info.is_running() {
            jobs.insert(id.clone(), info.clone());
        } else {
            jobs.remove(id);
        }
    }
    jobs
}

/// Request phase for [`JobTracker`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum JobLoadPhase {
    #[default]
    Idle,
    /// A `GET /api/experimental/job` call is in flight; `updates` buffers every
    /// `experimental.job.updated` event seen since it started so a stale HTTP snapshot cannot
    /// resurrect a job that already settled (`anomalyco/opencode#64966`).
    Loading {
        attempt: u64,
        session: String,
        updates: BTreeMap<String, JobInfo>,
    },
    /// This server answered 404/405/501 to `/api/experimental/job` on this connection; skip
    /// further list requests until [`JobTrackerEvent::Reset`] (reconnect or server switch).
    Unsupported,
}

/// Explicit state machine tracking running background jobs across HTTP list loads and live SSE
/// `experimental.job.updated` events (`packages/app/src/session/requests/model.ts`, #64966).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobTracker {
    running: BTreeMap<String, JobInfo>,
    phase: JobLoadPhase,
    issued: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobTrackerEvent {
    /// A session opened, or the stream reconnected while it was open.
    Opened { session: String },
    /// Live `experimental.job.updated` event.
    Updated(JobInfo),
    /// `GET /api/experimental/job` answered with jobs for `attempt`.
    Listed { attempt: u64, jobs: Vec<JobInfo> },
    /// `GET /api/experimental/job` answered 404/405/501 (older server without job endpoints).
    Unsupported { attempt: u64 },
    /// `GET /api/experimental/job` failed transiently (network error, timeout, missing session).
    ListFailed { attempt: u64 },
    /// Connection reconnected or active server changed: clear cached jobs and probe again.
    Reset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobTrackerEffect {
    List { attempt: u64, session: String },
}

impl JobTracker {
    pub fn running(&self) -> impl Iterator<Item = &JobInfo> {
        self.running.values()
    }

    pub fn get(&self, id: &str) -> Option<&JobInfo> {
        self.running.get(id)
    }

    pub fn is_empty(&self) -> bool {
        self.running.is_empty()
    }

    pub fn phase(&self) -> &JobLoadPhase {
        &self.phase
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.phase, JobLoadPhase::Loading { .. })
    }

    pub fn is_unsupported(&self) -> bool {
        matches!(self.phase, JobLoadPhase::Unsupported)
    }

    /// Whether applying `info` would mutate running state, buffer an in-flight update, or exit
    /// `Unsupported`. Lets callers avoid dirtying tracked slots on duplicate no-op events.
    pub fn would_change(&self, info: &JobInfo) -> bool {
        if !matches!(self.phase, JobLoadPhase::Idle) {
            return true;
        }
        match self.running.get(&info.id) {
            Some(prev) if info.is_running() => prev != info,
            Some(_) => true,
            None => info.is_running(),
        }
    }

    pub fn step(&mut self, event: JobTrackerEvent) -> Vec<JobTrackerEffect> {
        match event {
            JobTrackerEvent::Opened { session } => {
                if matches!(self.phase, JobLoadPhase::Unsupported) {
                    return Vec::new();
                }
                self.issued += 1;
                let attempt = self.issued;
                self.phase = JobLoadPhase::Loading { attempt, session: session.clone(), updates: BTreeMap::new() };
                vec![JobTrackerEffect::List { attempt, session }]
            }
            JobTrackerEvent::Updated(info) => {
                if matches!(self.phase, JobLoadPhase::Unsupported) {
                    self.phase = JobLoadPhase::Idle;
                }
                if let JobLoadPhase::Loading { updates, .. } = &mut self.phase {
                    updates.insert(info.id.clone(), info.clone());
                }
                if info.is_running() {
                    self.running.insert(info.id.clone(), info);
                } else {
                    self.running.remove(&info.id);
                }
                Vec::new()
            }
            JobTrackerEvent::Listed { attempt, jobs } => {
                let JobLoadPhase::Loading { attempt: active, updates, .. } = &mut self.phase else {
                    return Vec::new();
                };
                if *active != attempt {
                    return Vec::new();
                }
                let buffered = std::mem::take(updates);
                self.running = merge_job_list(jobs, &buffered);
                self.phase = JobLoadPhase::Idle;
                Vec::new()
            }
            JobTrackerEvent::Unsupported { attempt } => {
                if matches!(self.phase, JobLoadPhase::Loading { attempt: active, .. } if active == attempt) {
                    self.running.clear();
                    self.phase = JobLoadPhase::Unsupported;
                }
                Vec::new()
            }
            JobTrackerEvent::ListFailed { attempt } => {
                if matches!(self.phase, JobLoadPhase::Loading { attempt: active, .. } if active == attempt) {
                    self.phase = JobLoadPhase::Idle;
                }
                Vec::new()
            }
            JobTrackerEvent::Reset => {
                self.running.clear();
                self.phase = JobLoadPhase::Idle;
                Vec::new()
            }
        }
    }
}

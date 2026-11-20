//! Experimental background job endpoints (`packages/protocol/src/groups/job.ts`):
//! `GET /api/experimental/job` and `POST /api/experimental/job/:id/cancel`.
//!
//! Older OpenCode servers (`v2.2.6`, `v2.3.0`, and pre-17 Nov `dev`) do not expose these routes
//! and answer `404` / `405` / `501`. Both calls return [`JobEndpoint::Unsupported`] for those
//! statuses so callers can quietly fall back without raising an error toast, while preserving
//! typed [`WireError`]s for auth, captive portals, and transport failures.

use super::{Server, encode_component};
use crate::model::{self, JobInfo, JobListQuery};
use crate::wire_error::{WireError, WireKind};

/// Outcome of calling an experimental job route that older servers may not implement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobEndpoint<T> {
    Supported(T),
    /// Server answered 404/405/501 (not `SessionNotFoundError`): route is absent on this server.
    Unsupported,
}

impl<T> JobEndpoint<T> {
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::Supported(_))
    }

    pub fn into_option(self) -> Option<T> {
        match self {
            Self::Supported(value) => Some(value),
            Self::Unsupported => None,
        }
    }
}

/// Whether `err` means the server does not expose `/api/experimental/job*`.
///
/// A `404` tagged `SessionNotFoundError` comes from `JobHandler.list` on a server that DOES
/// support the route when the queried session no longer exists, so it is not treated as
/// unsupported. Captive-portal redirects/HTML (`WireKind::Redirected`, `NotOpenCode`), `401`
/// sign-out, and transport failures also remain typed errors.
pub fn is_job_endpoint_unsupported(err: &WireError) -> bool {
    match &err.kind {
        WireKind::Status { status: 404, tag, .. } => tag.as_deref() != Some("SessionNotFoundError"),
        WireKind::Status { status: 405 | 501, .. } => true,
        _ => false,
    }
}

impl Server {
    /// `GET /api/experimental/job` (`experimental.job.list`).
    pub fn list_jobs(&self, query: &JobListQuery) -> Result<JobEndpoint<Vec<JobInfo>>, WireError> {
        let path = job_list_path(query);
        match self.get(&path) {
            Ok(v) => Ok(JobEndpoint::Supported(model::parse_jobs(&v))),
            Err(e) if is_job_endpoint_unsupported(&e) => Ok(JobEndpoint::Unsupported),
            Err(e) => Err(e),
        }
    }

    /// `POST /api/experimental/job/:id/cancel` (`experimental.job.cancel`, 204 No Content).
    pub fn cancel_job(&self, id: &str) -> Result<JobEndpoint<()>, WireError> {
        let path = format!("/api/experimental/job/{}/cancel", encode_component(id));
        match self.write("POST", &path, None) {
            Ok(_) => Ok(JobEndpoint::Supported(())),
            Err(e) if is_job_endpoint_unsupported(&e) => Ok(JobEndpoint::Unsupported),
            Err(e) => Err(e),
        }
    }
}

fn job_list_path(query: &JobListQuery) -> String {
    let mut params = Vec::new();
    if let Some(session_id) = &query.session_id {
        params.push(format!("sessionID={}", encode_component(session_id)));
    }
    if let Some(kind) = query.kind {
        params.push(format!("kind={}", encode_component(kind.as_str())));
    }
    if let Some(status) = &query.status {
        params.push(format!("status={}", encode_component(status.as_str())));
    }
    if let Some(descendants) = query.descendants {
        params.push(format!("descendants={}", if descendants { "true" } else { "false" }));
    }
    if params.is_empty() {
        "/api/experimental/job".to_owned()
    } else {
        format!("/api/experimental/job?{}", params.join("&"))
    }
}

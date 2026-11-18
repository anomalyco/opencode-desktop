//! Off-thread server jobs scheduled by `opencode-feature-projects` (`ProjectsJob`,
//! `RemoteRequest`, `ProjectsReply`) and applied back on the UI thread via [`apply_reply`].

use opencode_kit::client::Server;
use opencode_kit::model::{SessionInfo, parse_session};
use opencode_kit::wire_error::WireError;
use rune::Tx;
use rune::machine::Attempt;
use serde_json::{Map, Value};

use crate::host::ProjectsHost;
use crate::import;
use crate::remote_picker::{self, Dir};

/// Background server request dispatched by [`ProjectsHost::spawn_job`].
#[derive(Clone, Debug)]
pub enum ProjectsJob {
    /// `POST /api/experimental/session/import` against the window's active live server.
    ImportSession {
        attempt: Attempt,
        payload: Map<String, Value>,
    },
    /// Request against an explicit [`Server`] connection (the remote folder picker may browse
    /// a server other than the window's checked-out one).
    Remote {
        server: Server,
        request: RemoteRequest,
    },
}

/// Off-thread request for the remote directory picker dialog.
#[derive(Clone, Debug)]
pub enum RemoteRequest {
    /// `GET /api/location` to infer the server's home and initial directory.
    Locate { attempt: Attempt },
    /// `GET /api/fs/list` for subdirectories under `dir`.
    ListDirs { dir: String, attempt: Attempt },
}

/// Result of a [`ProjectsJob`], applied on the UI thread by [`apply_reply`].
#[derive(Clone, Debug)]
pub enum ProjectsReply {
    SessionImported {
        attempt: Attempt,
        result: Box<Result<SessionInfo, WireError>>,
    },
    RemoteLocated {
        attempt: Attempt,
        home: Option<String>,
        location: Option<String>,
    },
    RemoteDirsListed {
        dir: String,
        attempt: Attempt,
        dirs: Option<Vec<Dir>>,
    },
}

/// Executes a session-import upload off the UI thread against `srv`.
pub fn execute_import(srv: &Server, attempt: Attempt, payload: Map<String, Value>) -> ProjectsReply {
    let result = srv
        .write("POST", "/api/experimental/session/import", Some(Value::Object(payload)))
        .map(|v| parse_session(v.get("data").unwrap_or(&v)));
    ProjectsReply::SessionImported { attempt, result: Box::new(result) }
}

/// Executes a remote directory picker request off the UI thread against `srv`.
pub fn execute_remote(srv: &Server, request: RemoteRequest) -> ProjectsReply {
    match request {
        RemoteRequest::Locate { attempt } => {
            // `bootstrapGlobal`: the home is inferred from the server's location or its project's.
            let v = srv.get("/api/location").ok();
            let location = v.as_ref().and_then(|v| field_text(v, &["directory"]));
            let home = v.as_ref().and_then(|v| {
                [location.clone(), field_text(&v["project"], &["directory"])]
                    .into_iter()
                    .flatten()
                    .map(|d| remote_picker::infer_home(&d))
                    .find(|h| !h.is_empty())
            });
            ProjectsReply::RemoteLocated { attempt, home, location }
        }
        RemoteRequest::ListDirs { dir, attempt } => {
            let dirs = remote_picker::read_dirs(srv, &dir);
            ProjectsReply::RemoteDirsListed { dir, attempt, dirs }
        }
    }
}

/// Executes any [`ProjectsJob`] off the UI thread, returning `None` if an active-server job has no
/// live server handle.
pub fn execute_job(active_server: Option<&Server>, job: ProjectsJob) -> Option<ProjectsReply> {
    match job {
        ProjectsJob::ImportSession { attempt, payload } => Some(execute_import(active_server?, attempt, payload)),
        ProjectsJob::Remote { server, request } => Some(execute_remote(&server, request)),
    }
}

/// Applies a completed [`ProjectsReply`] back onto `host` on the UI thread.
pub fn apply_reply(host: &mut dyn ProjectsHost, reply: ProjectsReply, tx: &mut Tx) {
    match reply {
        ProjectsReply::SessionImported { attempt, result } => import::apply_uploaded(host, attempt, *result, tx),
        ProjectsReply::RemoteLocated { attempt, home, location } => remote_picker::apply_located(host, attempt, home, location, tx),
        ProjectsReply::RemoteDirsListed { dir, attempt, dirs } => remote_picker::apply_listed(host, dir, attempt, dirs, tx),
    }
}

fn field_text(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| v[*k].as_str().filter(|t| !t.is_empty()).map(str::to_owned))
}

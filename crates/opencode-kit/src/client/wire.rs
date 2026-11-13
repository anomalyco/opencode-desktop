//! What goes over the connection's channels: reads asked of the fetch thread (`Request`),
//! writes for the write thread (`Write`), and what comes back to the app (`Incoming`).

use serde_json::Value;

use super::Prompt;
use crate::model::{self, Delivery, Message, Pending, SessionInfo};
use crate::wire_error::WireError;

#[derive(Clone)]
pub enum Incoming {
    Sessions(Vec<SessionInfo>),
    Info(SessionInfo),
    /// Every direct subagent session of `parent` (the reply to `Request::Children`).
    Children { parent: String, sessions: Vec<SessionInfo> },
    Messages { session: String, messages: Vec<Message> },
    Event { ty: String, data: Value },
    /// A line for the debug bar or a toast that isn't a failure (`Saved session to …`).
    Status(String),
    /// The event stream dropped, or failed to open, with what it failed with. Its reader tries
    /// again on its own.
    StreamDown(WireError),
    /// After a network change or a wake, the stream's server didn't answer a check
    /// (`streams::verify`): it's offline now, whatever its stream looked like.
    Unanswered(WireError),
    /// A read or write that failed, as the client knows it: the call and what answered, typed.
    /// The UI builds what it says from this, never from text.
    Failed(WireError),
    /// A prompt write (`Write::Send`, `SendMovingHome`, `SendFromRevert`, `StartAs`) that failed,
    /// with the id the prompt was sent under and its error.
    SendFailed { session: String, id: String, error: WireError },
    /// A rename, delete or restore this window sent (`client.session.write`'s `data`), and its
    /// error when it failed.
    SessionWrite { data: Value, error: Option<WireError> },
    /// A session's open permission requests and forms.
    Pending { session: String, items: Vec<Pending> },
    /// A session this app just created (its first prompt is already sent).
    Created(SessionInfo),
    /// The event stream is back after a drop: anything may have changed in the gap.
    Reconnected,
    /// The event stream opened for the first time (`server.connected`): the server is alive for as
    /// long as it stays up, so its health needn't be polled.
    Opened,
    /// The sessions running right now (the reply to `Request::Active`).
    Active(Vec<String>),
    /// The server's project list (`/api/project`).
    Projects(Vec<model::Project>),
    /// A folder resolved and registered as a project (`Write::OpenProject`), with the updated
    /// project list and, when `start_session` was set, the new session opened in it.
    ProjectOpened { location: model::LocationInfo, projects: Vec<model::Project>, session: Option<SessionInfo> },
    /// A `Write::Tracked` write's outcome, under the ticket it was sent with: `Ok(Some(session))`
    /// for a fork (the session it made), `Ok(None)` for any other write, or the raw error.
    Settled { ticket: Ticket, result: Result<Option<SessionInfo>, WireError> },
}

/// Which machine asked for a tracked write, for which session, at which attempt: the write's
/// `Incoming::Settled` comes back with it, so a result for an attempt that is no longer current
/// is dropped by the machine rather than parsed out of an error string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub kind: TicketKind,
    pub session: String,
    pub attempt: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TicketKind {
    /// `Write::Interrupt` from Stop.
    Stop,
    /// A rewind, undo or redo (`RevertStage`, `RevertClear`).
    Revert,
    /// `Write::Fork`.
    Fork,
    /// A queued or pending prompt's steer, move, removal or undo (`Deliver`, `Cancel`).
    Inbox,
}

pub enum Request {
    Session(String),
    Sessions,
    /// The server's project list (`/api/project`).
    Projects,
    /// One session's info (`time.idle`/`time.viewed` after it finishes or someone views it).
    Info(String),
    /// [`Request::Info`] for a session named in a message, which may not exist: replied as
    /// `Incoming::Info`, or `Incoming::Event` with type `session.lookup.missing` and data
    /// `{sessionID}` when the server can't give it.
    Lookup(String),
    Pending(String),
    /// A project's newest unarchived sessions, subagents included (`/api/session?project=`),
    /// replied as `Incoming::Sessions`.
    ProjectSessions(String),
    /// A session's undelivered prompts. The reply is `Incoming::Event` with type
    /// `session.inbox.listed` and data `{sessionID, items}` (see [`model::Inbox::apply`]).
    Inbox(String),
    /// Which sessions are running (`/api/session/active`).
    Active,
    /// Whether `experimental.background_compaction` is on in a directory (`/api/config`). The
    /// reply is `Incoming::Event` with type `config.listed` and data
    /// `{directory, backgroundCompaction}`.
    Config(String),
    /// A project's worktree inventory (`GET /api/worktree?projectID=`), the project's `root`
    /// left out. The reply is `Incoming::Event` with type `worktree.listed` and data
    /// `{projectID, directories}`.
    Worktrees { project: String, root: String },
    /// The branches at `directory` matching `search` (`GET /api/vcs/branch`, 50 at most). The
    /// reply is `Incoming::Event` with type `vcs.branch.listed` and data
    /// `{directory, search, branches}`, or with `failed: true`.
    Branches { directory: String, search: String },
    /// Every direct subagent session of a session, page after page (`GET /api/session?parentID=`,
    /// as Desktop's `listChildren`), replied as `Incoming::Children`: a root's cost counts them.
    Children(String),
}

pub enum Write {
    Prompt { session: String, text: String },
    /// A permission's reply. Once the server takes it, `Incoming::Event` with type
    /// `client.request.replied` and data `{ requestID }` follows (also for `Form` and
    /// `CancelForm`, with the form's id); a failure reports `Incoming::Status`.
    Permission { session: String, request: String, decision: String },
    Form { session: String, form: String, answer: Value },
    CancelForm { session: String, form: String },
    /// Stop: interrupts the session's run, which goes on with the steers waiting
    /// (`Server::stop`). `Incoming::Event` `session.interrupt.acknowledged` follows.
    Interrupt(String),
    /// A new session in `directory`, then `text` as its first prompt.
    Create { directory: String, text: String },
    /// `Prompt` with its client id and files.
    Send { session: String, prompt: Prompt },
    /// `Send` that moves a session whose folder is gone to its project's main checkout first
    /// (`Server::send_moving_home`).
    SendMovingHome { session: String, prompt: Prompt },
    /// `Create` with a full first prompt.
    Start { directory: String, prompt: Prompt },
    /// `Start` under an id the app chose (`session_id`), so it can show the session before the
    /// server has it: one create carrying the prompt's agent and model, then the prompt.
    /// `Incoming::Created` follows once both land. Failure reports `Incoming::SendFailed`
    /// with the prompt's id; ambiguous replies leave the session intact for a receipt check.
    StartAs { session: String, directory: String, prompt: Prompt },
    /// Withdraws an undelivered prompt (`DELETE /api/session/:id/inbox/:item`).
    Cancel { session: String, item: String },
    /// Moves an undelivered prompt between the queue and steering (`PATCH …/inbox/:item`).
    Deliver { session: String, item: String, delivery: Delivery },
    /// Runs one shell command in the session's directory (`POST …/shell`).
    Shell { session: String, id: Option<String>, command: String },
    /// Renames a session (`PATCH /api/session/:id`, as Desktop's `session.update`).
    Rename { session: String, title: String },
    /// Deletes a session and its subagents (`DELETE /api/session/:id`).
    Delete(String),
    /// Archives a session (`PATCH /api/session/:id` with `{ archived: true }`).
    Archive(String),
    /// Brings an archived session back (`PATCH /api/session/:id` with `{ archived: false }`).
    Unarchive(String),
    /// Marks a session read up to its `idle` (`POST /api/session/:id/view`).
    View { session: String, idle: i64 },
    /// Replaces a session's `metadata` (`PATCH /api/session/:id`).
    Metadata { session: String, metadata: Value },
    /// Saves a session as JSON in the downloads folder, as Desktop's Export does.
    Export(String),
    /// Moves the session's blocking shells and subagents to the background (`POST …/background`).
    /// `quiet` drops a failure rather than reporting it, as a steer's move does.
    Background { session: String, quiet: bool },
    /// Summarizes the session to shrink its context (`POST …/compact`, Desktop's Compact session).
    Compact(String),
    /// Rebuilds every location from its config (`POST /api/location/reload`, Desktop's `/reload`).
    Reload,
    /// Resolves `directory` (`/api/location`, registering its project on the server), reloads
    /// `/api/project`, and when `start_session` is set creates a session in `directory`.
    OpenProject { directory: String, start_session: bool },
    /// Stages a revert to `message` (`POST /api/session/:id/revert/stage`), settling active work
    /// first (`interrupt` + `wait`) when `settle` is true, as Desktop's `revert.stage` does.
    RevertStage { session: String, message: String, settle: bool },
    /// Clears a staged revert (`DELETE /api/session/:id/revert`), as Desktop's `revert.redo` and
    /// toast `Undo` do.
    RevertClear(String),
    /// Commits a staged revert (`POST /api/session/:id/revert/commit`).
    RevertCommit(String),
    /// Sends `prompt` while `session` has a staged revert, committing the revert after the agent
    /// switch and before the model switch and admission, as Desktop's `composer/submit.ts` does.
    SendFromRevert { session: String, prompt: Prompt },
    /// Forks `session` before `before` (`POST /api/session/:id/fork`); tracked, it settles with
    /// the session it made.
    Fork { session: String, before: Option<String> },
    /// `write`, whose outcome comes back as `Incoming::Settled` under `ticket`, success or not
    /// (no `Incoming::Status` for its failure).
    Tracked { ticket: Ticket, write: Box<Write> },
    /// `write` (a prompt's), held on the write lane until `gate` says the outbox row that keeps it
    /// is durable. The lane makes the request meanwhile (reads only, nothing changes on the
    /// server), sends it on `true` and drops it on `false` (the app hears why from its storage).
    #[cfg(not(target_arch = "wasm32"))]
    Gated { gate: std::sync::mpsc::Receiver<bool>, write: Box<Write> },
    /// `StartAs` in a new worktree of `project` (Desktop's `createWorktree`): `POST /api/worktree`
    /// from the checkout `from`, on `branch` or its current one, then the session in the
    /// worktree it made. `Incoming::Event` `worktree.updated` follows the worktree, then
    /// `Incoming::Created`; a failure reports `Incoming::SendFailed` with the prompt's id.
    StartInNewWorktree { session: String, project: String, from: String, branch: Option<String>, prompt: Prompt },
}

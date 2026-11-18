//! The permission dock's reply orchestration (`permission-machine.ts`) and a question form's
//! reply orchestration (`question::reply`, `question-machine.ts`). A reply the server refuses,
//! or one that takes too long, gives the dock back with a toast.

use std::collections::HashMap;
use std::time::Duration;

use opencode_kit::client::Write;
use opencode_kit::model::Pending;
use opencode_kit::wire_error::WireError;
use rune::machine::{Attempt, Effects};
use serde_json::{Value, json};

use crate::question::reply::{self as form, Action};

pub mod reply;

/// Identifies which dock reply state machine timed out after its request deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyTimeoutKind {
    Permission,
    Form,
}

/// The replies the docks have sent or are sending, by request id.
#[derive(Default)]
pub struct Sent(pub HashMap<String, Request>);

pub struct Request {
    kind: Kind,
    /// Answers the worker still owes for replies called off (`Abort`): they're dropped when
    /// they come, as Desktop's aborted fetch never answers.
    owed: u32,
}

enum Kind {
    Permission(reply::Reply),
    Form(form::Reply),
}

impl Sent {
    /// Whether request `id`'s reply is on its way or taken (`isPermissionResponding`,
    /// `isQuestionBusy`): the dock's buttons are off.
    pub fn busy(&self, id: &str) -> bool {
        self.0.get(id).is_some_and(|r| match &r.kind {
            Kind::Permission(reply) => !matches!(reply, reply::Reply::Idle { .. }),
            Kind::Form(reply) => form::busy(reply),
        })
    }

    /// Whether permission `id` is answered (`isPermissionReplied`): it doesn't come back before
    /// the server's `permission.replied` drops it.
    pub fn replied(&self, id: &str) -> bool {
        self.0.get(id).is_some_and(|r| matches!(r.kind, Kind::Permission(reply::Reply::Replied)))
    }

    /// Whether request `id` has an active or answered reply entry.
    pub fn contains(&self, id: &str) -> bool {
        self.0.contains_key(id)
    }

    /// Removes request `id` once settled by the server or forgotten locally.
    pub fn remove(&mut self, id: &str) -> bool {
        self.0.remove(id).is_some()
    }

    /// Drops all tracked reply entries (e.g. when replaying a storybook scene).
    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

/// Object-safe host boundary for permission and question-form reply orchestration.
pub trait PermissionHost {
    fn permission_sent(&self) -> &Sent;
    fn permission_sent_mut(&mut self) -> &mut Sent;

    /// Returns the session ID owning pending request `id`, if `id` is pending.
    fn pending_session(&self, id: &str) -> Option<String>;

    /// Currently visible permission request (none when a question is open or auto-accept is on).
    fn permission_asked(&self) -> Option<Pending> {
        None
    }

    /// Active dock face id: open question form id, else visible permission id.
    fn dock_face(&self) -> Option<String> {
        self.permission_asked().map(|p| p.id().to_owned())
    }

    /// Whether a reply can leave now: a live server that takes writes.
    fn writes_open(&self) -> bool;

    /// Whether permission replies succeed immediately without a server (storybook stage or fixture).
    fn answers_permission_at_once(&self) -> bool {
        false
    }

    /// Whether form replies settle immediately without a server (storybook stage).
    fn settles_form_at_once(&self) -> bool {
        false
    }

    /// Dispatches a server write; returns `false` when disconnected or unable to send.
    fn write(&mut self, write: Write) -> bool;

    /// Schedules a timeout callback after `delay` that invokes [`reply_timed_out`].
    fn schedule_reply_timeout(&mut self, kind: ReplyTimeoutKind, id: String, attempt: Attempt, delay: Duration);

    /// Shows a request-failure toast.
    fn toast_fail(&mut self, title: String, description: String);

    /// Removes an answered permission from pending lists and marks its notification answered,
    /// while keeping its `Replied` entry in [`Sent`] until `permission.replied` arrives.
    fn on_permission_replied(&mut self, id: &str);

    /// Forgets request `id` completely (pending, requests, question answers, sent entry, notifications).
    fn forget_request(&mut self, id: &str);

    /// Formats a server [`WireError`] into a user-facing reason.
    fn format_wire_error(&self, err: &WireError) -> String {
        err.to_string()
    }
}

/// The permission the open page shows: none while a question stands in the composer's place
/// or auto-accept answers them.
pub fn permission_asked(host: &dyn PermissionHost) -> Option<Pending> {
    host.permission_asked()
}

/// What stands in the composer's place, by id: the question's form, else a permission.
pub fn dock_face(host: &dyn PermissionHost) -> Option<String> {
    host.dock_face()
}

/// Whether a permission or question dock currently stands in the composer's place.
pub fn dock_shown(host: &dyn PermissionHost) -> bool {
    host.dock_face().is_some()
}

/// The dock's Deny, Allow always or Allow once (`decide`).
pub fn reply_permission(host: &mut dyn PermissionHost, id: &str, decision: &str) {
    let Some(session) = host.pending_session(id) else { return };
    if !host.answers_permission_at_once() && !host.writes_open() {
        // Says why nothing can be sent.
        host.write(Write::Permission { session, request: id.to_owned(), decision: decision.to_owned() });
        return;
    }
    let r = host
        .permission_sent_mut()
        .0
        .entry(id.to_owned())
        .or_insert(Request { kind: Kind::Permission(reply::Reply::Idle { attempt: Attempt(0) }), owed: 0 });
    let Kind::Permission(state) = &mut r.kind else { return };
    let effects = reply::step(state, reply::Event::Decide { session: &session, decision });
    permission_effects(host, id, effects);
}

/// Sends form `id`'s answers, or dismisses it.
pub fn send_form(host: &mut dyn PermissionHost, id: &str, action: Action) {
    let Some(session) = host.pending_session(id) else { return };
    if !host.settles_form_at_once() && !host.writes_open() {
        // Says why nothing can be sent.
        host.write(Write::CancelForm { session, form: id.to_owned() });
        return;
    }
    let r = host
        .permission_sent_mut()
        .0
        .entry(id.to_owned())
        .or_insert(Request { kind: Kind::Form(form::Reply::Answering { attempt: Attempt(0) }), owed: 0 });
    let Kind::Form(state) = &mut r.kind else { return };
    let effects = form::step(state, form::Event::Send(action));
    form_effects(host, id, &session, effects);
}

/// Applies a scheduled reply timeout for `kind` / `id` / `attempt`.
pub fn reply_timed_out(host: &mut dyn PermissionHost, kind: ReplyTimeoutKind, id: &str, attempt: Attempt) {
    match kind {
        ReplyTimeoutKind::Permission => permission_step(host, id, reply::Event::TimedOut { attempt }),
        ReplyTimeoutKind::Form => form_step(host, id, form::Event::TimedOut { attempt }),
    }
}

/// The worker's answer to request `id`'s reply: taken, or refused with `error`. A
/// permission the server no longer has counts as answered (`isPermissionNotFoundError`).
pub fn reply_answered(host: &mut dyn PermissionHost, id: &str, result: Result<(), &str>) {
    if !host.permission_sent().contains(id) {
        return;
    }
    let pending_step = {
        let Some(r) = host.permission_sent_mut().0.get_mut(id) else { return };
        if r.owed > 0 {
            r.owed -= 1;
            return;
        }
        match &r.kind {
            Kind::Permission(reply::Reply::Responding { attempt, .. }) => Some((ReplyTimeoutKind::Permission, *attempt)),
            Kind::Form(form::Reply::Sending { attempt, .. }) => Some((ReplyTimeoutKind::Form, *attempt)),
            _ => None,
        }
    };
    match pending_step {
        Some((ReplyTimeoutKind::Permission, attempt)) => {
            let event = match result {
                Err(error) if !error.contains("404") => reply::Event::Failed { attempt, error },
                _ => reply::Event::Succeeded { attempt },
            };
            permission_step(host, id, event);
        }
        Some((ReplyTimeoutKind::Form, attempt)) => {
            let event = match result {
                Ok(()) => form::Event::Succeeded { attempt },
                Err(error) => form::Event::Failed { attempt, error },
            };
            form_step(host, id, event);
        }
        None => {}
    }
}

/// Extracts the permission/form request id from a failed reply or cancel wire error path.
pub fn failed_request_id(e: &WireError) -> Option<&str> {
    let path = e.path.as_str();
    match e.method {
        "POST" => path
            .strip_prefix("/api/session/")
            .and_then(|r| r.strip_suffix("/reply"))
            .and_then(|r| r.split_once("/permission/").or_else(|| r.split_once("/form/"))),
        "DELETE" => path.strip_prefix("/api/session/").and_then(|r| r.split_once("/form/")),
        _ => None,
    }
    .map(|(_, id)| id)
}

/// A failed write from the server's worker: a dock's reply the server refused. False when it
/// isn't one.
pub fn failed(host: &mut dyn PermissionHost, e: &WireError) -> bool {
    let Some(id) = failed_request_id(e).filter(|id| host.permission_sent().contains(id)).map(str::to_owned) else {
        return false;
    };
    let why = host.format_wire_error(e);
    reply_answered(host, &id, Err(&why));
    true
}

/// Sample `permission.asked` payload used by the `"permission"` scene.
pub fn scene_permission_ask(session_id: &str) -> Value {
    json!({
        "id": "per_scene_bash",
        "sessionID": session_id,
        "action": "bash",
        "resources": ["bun test test/net/backoff.test.ts"],
    })
}

fn permission_step(host: &mut dyn PermissionHost, id: &str, event: reply::Event<'_>) {
    let Some(Request { kind: Kind::Permission(state), .. }) = host.permission_sent_mut().0.get_mut(id) else {
        return;
    };
    let effects = reply::step(state, event);
    permission_effects(host, id, effects);
}

fn permission_effects(host: &mut dyn PermissionHost, id: &str, effects: Effects<reply::Effect>) {
    for effect in effects {
        match effect {
            reply::Effect::Send { attempt, session, decision } => {
                if host.answers_permission_at_once() {
                    permission_step(host, id, reply::Event::Succeeded { attempt });
                    continue;
                }
                if !host.write(Write::Permission { session, request: id.to_owned(), decision }) {
                    permission_step(host, id, reply::Event::Failed { attempt, error: opencode_i18n::t!("rune.send.disconnected") });
                    continue;
                }
                host.schedule_reply_timeout(ReplyTimeoutKind::Permission, id.to_owned(), attempt, Duration::from_secs_f64(reply::TIMEOUT_S));
            }
            reply::Effect::Abort => {
                if let Some(r) = host.permission_sent_mut().0.get_mut(id) {
                    r.owed += 1;
                }
            }
            reply::Effect::ToastFailed(error) => {
                host.toast_fail(opencode_i18n::t!("common.requestFailed").to_owned(), error.unwrap_or_default());
            }
        }
    }
    // An answered request leaves the dock at once, without waiting for `permission.replied`.
    if host.permission_sent().replied(id) {
        host.on_permission_replied(id);
    }
}

fn form_step(host: &mut dyn PermissionHost, id: &str, event: form::Event<'_>) {
    let Some(Request { kind: Kind::Form(state), .. }) = host.permission_sent_mut().0.get_mut(id) else {
        return;
    };
    let effects = form::step(state, event);
    let session = host.pending_session(id).unwrap_or_default();
    form_effects(host, id, &session, effects);
}

fn form_effects(host: &mut dyn PermissionHost, id: &str, session: &str, effects: Effects<form::Effect>) {
    for effect in effects {
        match effect {
            form::Effect::Send { attempt, action } => {
                // A story's form is settled at once, as the server would.
                if host.settles_form_at_once() {
                    host.forget_request(id);
                    return;
                }
                let w = match action {
                    Action::Reply(answer) => Write::Form { session: session.to_owned(), form: id.to_owned(), answer },
                    Action::Dismiss => Write::CancelForm { session: session.to_owned(), form: id.to_owned() },
                };
                if !host.write(w) {
                    form_step(host, id, form::Event::Failed { attempt, error: opencode_i18n::t!("rune.send.disconnected") });
                    continue;
                }
                host.schedule_reply_timeout(ReplyTimeoutKind::Form, id.to_owned(), attempt, Duration::from_secs_f64(form::TIMEOUT_S));
            }
            form::Effect::Abort => {
                if let Some(r) = host.permission_sent_mut().0.get_mut(id) {
                    r.owed += 1;
                }
            }
            form::Effect::ToastFailed(error) => {
                host.toast_fail(opencode_i18n::t!("common.requestFailed").to_owned(), error.unwrap_or_default());
            }
        }
    }
}

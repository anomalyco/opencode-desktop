//! The permission dock, drawn by `session_view::dock::permission` (Desktop's
//! `session-permission-dock.tsx`), and the replies the docks send: a permission's ([`reply`],
//! `permission-machine.ts`) and a question form's (`question::reply`, `question-machine.ts`). A
//! reply the server refuses, or one that takes too long, gives the dock back with a toast.

use std::collections::HashMap;
use std::time::Duration;

use opencode_kit::client::Write;
use opencode_kit::model::Pending;
use serde_json::{Value, json};
use session_view::dock::{self, permission};

use crate::question::reply::{self as form, Action};
use crate::{App, Route, Source};

pub mod reply;

/// The replies the docks have sent or are sending, by request id.
#[derive(Default)]
pub struct Sent(pub(crate) HashMap<String, Request>);

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
}

impl App {
    /// The permission the open page shows: none while a question stands in the composer's place
    /// or auto-accept answers them. A subagent's page shows its own and its subagents'.
    pub fn permission_asked(&self) -> Option<Pending> {
        permission::asked(self)
    }

    /// What stands in the composer's place, by id: the question's form, else a permission.
    pub fn dock_face(&self) -> Option<String> {
        dock::shown(self)
    }

    pub fn dock_shown(&self) -> bool {
        self.dock_face().is_some()
    }

    /// The dock's Deny, Allow always or Allow once (`decide`).
    pub fn reply_permission(&mut self, id: &str, decision: &str) {
        let Some(session) = self.pending.iter().find(|p| p.id() == id).map(|p| p.session().to_owned()) else { return };
        if !self.answers_at_once() && !self.writes_open() {
            // Says why nothing can be sent.
            self.write(Write::Permission { session, request: id.to_owned(), decision: decision.to_owned() });
            return;
        }
        let r = self.permission_sent.0.entry(id.to_owned()).or_insert(Request { kind: Kind::Permission(reply::Reply::Idle { attempt: rune::machine::Attempt(0) }), owed: 0 });
        let Kind::Permission(state) = &mut r.kind else { return };
        let effects = reply::step(state, reply::Event::Decide { session: &session, decision });
        self.permission_effects(id, effects);
    }

    /// Sends form `id`'s answers, or dismisses it.
    pub(crate) fn send_form(&mut self, id: &str, action: Action) {
        let Some(session) = self.pending.iter().find(|p| p.id() == id).map(|p| p.session().to_owned()) else { return };
        if self.stage.is_none() && !self.writes_open() {
            // Says why nothing can be sent.
            self.write(Write::CancelForm { session, form: id.to_owned() });
            return;
        }
        let r = self.permission_sent.0.entry(id.to_owned()).or_insert(Request { kind: Kind::Form(form::Reply::Answering { attempt: rune::machine::Attempt(0) }), owed: 0 });
        let Kind::Form(state) = &mut r.kind else { return };
        let effects = form::step(state, form::Event::Send(action));
        self.form_effects(id, &session, effects);
    }

    /// Whether a reply can leave now: a live server that takes writes. Otherwise [`App::write`]
    /// says why not.
    fn writes_open(&self) -> bool {
        matches!(&*self.source, Source::Live { allow_send: true, writes: Some(_), .. })
    }

    /// A story's or the fixture's permission is answered at once, as the server's
    /// `permission.replied` would.
    fn answers_at_once(&self) -> bool {
        self.stage.is_some() || matches!(*self.source, Source::Fixture)
    }

    fn permission_step(&mut self, id: &str, event: reply::Event<'_>) {
        let Some(Request { kind: Kind::Permission(state), .. }) = self.permission_sent.0.get_mut(id) else { return };
        let effects = reply::step(state, event);
        self.permission_effects(id, effects);
    }

    fn permission_effects(&mut self, id: &str, effects: rune::machine::Effects<reply::Effect>) {
        for effect in effects {
            match effect {
                reply::Effect::Send { attempt, session, decision } => {
                    if self.answers_at_once() {
                        self.permission_step(id, reply::Event::Succeeded { attempt });
                        continue;
                    }
                    if !self.write(Write::Permission { session, request: id.to_owned(), decision }) {
                        self.permission_step(id, reply::Event::Failed { attempt, error: opencode_i18n::t!("rune.send.disconnected") });
                        continue;
                    }
                    let id = id.to_owned();
                    self.job_after(Duration::from_secs_f64(reply::TIMEOUT_S), move |_| {
                        Box::new(move |s: &mut App, _: &mut rune::Tx| s.permission_step(&id, reply::Event::TimedOut { attempt })) as crate::jobs::Apply
                    });
                }
                reply::Effect::Abort => {
                    if let Some(r) = self.permission_sent.0.get_mut(id) {
                        r.owed += 1;
                    }
                }
                reply::Effect::ToastFailed(error) => crate::status_toast::fail(self, opencode_i18n::t!("common.requestFailed"), error.unwrap_or_default()),
            }
        }
        // An answered request leaves the dock at once, without waiting for `permission.replied`.
        if self.permission_sent.replied(id) {
            self.pending.retain(|p| p.id() != id);
            self.requests.retain(|(_, r)| r != id);
            crate::notifications::answered(self, id);
        }
    }

    fn form_step(&mut self, id: &str, event: form::Event<'_>) {
        let Some(Request { kind: Kind::Form(state), .. }) = self.permission_sent.0.get_mut(id) else { return };
        let effects = form::step(state, event);
        let session = self.pending.iter().find(|p| p.id() == id).map(|p| p.session().to_owned()).unwrap_or_default();
        self.form_effects(id, &session, effects);
    }

    fn form_effects(&mut self, id: &str, session: &str, effects: rune::machine::Effects<form::Effect>) {
        for effect in effects {
            match effect {
                form::Effect::Send { attempt, action } => {
                    // A story's form is settled at once, as the server would.
                    if self.stage.is_some() {
                        self.forget(id);
                        return;
                    }
                    let w = match action {
                        Action::Reply(answer) => Write::Form { session: session.to_owned(), form: id.to_owned(), answer },
                        Action::Dismiss => Write::CancelForm { session: session.to_owned(), form: id.to_owned() },
                    };
                    if !self.write(w) {
                        self.form_step(id, form::Event::Failed { attempt, error: opencode_i18n::t!("rune.send.disconnected") });
                        continue;
                    }
                    let id = id.to_owned();
                    self.job_after(Duration::from_secs_f64(form::TIMEOUT_S), move |_| {
                        Box::new(move |s: &mut App, _: &mut rune::Tx| s.form_step(&id, form::Event::TimedOut { attempt })) as crate::jobs::Apply
                    });
                }
                form::Effect::Abort => {
                    if let Some(r) = self.permission_sent.0.get_mut(id) {
                        r.owed += 1;
                    }
                }
                form::Effect::ToastFailed(error) => crate::status_toast::fail(self, opencode_i18n::t!("common.requestFailed"), error.unwrap_or_default()),
            }
        }
    }

    /// The worker's answer to request `id`'s reply: taken, or refused with `error`. A
    /// permission the server no longer has counts as answered (`isPermissionNotFoundError`).
    fn reply_answered(&mut self, id: &str, result: Result<(), &str>) {
        let Some(r) = self.permission_sent.0.get_mut(id) else { return };
        if r.owed > 0 {
            r.owed -= 1;
            return;
        }
        match &r.kind {
            Kind::Permission(reply::Reply::Responding { attempt, .. }) => {
                let attempt = *attempt;
                let event = match result {
                    Err(error) if !error.contains("404") => reply::Event::Failed { attempt, error },
                    _ => reply::Event::Succeeded { attempt },
                };
                self.permission_step(id, event);
            }
            Kind::Form(form::Reply::Sending { attempt, .. }) => {
                let attempt = *attempt;
                let event = match result {
                    Ok(()) => form::Event::Succeeded { attempt },
                    Err(error) => form::Event::Failed { attempt, error },
                };
                self.form_step(id, event);
            }
            _ => {}
        }
    }
}

/// The worker took a dock's reply (`client.request.replied`). False when it isn't that event.
pub fn event(s: &mut App, ty: &str, data: &Value) -> bool {
    if ty != "client.request.replied" {
        return false;
    }
    let id = data["requestID"].as_str().unwrap_or_default().to_owned();
    s.reply_answered(&id, Ok(()));
    true
}

/// A failed write from the server's worker: a dock's reply the server refused. False when it
/// isn't one.
pub fn failed(s: &mut App, e: &str) -> bool {
    let (path, why) = e.split_once(": ").unwrap_or((e, ""));
    let id = path
        .strip_prefix("POST /api/session/")
        .and_then(|r| r.strip_suffix("/reply"))
        .and_then(|r| r.split_once("/permission/").or_else(|| r.split_once("/form/")))
        .or_else(|| path.strip_prefix("DELETE /api/session/").and_then(|r| r.split_once("/form/")))
        .map(|(_, id)| id.to_owned());
    let Some(id) = id.filter(|id| s.permission_sent.0.contains_key(id)) else { return false };
    s.reply_answered(&id, Err(why));
    true
}

// The fixture session at rest; a bash permission arrives 0.6 s in (`--at 1.5` for a still).
rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("permission", |s| {
        crate::open_scene(s);
        s.finish_stream();
        let Route::Session { id, .. } = (*s.route).clone() else { return };
        let ask = json!({ "id": "per_scene_bash", "sessionID": id, "action": "bash", "resources": ["bun test test/net/backoff.test.ts"] });
        s.playback.push_back((*s.clock + 0.6, "permission.asked".into(), ask));
        s.wake_at_playback();
    }),
]);

mod stories;

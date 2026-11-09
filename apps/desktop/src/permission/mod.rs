//! The permission dock, drawn by `session_view::dock::permission` (Desktop's
//! `session-permission-dock.tsx`). A reply the server refuses brings the request back where it
//! stood, with a toast.

use std::collections::HashMap;

use opencode_kit::model::Pending;
use session_view::dock::{self, permission};
use serde_json::json;

use crate::{App, Route};

/// Replies on their way, by request id: where the request stood and what it was, to bring it
/// back if the server refuses the reply.
#[derive(Default)]
pub struct Sent(pub(crate) HashMap<String, (usize, Pending)>);

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

    /// Answers the request at once and keeps it, to bring it back if the server refuses.
    pub fn reply_permission(&mut self, id: &str, decision: &str) {
        let Some(at) = self.pending.iter().position(|p| p.id() == id) else { return };
        // The fixture answers at once, as the server's `permission.replied` would.
        if matches!(self.source, crate::Source::Fixture) && self.stage.is_none() {
            self.forget(id);
            return;
        }
        let p = self.pending[at].clone();
        let w = opencode_kit::client::Write::Permission { session: p.session().to_owned(), request: id.to_owned(), decision: decision.to_owned() };
        if self.write(w) {
            self.forget(id);
            self.permission_sent.0.insert(id.to_owned(), (at, p));
        }
    }
}

/// A failed write from the server's worker: a permission's reply puts the request back where it
/// stood, and the toast says so. False when it isn't a permission's.
pub fn failed(s: &mut App, e: &str) -> bool {
    let (path, why) = e.split_once(": ").unwrap_or((e, ""));
    let Some(rest) = path.strip_prefix("POST /api/session/").and_then(|r| r.strip_suffix("/reply")) else { return false };
    let Some((_, rid)) = rest.split_once("/permission/") else { return false };
    if let Some((at, p)) = s.permission_sent.0.remove(rid) {
        s.requests.insert((p.session().to_owned(), rid.to_owned()));
        if !s.pending.iter().any(|x| x.id() == rid) {
            let at = at.min(s.pending.len());
            s.pending.insert(at, p);
        }
    }
    crate::status_toast::fail(s, "Couldn't answer the permission", why);
    true
}

// The fixture session at rest; a bash permission arrives 0.6 s in (`--at 1.5` for a still).
rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("permission", |s| {
        crate::open_scene(s);
        s.finish_stream();
        let Route::Session(id) = s.route.clone() else { return };
        let ask = json!({ "id": "per_scene_bash", "sessionID": id, "action": "bash", "resources": ["bun test test/net/backoff.test.ts"] });
        s.playback.push_back((s.clock + 0.6, "permission.asked".into(), ask));
        s.wake_at_playback();
    }),
]);

mod stories;
#[cfg(test)]
mod tests;

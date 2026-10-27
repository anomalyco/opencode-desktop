//! User messages that arrive while the open session shows, from another client.

use std::collections::HashSet;

use opencode_kit::model::Message;

use crate::{App, Route};

/// The open session's user messages before a poll, so the ones that arrive from another client
/// (not this app's sends, not a first load) can glow as they land.
pub struct Arrivals {
    session: Option<String>,
    seen: HashSet<String>,
    echoes: Vec<String>,
}

impl Arrivals {
    pub fn before(s: &App) -> Arrivals {
        let session = match &s.route {
            Route::Session(id) if s.transcripts.contains_key(id) => Some(id.clone()),
            _ => None,
        };
        // Prompts already showing in a queue or steer list aren't arriving either.
        let queued = s.inboxes.values().flat_map(|i| i.items.iter().map(|i| i.id.clone()));
        let users = session.iter().flat_map(|id| s.transcripts[id].messages.iter().filter(|m| matches!(m, Message::User { .. })));
        let seen = users.map(|m| m.id().to_owned()).chain(queued).collect();
        Arrivals { session, seen, echoes: s.attach.echoes.iter().map(|e| e.2.clone()).collect() }
    }

    pub fn glow(self, s: &mut App, now: f64) {
        let Some(id) = self.session.filter(|id| s.route == Route::Session(id.clone())) else { return };
        if s.reduced_motion {
            return;
        }
        for m in s.transcripts.get(&id).map_or(&[][..], |t| &t.messages[..]) {
            if let Message::User { id, text } = m
                && !self.seen.contains(id)
                && !self.echoes.iter().any(|e| e == text.trim())
            {
                s.glows.insert(id.clone(), now);
            }
        }
    }
}

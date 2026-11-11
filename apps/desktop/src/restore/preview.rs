//! The shown session's first screen, kept in the app's database so the next launch paints it
//! from the boot packet before any server answers (`opencode_kit::model::preview`, a compact
//! binary form decoded without parsing JSON). It's the session's last two turns (what a window
//! opened at the bottom shows, and one scroll step up), at most [`BYTES`].
//!
//! It's written when it would change what a launch shows: when the shown session settles (its
//! reply finished, a message came or went), and, streaming or not, as the window hides, closes
//! or quits. A streamed token never writes it.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use opencode_kit::model::{Message, preview};

use crate::{App, Route};

/// A preview stays inside the boot record's first read (64 KiB pages).
const BYTES: usize = 48 * 1024;

/// What each session's preview was last written (or read) from.
#[derive(Default)]
pub struct Previews(HashMap<String, u64>);

/// What a session's preview depends on, cheap to take every poll.
fn stamp(messages: &[Message]) -> u64 {
    let mut h = DefaultHasher::new();
    messages.len().hash(&mut h);
    if let Some(last) = messages.last() {
        last.id().hash(&mut h);
        match last {
            Message::Assistant(a) => (a.completed, a.parts.len(), a.error.is_some()).hash(&mut h),
            Message::Note { failed, step, .. } => (failed, step.completed).hash(&mut h),
            _ => {}
        }
    }
    h.finish()
}

/// The preview's bytes: from the start of the turn before the last, trimmed from the front to fit.
fn encode(messages: &[Message]) -> Vec<u8> {
    let users: Vec<usize> = messages.iter().enumerate().filter(|(_, m)| matches!(m, Message::User { .. })).map(|(i, _)| i).collect();
    let mut start = users.len().checked_sub(2).map_or(0, |i| users[i]);
    loop {
        let bytes = preview::encode(&messages[start..]);
        if bytes.len() <= BYTES || start + 1 >= messages.len() {
            return bytes;
        }
        start += 1;
    }
}

impl App {
    /// The boot packet's preview of the window's shown session, decoded into its transcript: the
    /// first frame draws it while the server connects.
    pub(super) fn take_boot_preview(&mut self, server: &opencode_kit::servers::ServerKey, id: &str) -> bool {
        let Some((_, bytes)) = crate::storage::boot_preview(server.as_str(), id) else { return false };
        let Some(messages) = preview::decode(&bytes) else { return false };
        if messages.is_empty() {
            return false;
        }
        rune::marks::mark("state.preview");
        self.restore.previews.0.insert(id.to_owned(), stamp(&messages));
        self.transcripts.insert(id.to_owned(), opencode_kit::model::Transcript { session: id.to_owned(), messages });
        true
    }

    /// Keeps the shown session's preview current: when it settles, or (`now`) whatever it's doing,
    /// as the window hides, closes or quits.
    pub(crate) fn poll_preview(&mut self, now: bool) {
        let Route::Session { server, id } = &self.route else { return };
        let Some(t) = self.transcripts.get(id) else { return };
        let streaming = matches!(t.messages.last(), Some(Message::Assistant(a)) if a.completed.is_none());
        if (streaming && !now) || t.messages.is_empty() {
            return;
        }
        let stamp = stamp(&t.messages);
        if self.restore.previews.0.get(id) == Some(&stamp) {
            return;
        }
        let updated = self.session(id).map_or(0, |s| s.updated);
        crate::storage::set_preview(server.as_str(), id, Some((updated, Arc::from(encode(&t.messages)))));
        self.restore.previews.0.insert(id.clone(), stamp);
    }
}

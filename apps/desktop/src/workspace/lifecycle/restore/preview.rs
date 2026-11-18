//! The shown session's first screen, kept in the app's database so the next launch paints it
//! from the boot packet before any server answers (`opencode_kit::model::preview`, a compact
//! binary form decoded without parsing JSON). It's the session's last two turns (what a window
//! opened at the bottom shows, and one scroll step up), at most [`BYTES`]; read up from its end,
//! it's the turn of the place the window keeps (`places`) and what follows it, up to
//! [`PLACE_BYTES`], so the next launch draws the place in its first frame. When that stops short
//! of the end, the latest page lands under it with a gap between, which history fills first
//! (`session_pages`: `State::gap`).
//!
//! It's written when it would change what a launch shows: when the shown session settles (its
//! reply finished, a message came or went), when the window's place moves to another turn and
//! holds still (`App::preview_place`, as the place is written), and, streaming or not, as the
//! window hides, closes or quits. A streamed token or a scroll step never writes it.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use opencode_kit::model::{Message, preview};

use crate::{App, Route};

/// A preview stays inside the boot record's first read (64 KiB pages).
const BYTES: usize = 48 * 1024;
/// A place's preview: its turn and a few screens under it, not the whole way to the end.
const PLACE_BYTES: usize = 12 * 1024;

/// What each session's preview was last written (or read) from: its messages' stamp, and the
/// turn the place it starts from stood in.
#[derive(Default)]
pub struct Previews(HashMap<String, (u64, Option<usize>)>);

/// What a session's preview depends on, cheap to take every poll.
pub(super) fn stamp(messages: &[Message]) -> u64 {
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

/// The first message of the turn message `at` is in.
fn turn_of(messages: &[Message], at: usize) -> usize {
    messages[..=at.min(messages.len().saturating_sub(1))].iter().rposition(|m| matches!(m, Message::User { .. })).unwrap_or(0)
}

/// The preview's bytes: from the start of the turn before the last, trimmed from the front to fit.
/// From `turn`, the place's, instead when that's further up: its turn and the messages after it
/// up to [`PLACE_BYTES`] (the place's turn whole if it alone is more, up to [`BYTES`]).
fn encode(messages: &[Message], turn: Option<usize>) -> Vec<u8> {
    let users: Vec<usize> = messages.iter().enumerate().filter(|(_, m)| matches!(m, Message::User { .. })).map(|(i, _)| i).collect();
    let mut start = users.len().checked_sub(2).map_or(0, |i| users[i]);
    if let Some(place) = turn.filter(|&t| t < start) {
        // From the turn before the place's, so the place isn't the transcript's first row, which
        // the history's loading look above it would move. The place's turn ends at the next
        // prompt.
        let turn = users.iter().copied().rev().find(|&u| u < place).unwrap_or(place);
        let own = users.iter().copied().find(|&u| u > place).unwrap_or(messages.len());
        let mut end = own;
        let mut bytes = preview::encode(&messages[turn..end]);
        while end < messages.len() {
            let more = preview::encode(&messages[turn..end + 1]);
            if more.len() > PLACE_BYTES {
                break;
            }
            (bytes, end) = (more, end + 1);
        }
        if bytes.len() <= BYTES {
            return bytes;
        }
    }
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
        self.restore.previews.0.insert(id.to_owned(), (stamp(&messages), None));
        self.transcripts.insert(id.to_owned(), opencode_kit::model::Transcript { session: id.to_owned(), messages });
        true
    }

    /// The turn the shown session's place stands in, when it's read up from its end.
    fn place_turn(&self, id: &str, messages: &[Message]) -> Option<usize> {
        let t = self.transcripts.get(id)?;
        let (row, _) = self.ends.place(id)?;
        session_view::timeline::row_message(self, t, &row).map(|at| turn_of(messages, at))
    }

    /// Writes the shown session's preview if what it would hold (`stamp`, from `turn`) isn't
    /// what it holds.
    fn write_preview(&mut self, server: &str, id: &str, stamp: u64, turn: Option<usize>) {
        if self.restore.previews.0.get(id) == Some(&(stamp, turn)) {
            return;
        }
        let Some(t) = self.transcripts.get(id) else { return };
        let updated = self.session(id).map_or(0, |s| s.updated);
        crate::storage::set_preview(server, id, Some((updated, Arc::from(encode(&t.messages, turn)))));
        self.restore.previews.0.insert(id.to_owned(), (stamp, turn));
    }

    /// The shown session's place moved and held still (`places`): its preview runs from the
    /// place's turn now. Not while streaming, and not while the transcript is still the boot
    /// record's.
    pub(super) fn preview_place(&mut self) {
        let Route::Session { server, id } = &*self.route else { return };
        let Some(t) = self.transcripts.get(id) else { return };
        let streaming = matches!(t.messages.last(), Some(Message::Assistant(a)) if a.completed.is_none());
        // Nor while history fills the gap under a place's preview: the transcript has a hole.
        if streaming || t.messages.is_empty() || self.launch.cold_transcript.as_deref() == Some(id.as_str()) || self.pages.gap(id) {
            return;
        }
        let stamp = stamp(&t.messages);
        let turn = self.place_turn(id, &t.messages);
        let (server, id) = (server.as_str().to_owned(), id.clone());
        self.write_preview(&server, &id, stamp, turn);
    }

    /// Keeps the shown session's preview current: when it settles, or (`now`) whatever it's doing,
    /// as the window hides, closes or quits.
    pub(crate) fn poll_preview(&mut self, now: bool) {
        let Route::Session { server, id } = &*self.route else { return };
        let Some(t) = self.transcripts.get(id) else { return };
        let streaming = matches!(t.messages.last(), Some(Message::Assistant(a)) if a.completed.is_none());
        // A transcript with a gap under a place's preview keeps the preview and copy it came from.
        if (streaming && !now) || t.messages.is_empty() || self.pages.gap(id) {
            return;
        }
        let stamp = stamp(&t.messages);
        // The session's copy for reading it later (`copies`), once it has settled.
        // A transcript that's still the boot record's two turns would overwrite a fuller copy.
        let cold = self.launch.cold_transcript.as_deref() == Some(id.as_str());
        let copy = (!streaming && !cold && self.copy_due(id, stamp)).then(|| {
            rune::marks::mark("state.copy.encode");
            let bytes = super::copies::encode(&t.messages);
            rune::marks::mark("state.copy.encoded");
            bytes
        });
        // The place is read as the messages change and as the window hides, closes or quits; as
        // it moves, `preview_place` keeps up.
        let kept = self.restore.previews.0.get(id).copied();
        let turn = if now || kept.is_none_or(|(s, _)| s != stamp) { self.place_turn(id, &t.messages) } else { kept.and_then(|(_, turn)| turn) };
        let (server, id) = (server.clone(), id.clone());
        self.write_preview(server.as_str(), &id, stamp, turn);
        if let Some(bytes) = copy {
            self.store_copy(&server, &id, stamp, bytes);
        }
    }
}

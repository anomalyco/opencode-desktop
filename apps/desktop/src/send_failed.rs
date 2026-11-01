//! A prompt the server never took. Desktop puts a failed send back in the composer and says
//! "Failed to send prompt" (`composer/submit.ts` `failSubmission`); here too.
//!
//! - A failure that certainly never arrived (a refusal, no connection) rolls back at once.
//! - One that may have arrived (a timeout) keeps its prompt marked "Sending…" until the server
//!   answers again (any read, or the event stream coming back). Then the session is re-read:
//!   a prompt with that id is there, so it was sent after all; if not, it rolls back.
//!
//! A send is never retried, so it can't post twice.
use opencode_kit::client::PathAttachment;
use opencode_kit::model::{Attachment, Message};
use opencode_kit::plain_error::failed_for_sure;

use crate::prompt_history::Entry;
use crate::{App, Source};

const KEEP: usize = 32;
/// A re-read that hasn't been answered by now is asked for again.
const REASK: f64 = 10.0;

pub const TITLE: &str = "Failed to send prompt";

/// A prompt sent and not yet seen back from the server.
pub struct Sent {
    pub session: String,
    pub id: String,
    pub draft: Entry,
    pub files: Vec<Attachment>,
    pub paths: Vec<PathAttachment>,
    /// Waiting in the session's inbox (a steer or a queued prompt), not shown as a message.
    pub inbox: bool,
    pub why: String,
    /// The write may have arrived; set once it timed out.
    pub unsure: bool,
    /// When the check was last asked for, and whether a re-read since showed no such message.
    asked: Option<f64>,
    missing: bool,
}

#[derive(Default)]
pub struct Log {
    sent: Vec<Sent>,
}

impl Log {
    pub fn note(&mut self, s: Sent) {
        self.sent.retain(|x| x.unsure || x.id != s.id);
        self.sent.push(s);
        if self.sent.len() > KEEP {
            if let Some(i) = self.sent.iter().position(|x| !x.unsure) { self.sent.remove(i); }
        }
    }

    /// Whether prompt `id` is on screen as "Sending…".
    pub fn sending(&self, id: &str) -> bool {
        self.sent.iter().any(|s| s.id == id && s.unsure)
    }

    fn take(&mut self, id: &str) -> Option<Sent> {
        let at = self.sent.iter().position(|s| s.id == id)?;
        Some(self.sent.remove(at))
    }

    pub fn unsure(&self) -> bool {
        self.sent.iter().any(|s| s.unsure)
    }
}

impl Sent {
    pub fn new(session: &str, id: &str, draft: Entry, files: Vec<Attachment>, paths: Vec<PathAttachment>, inbox: bool) -> Self {
        Sent { session: session.into(), id: id.into(), draft, files, paths, inbox, why: String::new(), unsure: false, asked: None, missing: false }
    }
}

impl App {
    /// A prompt write failed. False when this isn't a send the app knows (the caller shows the
    /// failure its usual way).
    pub fn send_failed(&mut self, id: &str, error: &str) -> bool {
        let why = error.split_once(": ").map_or(error, |(_, why)| why);
        eprintln!("opencode-rune: prompt send failed: {error}");
        let Some(sent) = self.unsent.sent.iter_mut().find(|s| s.id == id) else { return false };
        if failed_for_sure(error) {
            let sent = self.unsent.take(id).unwrap();
            self.roll_back(sent, crate::status_toast::plain(why));
        } else {
            sent.unsure = true;
            sent.why = crate::status_toast::plain(why);
        }
        true
    }

    /// The server answered something: ask what became of each prompt that timed out. Called
    /// when the event stream is back and after any read that came from the server.
    pub fn check_sends(&mut self) {
        let clock = self.clock;
        let Source::Live { requests, .. } = &self.source else { return };
        let requests = requests.clone();
        let mut ask = Vec::new();
        for s in self.unsent.sent.iter_mut().filter(|s| s.unsure) {
            if s.asked.is_some_and(|at| clock - at < REASK) { continue; }
            s.asked = Some(clock);
            s.missing = false;
            ask.push((s.session.clone(), s.inbox));
        }
        for (session, inbox) in ask {
            self.read_latest(&session);
            if inbox && let Some(r) = &requests { let _ = r.send(opencode_kit::client::Request::Inbox(session)); }
        }
    }

    /// A fresh latest page of `session` landed from the server: a prompt that timed out is
    /// either in it (sent) or, unless it waits in the inbox, never arrived.
    pub fn judge_sends_by_page(&mut self, session: &str, messages: &[Message]) {
        let mut lost = Vec::new();
        for s in self.unsent.sent.iter_mut().filter(|s| s.unsure && s.session == session && s.asked.is_some()) {
            if messages.iter().any(|m| m.id() == s.id) { lost.push((s.id.clone(), true)); }
            else if !s.inbox { lost.push((s.id.clone(), false)); }
            else { s.missing = true; }
        }
        self.settle_sends(lost);
    }

    /// The session's inbox was listed.
    pub fn judge_sends_by_inbox(&mut self, session: &str, ids: &[String]) {
        let mut done = Vec::new();
        for s in self.unsent.sent.iter().filter(|s| s.unsure && s.session == session && s.inbox && s.asked.is_some()) {
            if ids.contains(&s.id) { done.push((s.id.clone(), true)); }
            else if s.missing { done.push((s.id.clone(), false)); }
        }
        self.settle_sends(done);
    }

    fn settle_sends(&mut self, done: Vec<(String, bool)>) {
        for (id, arrived) in done {
            let Some(sent) = self.unsent.take(&id) else { continue };
            if !arrived { let why = sent.why.clone(); self.roll_back(sent, why); }
        }
    }

    /// Takes the prompt back off the timeline and out of the inbox, puts its words and files
    /// in the composer when that's empty, and says why in plain words.
    fn roll_back(&mut self, sent: Sent, why: String) {
        if let Some(t) = self.transcripts.get_mut(&sent.session) { t.messages.retain(|m| m.id() != sent.id); }
        if let Some(i) = self.inboxes.get_mut(&sent.session) { i.items.retain(|x| x.id != sent.id); }
        self.attach.echoes.retain(|e| e.1 != sent.id);
        if self.composer.text.is_empty() && !self.attach.any() {
            sent.draft.put(&mut self.composer, sent.draft.text.len());
            for f in sent.files { self.attach.add(f); }
            for p in sent.paths { self.attach.add_path(p); }
        }
        crate::status_toast::fail(self, TITLE, why);
    }
}

#[cfg(test)]
#[path = "send_failed_tests.rs"]
mod tests;

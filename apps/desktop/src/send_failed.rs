//! Delivery receipts outlive a timed-out POST. A timeout is not proof of failure: server
//! admission, a fetched message or a read-only receipt check settles it, never another POST.
use opencode_kit::client::{PathAttachment, Prompt};
use opencode_kit::model::{Attachment, Message};
use opencode_kit::plain_error::failed_for_sure;

use crate::prompt_history::Entry;
use crate::{App, Source};

mod receipt;
mod view;
pub(crate) mod stories;
pub use view::footer;

const KEEP: usize = 32;
pub const TITLE: &str = "Failed to send prompt";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum State {
    #[default]
    Sending,
    Unconfirmed,
    Failed,
    Accepted,
}

#[derive(Clone)]
pub struct Sent {
    pub session: String,
    pub id: String,
    pub draft: Entry,
    pub files: Vec<Attachment>,
    pub paths: Vec<PathAttachment>,
    pub inbox: bool,
    pub why: String,
    pub state: State,
    pub prompt: Option<Prompt>,
}

#[derive(Default)]
pub struct Log {
    sent: Vec<Sent>,
    receipts: rune::Latest<Vec<(String, Result<receipt::Receipt, String>)>>,
    checking: bool,
    check_again: bool,
    #[cfg(test)]
    fetch: Option<crate::session_pages::Fetch>,
    /// Set, receipt checks are handed here for a test's own network to answer
    /// (`receipts_from`), rather than read on a thread.
    #[cfg(test)]
    pub(crate) asked: Option<Vec<(String, String)>>,
}

impl Log {
    pub fn note(&mut self, s: Sent) {
        self.sent.retain(|x| x.id != s.id);
        self.sent.push(s);
        if self.sent.len() > KEEP {
            // Never discard words the user still needs to recover.
            if let Some(i) = self.sent.iter().position(|x| x.state == State::Accepted) { self.sent.remove(i); }
        }
    }

    pub fn state(&self, id: &str) -> Option<State> {
        self.sent.iter().find(|s| s.id == id).map(|s| s.state)
    }

    pub fn sending(&self, id: &str) -> bool { self.state(id) == Some(State::Unconfirmed) }

    pub(crate) fn take(&mut self, id: &str) -> Option<Sent> {
        let at = self.sent.iter().position(|s| s.id == id)?;
        Some(self.sent.remove(at))
    }

    pub(crate) fn preserve_inbox(&self, session: &str, inbox: &mut opencode_kit::model::Inbox, old: Vec<opencode_kit::model::InboxItem>) {
        for item in old {
            if self.sent.iter().any(|s| s.session == session && s.id == item.id && matches!(s.state, State::Sending | State::Unconfirmed))
                && !inbox.items.iter().any(|i| i.id == item.id) { inbox.upsert(item); }
        }
    }
}

impl Sent {
    pub fn new(session: &str, id: &str, draft: Entry, files: Vec<Attachment>, paths: Vec<PathAttachment>, inbox: bool) -> Self {
        Sent { session: session.into(), id: id.into(), draft, files, paths, inbox, why: String::new(), state: State::Sending, prompt: None }
    }

    pub fn with_prompt(mut self, prompt: &Prompt) -> Self { self.prompt = Some(prompt.clone()); self }
}

impl App {
    pub fn send_failed(&mut self, id: &str, error: &str) -> bool {
        let Some(sent) = self.unsent.sent.iter_mut().find(|s| s.id == id) else { return false };
        // An SSE admission can beat the failed HTTP response back to the UI.
        if sent.state == State::Accepted { return true; }
        eprintln!("opencode-rune: prompt send failed: {error}");
        let why = error.split_once(": ").map_or(error, |(_, why)| why);
        sent.why = crate::status_toast::plain(why);
        if failed_for_sure(error) { self.fail_send(id); }
        else {
            sent.state = State::Unconfirmed;
            self.check_sends();
        }
        true
    }

    /// A successful read or reconnect retries receipts, with at most one bounded batch in flight.
    pub fn check_sends(&mut self) {
        if self.unsent.checking { self.unsent.check_again = true; return; }
        let Source::Live { server, .. } = &self.source else { return };
        let server = server.clone();
        let fetch: crate::session_pages::Fetch = std::sync::Arc::new(move |path| server.read(path));
        #[cfg(test)]
        let fetch = self.unsent.fetch.clone().unwrap_or(fetch);
        let mut ask = Vec::new();
        for s in self.unsent.sent.iter().filter(|s| s.state == State::Unconfirmed) {
            ask.push((s.session.clone(), s.id.clone()));
        }
        if ask.is_empty() { return; }
        self.unsent.checking = true;
        #[cfg(test)]
        if let Some(asked) = &mut self.unsent.asked { asked.extend(ask); return; }
        self.unsent.receipts = rune::Latest::new(self.waker.clone().unwrap_or_else(|| std::sync::Arc::new(|| {})));
        self.unsent.receipts.request(ask, move |ask| read_receipts(&*fetch, ask));
    }

    pub(crate) fn poll_sends(&mut self) -> bool {
        let Some(receipts) = self.unsent.receipts.take() else { return false };
        self.apply_receipts(receipts);
        true
    }

    /// Answers the checks handed to `Log::asked` with `fetch`, as the thread would have.
    #[cfg(test)]
    pub(crate) fn receipts_from(&mut self, fetch: &dyn Fn(&str) -> Result<serde_json::Value, String>, ask: Vec<(String, String)>) {
        self.apply_receipts(read_receipts(fetch, ask));
    }

    fn apply_receipts(&mut self, receipts: Vec<(String, Result<receipt::Receipt, String>)>) {
        self.unsent.checking = false;
        for (id, result) in receipts {
            // A newer SSE receipt always wins over an older absent read.
            if self.unsent.state(&id) != Some(State::Unconfirmed) { continue; }
            match result {
                Ok(receipt::Receipt::Delivered(message)) => {
                    let session = self.unsent.sent.iter().find(|s| s.id == id).unwrap().session.clone();
                    self.transcripts.entry(session.clone()).or_insert_with(|| opencode_kit::model::Transcript { session: session.clone(), messages: vec![] }).merge(vec![message]);
                    if let Some(inbox) = self.inboxes.get_mut(&session) { inbox.remove(&id); }
                    self.accept_send(&id);
                }
                Ok(receipt::Receipt::Admitted) => { self.accept_send(&id); }
                Ok(receipt::Receipt::Missing) => self.fail_send(&id),
                Err(error) => eprintln!("opencode-rune: prompt receipt unavailable: {error}"),
            }
        }
        if std::mem::take(&mut self.unsent.check_again) { self.check_sends(); }
    }

    /// Positive evidence is usable even without a preceding reconnect, and from older pages.
    /// Absence on a limited latest page is NOT evidence that an older prompt never arrived.
    pub fn judge_sends_by_page(&mut self, _session: &str, messages: &[Message]) {
        for message in messages { self.accept_send(message.id()); }
    }

    pub fn judge_sends_by_inbox(&mut self, _session: &str, ids: &[String]) -> bool {
        let mut changed = false;
        for id in ids { changed |= self.accept_send(id); }
        changed
    }

    pub(crate) fn accept_send(&mut self, id: &str) -> bool {
        let Some(s) = self.unsent.sent.iter_mut().find(|s| s.id == id) else { return false };
        let changed = s.state != State::Accepted;
        s.state = State::Accepted;
        changed
    }

    fn fail_send(&mut self, id: &str) {
        let Some(sent) = self.unsent.sent.iter_mut().find(|s| s.id == id) else { return };
        sent.state = State::Failed;
        let sent = sent.clone();
        // Keep the failed bubble in place, including when it was an optimistic inbox entry.
        if let Some(i) = self.inboxes.get_mut(&sent.session) { i.remove(id); }
        let t = self.transcripts.entry(sent.session.clone()).or_insert_with(|| opencode_kit::model::Transcript { session: sent.session.clone(), messages: vec![] });
        if !t.messages.iter().any(|m| m.id() == id) {
            t.messages.push(Message::User { id: id.into(), text: sent.draft.text.clone() });
        }
        self.attach.echoes.retain(|e| e.1 != id);
        crate::status_toast::fail(self, TITLE, sent.why);
    }

    pub(crate) fn retry_send(&mut self, id: &str) {
        let Some(sent) = self.unsent.sent.iter().find(|s| s.id == id && s.state == State::Failed).cloned() else { return };
        let Some(prompt) = sent.prompt.clone() else { return };
        let write = self.send_write(&sent.session, prompt);
        if !matches!(self.source, Source::Fixture) && !self.write(write) { return; }
        if sent.inbox {
            if let Some(t) = self.transcripts.get_mut(&sent.session) { t.messages.retain(|m| m.id() != id); }
            self.inboxes.entry(sent.session.clone()).or_default().upsert(opencode_kit::model::InboxItem {
                id: id.into(), text: sent.draft.text.clone(), files: sent.files.clone(),
                delivery: sent.prompt.as_ref().and_then(|p| p.delivery).unwrap_or(opencode_kit::model::Delivery::Steer),
            });
        }
        self.unsent.note(Sent { state: State::Sending, ..sent });
    }

    pub(crate) fn edit_failed_send(&mut self, id: &str) {
        let Some(sent) = self.unsent.sent.iter().find(|s| s.id == id && s.state == State::Failed).cloned() else { return };
        if self.route != crate::Route::Session(sent.session.clone()) || !self.composer.text.is_empty() || self.attach.any() { return; }
        self.restore_draft(sent);
        self.composer.focused = true;
    }
}

/// Each prompt's receipt in turn, stopping at the first the server can't answer.
fn read_receipts(fetch: &dyn Fn(&str) -> Result<serde_json::Value, String>, ask: Vec<(String, String)>) -> Vec<(String, Result<receipt::Receipt, String>)> {
    let mut results = Vec::new();
    for (session, id) in ask {
        let result = receipt::read(fetch, &session, &id);
        let unavailable = result.is_err();
        results.push((id, result));
        if unavailable { break; }
    }
    results
}

#[cfg(test)]
#[path = "send_failed_tests.rs"]
mod tests;

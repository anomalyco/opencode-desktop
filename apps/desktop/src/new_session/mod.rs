//! Starting a session from the new-session page. Send shows the new session at once, its prompt
//! the first message with Working under it, while the server creates it and takes the prompt
//! in the background (`Write::StartAs`). The id is chosen here, as Desktop does, so nothing
//! swaps when the server's copy lands. A start that fails goes back to the new-session page
//! with the words in the composer.
use opencode_kit::client::Request;
use opencode_kit::model::SessionInfo;
use rune::Tx;

use crate::send_failed::Sent;
use crate::{App, Route, Source};

/// Sessions on screen that the server may not have yet, with the folder each started in.
#[derive(Default)]
pub struct Starts(Vec<(String, String)>);

impl Starts {
    pub fn has(&self, session: &str) -> bool {
        self.0.iter().any(|(s, _)| s == session)
    }

    fn take(&mut self, session: &str) -> Option<String> {
        let at = self.0.iter().position(|(s, _)| s == session)?;
        Some(self.0.remove(at).1)
    }
}

pub const FAILED: &str = "Couldn't start the session";

#[cfg(test)]
thread_local! {
    /// The benchmark's "before": Send waits for the server's session, as it used to.
    pub(crate) static BEFORE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether Send on the new-session page takes the old wait-for-the-server path.
pub(crate) fn before() -> bool {
    #[cfg(test)]
    { BEFORE.get() }
    #[cfg(not(test))]
    { false }
}

impl App {
    /// Shows session `id`, which the server is about to create in the new-session page's
    /// folder with the toolbar's agent and model, as the open page. It switches at once, as any
    /// session opens; the caller adds the prompt. A live one asks the server for nothing until
    /// it lands, so nothing reads a session that isn't there.
    pub(crate) fn start(&mut self, id: &str, tx: &mut Tx) {
        let selection = self.selection();
        let mut info = SessionInfo {
            id: id.into(),
            directory: self.draft_dir.clone(),
            agent: selection.agent,
            model: selection.model,
            created: self.now_ms,
            updated: self.now_ms,
            ..Default::default()
        };
        info.project = self.project_for_session(&info).map(|p| p.id.clone()).unwrap_or_default();
        self.sessions.insert(0, info);
        if self.live() {
            self.starts.0.push((id.into(), self.draft_dir.clone()));
        }
        self.open_now(id, tx);
    }

    /// `Incoming::Created` for a start: the server has the session and its prompt. It takes the
    /// server's details and reads what opening it would have.
    pub(crate) fn started(&mut self, mut info: SessionInfo, tx: &mut Tx) {
        self.starts.take(&info.id);
        let id = info.id.clone();
        if let Some(old) = self.sessions.iter_mut().find(|s| s.id == id) {
            info.viewed = info.viewed.max(old.viewed);
            *old = info;
        }
        if let Source::Live { requests: Some(r), .. } = &self.source {
            if !self.pages.has_pool() { let _ = r.send(Request::Session(id.clone())); }
            let _ = r.send(Request::Pending(id.clone()));
            let _ = r.send(Request::Inbox(id.clone()));
        }
        crate::compaction::ask(self, &id);
        self.open_page(&id, tx);
    }

    /// `Incoming::SendFailed` for a start: the session goes (the server has deleted whatever it
    /// made), and the new-session page comes back with the prompt in its composer. False when
    /// `session` isn't starting.
    pub(crate) fn start_failed(&mut self, session: &str, prompt: &str, error: &str, tx: &mut Tx) -> bool {
        let Some(directory) = self.starts.take(session) else { return false };
        let sent = self.unsent.take(prompt);
        self.sessions.retain(|s| s.id != session);
        self.transcripts.remove(session);
        self.inboxes.remove(session);
        self.running.remove(session);
        if self.tabs.iter().any(|t| t.id == session) {
            self.tabs.retain(|t| t.id != session);
            self.save_tabs();
        }
        self.forget_history(|_, id| id == session);
        if self.route == Route::Session(session.into()) {
            self.route = Route::Draft;
            self.draft_dir = directory;
            self.composer.focused = true;
            tx.settle();
        }
        if let Some(sent) = sent {
            self.restore_draft(sent);
        }
        let why = error.split_once(": ").map_or(error, |(_, why)| why);
        crate::status_toast::fail(self, FAILED, crate::status_toast::plain(why));
        true
    }

    /// Puts a prompt that never went back in the composer, when that's empty.
    pub(crate) fn restore_draft(&mut self, sent: Sent) {
        self.attach.echoes.retain(|e| e.1 != sent.id);
        if self.composer.text.is_empty() && !self.attach.any() {
            sent.draft.put(&mut self.composer, sent.draft.text.len());
            for f in sent.files { self.attach.add(f); }
            for p in sent.paths { self.attach.add_path(p); }
        }
    }
}

mod stories;
#[cfg(test)]
mod bench;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

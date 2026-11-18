//! Answering open permission requests and structured forms (`Pending`).

use opencode_kit::model::Pending;
use rune::TextEdit;
use serde_json::Value;

use crate::App;
use crate::question::reply::Action;

impl App {
    /// The open requests in a session's family (a subagent's question shows on its root).
    pub fn pending_for(&self, id: &str) -> Vec<&Pending> {
        let root = self.root_of(id);
        self.pending.iter().filter(|p| self.root_of(p.session()) == root).collect()
    }

    pub(crate) fn add_pending(&mut self, p: Pending) -> bool {
        // An answered permission: a re-read before the server drops it doesn't bring it back.
        if self.permission_sent.replied(p.id()) {
            return false;
        }
        let mut changed = self.requests.insert((p.session().to_owned(), p.id().to_owned()));
        if !self.pending.iter().any(|x| x.id() == p.id()) {
            let id = p.id().to_owned();
            self.pending.push(p);
            self.auto_answer(&id);
            changed = true;
        }
        changed
    }

    pub(crate) fn forget(&mut self, id: &str) {
        self.pending.retain(|p| p.id() != id);
        self.requests.retain(|(_, r)| r != id);
        self.question.forget(id);
        self.permission_sent.0.remove(id);
        crate::notifications::answered(self, id);
    }

    pub fn typed_mut(&mut self, form: &str, key: &str) -> &mut TextEdit {
        self.question.typed_mut(form, key)
    }

    /// The answer to send, or None while a required field is empty.
    pub fn form_answer(&self, id: &str) -> Option<Value> {
        let Some(Pending::Form { fields, .. }) = self.pending.iter().find(|p| p.id() == id) else {
            return None;
        };
        self.question.answer(id, fields)
    }

    pub fn submit_form(&mut self, id: &str) {
        let Some(answer) = self.form_answer(id) else {
            crate::status_toast::say(self, "Answer the questions first", "");
            return;
        };
        self.send_form(id, Action::Reply(answer));
    }

    pub fn dismiss_form(&mut self, id: &str) {
        self.send_form(id, Action::Dismiss);
    }
}

//! Answering open permission requests and structured forms (`Pending`).

use opencode_kit::client::Write;
use opencode_kit::model::Pending;
use rune::TextEdit;
use serde_json::Value;

use crate::App;

impl App {
    /// The open requests in a session's family (a subagent's question shows on its root).
    pub fn pending_for(&self, id: &str) -> Vec<&Pending> {
        let root = self.root_of(id);
        self.pending.iter().filter(|p| self.root_of(p.session()) == root).collect()
    }

    pub(crate) fn add_pending(&mut self, p: Pending) {
        // A reply on its way: a re-read before the server has it doesn't bring it back.
        if self.permission_sent.0.contains_key(p.id()) {
            return;
        }
        self.requests.insert((p.session().to_owned(), p.id().to_owned()));
        if !self.pending.iter().any(|x| x.id() == p.id()) {
            let id = p.id().to_owned();
            self.pending.push(p);
            self.auto_answer(&id);
        }
    }

    pub(crate) fn forget(&mut self, id: &str) {
        self.pending.retain(|p| p.id() != id);
        self.requests.retain(|(_, r)| r != id);
        self.question.forget(id);
        self.permission_sent.0.remove(id);
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
        let Some(p) = self.pending.iter().find(|p| p.id() == id) else { return };
        if self.write(Write::Form { session: p.session().to_owned(), form: id.to_owned(), answer })
        {
            self.forget(id);
        }
    }

    pub fn dismiss_form(&mut self, id: &str) {
        let Some(p) = self.pending.iter().find(|p| p.id() == id) else { return };
        if self.write(Write::CancelForm { session: p.session().to_owned(), form: id.to_owned() }) {
            self.forget(id);
        }
    }
}

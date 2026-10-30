//! Answering open permission requests and structured forms (`Pending`).

use opencode_kit::client::Write;
use opencode_kit::model::{FieldKind, Pending};
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
    }

    pub fn reply_permission(&mut self, id: &str, decision: &str) {
        let Some(p) = self.pending.iter().find(|p| p.id() == id) else { return };
        let w = Write::Permission {
            session: p.session().to_owned(),
            request: id.to_owned(),
            decision: decision.to_owned(),
        };
        if self.write(w) {
            self.forget(id);
        }
    }

    pub fn typed_mut(&mut self, form: &str, key: &str) -> &mut TextEdit {
        self.question.typed.entry(format!("{form}/{key}")).or_insert_with(|| TextEdit::new(""))
    }

    /// The answer to send, or None while a required field is empty.
    pub fn form_answer(&self, id: &str) -> Option<Value> {
        let Some(Pending::Form { fields, .. }) = self.pending.iter().find(|p| p.id() == id) else {
            return None;
        };
        let mut answer = serde_json::Map::new();
        for f in fields {
            let k = format!("{id}/{}", f.key);
            let typed = self
                .question
                .typed
                .get(&k)
                .filter(|_| self.question.custom_on(&k))
                .map(|t| t.text.trim().to_owned())
                .filter(|t| !t.is_empty());
            let picks = self.question.picks.get(&k).cloned().unwrap_or_default();
            let v = match f.kind {
                FieldKind::Multi => {
                    let mut all = picks;
                    all.extend(typed);
                    (!all.is_empty()).then(|| Value::from(all))
                }
                FieldKind::Boolean => picks.first().map(|v| Value::Bool(v == "true")),
                FieldKind::Number => typed.and_then(|t| t.parse::<f64>().ok()).map(Value::from),
                FieldKind::Text => typed.or_else(|| picks.first().cloned()).map(Value::from),
            };
            match v {
                Some(v) => {
                    answer.insert(f.key.clone(), v);
                }
                None if f.required || fields.len() == 1 => return None,
                None => {}
            }
        }
        Some(Value::Object(answer))
    }

    pub fn submit_form(&mut self, id: &str) {
        let Some(answer) = self.form_answer(id) else {
            self.status = "Answer the questions first".into();
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

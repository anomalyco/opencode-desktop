//! The fixture's server: the replies it stores from the events it streams, and the page
//! refresh it answers after a step starts, as a live server does (`events.rs`).
//!
//! A server keeps a reply's parts in the order they started, and a stream's ordinals count
//! texts and thoughts each on their own. So a refresh shows any place the app folded the
//! stream into different parts than the server stored.

use std::collections::HashMap;

use opencode_kit::model::{Message, Part, Transcript};
use serde_json::{Value, json};

/// How long after a step starts its page refresh lands.
pub const REFRESH: f64 = 0.55;

#[derive(Default)]
pub struct Served {
    /// Each reply's streamed events, by assistant message id.
    log: HashMap<String, Vec<(String, Value)>>,
}

impl Served {
    pub fn record(&mut self, ty: &str, d: &Value) {
        if let Some(id) = d["assistantMessageID"].as_str().filter(|id| !id.is_empty()) {
            self.log.entry(id.to_owned()).or_default().push((ty.to_owned(), d.clone()));
        }
    }

    /// The latest page as the server returns it: the session's last messages, each streamed
    /// reply with its parts as stored.
    pub fn page(&self, t: &Transcript) -> Vec<Message> {
        let from = t.messages.len().saturating_sub(crate::session_pages::LATEST);
        t.messages[from..].iter().map(|m| match (m, self.log.get(m.id())) {
            (Message::Assistant(a), Some(log)) => {
                let mut a = a.clone();
                a.parts = stored(log, &a.parts);
                Message::Assistant(a)
            }
            (m, _) => m.clone(),
        }).collect()
    }
}

/// A reply's parts in start order. Tools are taken as the app has them.
fn stored(log: &[(String, Value)], app: &[Part]) -> Vec<Part> {
    let mut parts: Vec<Part> = Vec::new();
    let mut at: HashMap<(bool, u64), usize> = HashMap::new();
    for (ty, d) in log {
        let text = ty.starts_with("session.text.");
        let key = (text, d["ordinal"].as_u64().unwrap_or(0));
        match ty.as_str() {
            "session.text.started" | "session.reasoning.started" => {
                at.insert(key, parts.len());
                parts.push(if text { Part::Text { text: String::new() } } else { Part::Reasoning { text: String::new(), started: 0, completed: None } });
            }
            "session.text.delta" | "session.reasoning.delta" | "session.text.ended" | "session.reasoning.ended" => {
                let Some(Part::Text { text: body } | Part::Reasoning { text: body, .. }) = at.get(&key).and_then(|&i| parts.get_mut(i)) else { continue };
                match d["delta"].as_str() {
                    Some(delta) => body.push_str(delta),
                    None => if let Some(full) = d["text"].as_str() { *body = full.to_owned() },
                }
                if let (true, Part::Reasoning { completed, .. }) = (ty.ends_with(".ended"), &mut parts[at[&key]]) {
                    *completed = Some(1000);
                }
            }
            "session.tool.input.started" | "session.tool.called" => {
                let id = d["id"].as_str().unwrap_or("");
                let seen = parts.iter().any(|p| matches!(p, Part::Tool { id: t, .. } if t == id));
                if let (false, Some(tool)) = (seen, app.iter().find(|p| matches!(p, Part::Tool { id: t, .. } if t == id))) {
                    parts.push(tool.clone());
                }
            }
            _ => {}
        }
    }
    parts
}

impl crate::App {
    /// A live app re-reads the open session after each step starts (`events.rs`); the fixture
    /// answers from what it streamed.
    pub(crate) fn refresh_fixture_page(&mut self, sid: &str) {
        let at = self.clock + REFRESH;
        self.playback.push_back((at, "fixture.page".into(), json!({ "sessionID": sid })));
        self.playback.make_contiguous().sort_by(|a, b| a.0.total_cmp(&b.0));
    }

    pub(crate) fn show_fixture_page(&mut self, sid: &str, tx: &mut rune::Tx) {
        let Some(t) = self.transcripts.get(sid) else { return };
        let messages = self.served.page(t);
        let key = crate::session_pages::Key { id: sid.to_owned(), updated: self.session(sid).map_or(0, |s| s.updated) };
        let page = crate::session_pages::Page { key, messages, cursor: None, cached: false, history: false, images: Vec::new(), bytes: 0 };
        self.show_page(&page, tx);
    }
}

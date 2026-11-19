//! The fixture's server: the replies it stores from the events it streams, and the page
//! refresh it answers after a step starts, as a live server does (`session::events`).
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
    /// Queues a fixture event at `at`, keeping `playback` ordered by time.
    pub(crate) fn schedule(&mut self, at: f64, ty: impl Into<String>, data: Value) {
        let i = self.fixture_run.playback.iter().position(|e| e.0 > at).unwrap_or(self.fixture_run.playback.len());
        self.fixture_run.playback.insert(i, (at, ty.into(), data));
    }

    /// A live app re-reads the open session after each step starts (`events.rs`); the fixture
    /// answers from what it streamed.
    pub(crate) fn refresh_fixture_page(&mut self, sid: &str) {
        self.schedule(*self.clock + REFRESH, "fixture.page", json!({ "sessionID": sid }));
    }

    /// `POST /api/session/:id/background`: each blocking call returns, as a live server's does
    /// once its job moves to the background, and the job runs on unseen.
    pub(crate) fn fixture_background(&mut self, sid: &str, parts: &[String]) {
        self.appearance_state.background.fixture_moves.push(sid.to_owned());
        let Some(Message::Assistant(a)) = self.transcripts.get(sid).and_then(|t| t.messages.iter().rev().find(|m| matches!(m, Message::Assistant(a) if a.completed.is_none()))) else { return };
        let at = *self.clock + 0.05;
        let moves: Vec<Value> = a
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::Tool { id, metadata, .. } if parts.contains(id) => {
                    let mut metadata = if metadata.is_object() { metadata.clone() } else { json!({}) };
                    metadata["background"] = true.into();
                    Some(json!({ "sessionID": sid, "assistantMessageID": a.id, "id": id, "metadata": metadata }))
                }
                _ => None,
            })
            .collect();
        for d in moves {
            self.schedule(at, "session.tool.success", d);
        }
        self.wake_at_playback();
    }

    pub(crate) fn show_fixture_page(&mut self, sid: &str, tx: &mut rune::Tx) {
        let Some(t) = self.transcripts.get(sid) else { return };
        let messages = self.fixture_run.served.page(t);
        let page = crate::session_pages::Page { key: self.page_key(sid), messages, cursor: None, stubs: Vec::new(), asked: 0, history: false, images: Vec::new(), bytes: 0, read: Default::default() };
        self.show_page(&page, tx);
    }
}

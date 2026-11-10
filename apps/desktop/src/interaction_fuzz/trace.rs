//! What the screen shows and what the server has, read from the app each frame, and the
//! short text timeline a failure prints: one line each time either changes.
//!
//! ```text
//!  12.40  > Send p3
//!  12.42  screen [h1 p1 p2 ‖ p3(Pending)] Stop working   server [h1 p1 p2] inbox[p3] busy   link fine
//! ```
//!
//! `‖` is where the turn's Working row (`turn-actions`) sits among the prompts.

use std::collections::{HashMap, HashSet};

use opencode_kit::model::Message;

use super::oracle::Truth;
use crate::App;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Screen {
    /// The prompts' ids top to bottom, from the timeline's `u-{id}` rows.
    pub prompts: Vec<String>,
    /// How many prompts sit above the Working row, when it shows.
    pub actions_at: Option<usize>,
    pub pending: HashSet<String>,
    /// Prompts whose send timed out, marked Not confirmed until a receipt settles them.
    pub sending: HashSet<String>,
    /// Prompts marked Failed to send, kept in place with Retry and Edit.
    pub failed: HashSet<String>,
    pub queued: Vec<String>,
    /// `App::busy` for the open session.
    pub busy: bool,
    /// The composer button's label.
    pub button: String,
    pub working: bool,
    pub stopping: bool,
    pub composer_empty: bool,
    /// The toast up now: (id, title, description).
    pub toast: Option<(u64, String, String)>,
    /// The words each prompt shows, by id.
    pub words: HashMap<String, String>,
}

impl Screen {
    pub fn read(rt: &rune::Runtime<App>, sid: &str) -> Screen {
        let app = &rt.state;
        let mut s = Screen::default();
        if let Some(t) = app.transcripts.get(sid) {
            for m in &t.messages {
                if let Message::User { id, text } = m {
                    s.words.insert(id.clone(), text.clone());
                }
            }
            for key in session_view::timeline::row_keys(app, t) {
                if let Some(id) = key.strip_prefix("u-") {
                    s.prompts.push(id.to_owned());
                } else if key == "turn-actions" && s.actions_at.is_none() {
                    s.actions_at = Some(s.prompts.len());
                }
            }
        }
        if let Some(inbox) = app.inboxes.get(sid) {
            for i in &inbox.items {
                s.words.entry(i.id.clone()).or_insert_with(|| i.text.clone());
            }
            s.pending = inbox.steering().map(|i| i.id.clone()).collect();
            s.queued = inbox.queued().map(|i| i.id.clone()).collect();
        }
        s.sending = s.words.keys().filter(|id| app.unsent.sending(id)).cloned().collect();
        s.failed = s.words.keys().filter(|id| app.unsent.state(id) == Some(crate::send_failed::State::Failed)).cloned().collect();
        s.busy = app.busy(sid);
        s.stopping = app.stops.has(sid);
        s.composer_empty = app.composer.text.trim().is_empty();
        for e in rt.elements() {
            if e.exiting {
                continue;
            }
            match e.key.as_deref() {
                Some("submit") => s.button = e.label.clone().unwrap_or_default(),
                Some("working") if e.opacity > 0.5 => s.working = true,
                _ => {}
            }
        }
        s.toast = app.toast.current().map(|e| (e.id, e.title.clone(), e.description.clone().unwrap_or_default()));
        s
    }

    /// Whether the Working row sits below prompt `id` (the prompt is above it).
    pub fn above_actions(&self, id: &str) -> Option<bool> {
        let at = self.actions_at?;
        self.prompts.iter().position(|p| p == id).map(|k| k < at)
    }
}

/// Short names: a prompt's words (`p3`), else its id's tail.
pub fn label(words: &HashMap<String, String>, id: &str) -> String {
    match words.get(id) {
        Some(w) if !w.is_empty() && w.len() <= 8 => w.clone(),
        _ => format!("…{}", &id[id.len().saturating_sub(4)..]),
    }
}

pub fn screen_line(s: &Screen) -> String {
    let mut parts = vec![];
    for (k, id) in s.prompts.iter().enumerate() {
        if s.actions_at == Some(k) {
            parts.push("‖".to_owned());
        }
        let mut p = label(&s.words, id);
        if s.sending.contains(id) {
            p.push_str("(Not confirmed)");
        } else if s.failed.contains(id) {
            p.push_str("(Failed)");
        } else if s.pending.contains(id) {
            p.push_str("(Pending)");
        }
        parts.push(p);
    }
    if s.actions_at == Some(s.prompts.len()) {
        parts.push("‖".to_owned());
    }
    let mut line = format!("[{}] {}", parts.join(" "), if s.button.is_empty() { "?" } else { &s.button });
    if s.working {
        line.push_str(" working");
    }
    if s.stopping {
        line.push_str(" stopping");
    }
    if !s.queued.is_empty() {
        let q: Vec<String> = s.queued.iter().map(|id| label(&s.words, id)).collect();
        line.push_str(&format!(" queue[{}]", q.join(" ")));
    }
    if let Some((_, title, desc)) = &s.toast {
        line.push_str(&format!(" toast \"{title}\""));
        if !desc.is_empty() {
            line.push_str(&format!(" / \"{desc}\""));
        }
    }
    line
}

pub fn server_line(truth: &dyn Truth) -> String {
    let words = truth.words();
    let users: Vec<String> = truth.users().iter().map(|id| label(&words, id)).collect();
    let mut line = format!("[{}]", users.join(" "));
    let inbox = truth.inbox();
    if !inbox.is_empty() {
        let items: Vec<String> = inbox.iter().map(|(_, text, queue)| if *queue { format!("q:{text}") } else { text.clone() }).collect();
        line.push_str(&format!(" inbox[{}]", items.join(" ")));
    }
    line.push_str(if !truth.up() { " down" } else if truth.busy() { " busy" } else { " idle" });
    if truth.helpers() > 0 {
        line.push_str(&format!(" +{} helper", truth.helpers()));
    }
    line
}

/// The timeline: ops, failures, and a line whenever screen, server or link changes.
#[derive(Default)]
pub struct Trace {
    pub lines: Vec<String>,
    last: String,
    pub quiet: bool,
}

impl Trace {
    pub fn note(&mut self, t: f64, what: impl AsRef<str>) {
        if !self.quiet {
            self.lines.push(format!("{t:7.2}  {}", what.as_ref()));
        }
    }

    pub fn frame(&mut self, t: f64, screen: &Screen, truth: &dyn Truth, link: &str) {
        if self.quiet {
            return;
        }
        let now = format!("screen {}   server {}   link {link}", screen_line(screen), server_line(truth));
        if now != self.last {
            self.lines.push(format!("{t:7.2}  {now}"));
            self.last = now;
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
}

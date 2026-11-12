//! What a screen reader hears of the agent's work without moving focus, through `Tx::announce`.
//!
//! The rule: never a reply's tokens, never silence. NVDA and Narrator queue polite UIA
//! live-region changes and speak each one whole, so every token as it streams would bury the
//! user; nothing at all would leave them guessing whether the agent heard. For the session on
//! screen the reader hears that work started, each tool as it starts (at most one every
//! [`TOOL_GAP`] seconds, never the same twice running), and the end with the reply's first
//! sentence; the reply itself is read at the user's pace in the timeline. A failure and a
//! request that waits on the user (permission, question) interrupt, from any session with a
//! tab; another session finishing is said politely.

use rune::{LiveRegion, Tx};
use serde_json::Value;

use crate::App;
use opencode_kit::model::{Message, Part};

/// The least time between two tools said for one run, in seconds.
const TOOL_GAP: f64 = 4.0;
/// A reply's first sentence is cut here, in characters.
const SENTENCE_MAX: usize = 160;

/// The open session's run, as speech follows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Turn {
    #[default]
    Idle,
    /// `said`: the last tool spoken, and when (the app's clock, seconds).
    Working { session: String, said: Option<(String, f64)> },
}

/// What the stream said, as speech hears it.
#[derive(Clone, Debug, PartialEq)]
pub enum Heard {
    Started { session: String },
    Tool { session: String, step: String, now: f64 },
    Succeeded { session: String, reply: Option<String> },
    Failed { session: String, error: Option<String> },
    Interrupted { session: String },
    /// A request that waits on the user, already in words.
    Asked { words: String },
    /// Another session's run ended: its title and how.
    Elsewhere { title: String, failed: bool },
}

/// One thing to say.
#[derive(Clone, Debug, PartialEq)]
pub struct Say {
    pub words: String,
    pub how: LiveRegion,
}

/// The speech machine: the open session's run, and what waits to be said on the next poll.
#[derive(Debug, Default)]
pub struct Speech {
    turn: Turn,
    queued: Option<Say>,
    /// A request arrived in the open session: the key of what takes focus on the next poll, if
    /// focus is then in the composer the dock replaces, or nowhere.
    focus_dock: Option<&'static str>,
}

impl Speech {
    /// Queues `words` for the next poll (a toast's message).
    pub fn say(&mut self, words: String, how: LiveRegion) {
        if !words.trim().is_empty() {
            self.queued = Some(Say { words, how });
        }
    }

    /// What's queued interrupts rather than waits (a failure's toast).
    pub fn urgent(&mut self) {
        if let Some(say) = &mut self.queued {
            say.how = LiveRegion::Assertive;
        }
    }
}

/// One step of the machine: the next state and what to say, if anything.
pub fn step(turn: Turn, heard: Heard) -> (Turn, Option<Say>) {
    let polite = |words: String| Some(Say { words, how: LiveRegion::Polite });
    let assertive = |words: String| Some(Say { words, how: LiveRegion::Assertive });
    match (turn, heard) {
        (_, Heard::Started { session }) => (Turn::Working { session, said: None }, polite(opencode_i18n::t!("rune.a11y.working").to_owned())),
        (Turn::Working { session, said }, Heard::Tool { session: s, step, now }) if s == session => {
            let quiet = said.as_ref().is_some_and(|(last, at)| *last == step || now - at < TOOL_GAP);
            if quiet {
                return (Turn::Working { session, said }, None);
            }
            (Turn::Working { session, said: Some((step.clone(), now)) }, polite(step))
        }
        (Turn::Working { session, .. }, Heard::Succeeded { session: s, reply }) if s == session => {
            let words = match reply {
                Some(r) => opencode_i18n::t!("rune.a11y.replied", reply = r).to_owned(),
                None => opencode_i18n::t!("rune.a11y.done").to_owned(),
            };
            (Turn::Idle, polite(words))
        }
        (Turn::Working { session, .. }, Heard::Failed { session: s, error }) if s == session => {
            let error = error.unwrap_or_else(|| opencode_i18n::t!("notification.session.error.fallbackDescription").to_owned());
            (Turn::Idle, assertive(opencode_i18n::t!("rune.a11y.failed", error = error).to_owned()))
        }
        (Turn::Working { session, .. }, Heard::Interrupted { session: s }) if s == session => {
            (Turn::Idle, polite(opencode_i18n::t!("rune.a11y.stopped").to_owned()))
        }
        (turn, Heard::Asked { words }) => (turn, assertive(words)),
        (turn, Heard::Elsewhere { title, failed }) => {
            let key = if failed { opencode_i18n::t!("sidebar.inbox.announce.failed", title = title) } else { opencode_i18n::t!("sidebar.inbox.announce.done", title = title) };
            (turn, polite(key.to_owned()))
        }
        (turn, _) => (turn, None),
    }
}

/// Feeds one stream event to the machine (from `App::react`).
pub fn event(app: &mut App, ty: &str, data: &Value, sid: &str) {
    let Some(heard) = hear(app, ty, data, sid) else { return };
    if matches!(heard, Heard::Asked { .. }) && open_root(app) == Some(app.root_of(sid).to_owned()) {
        app.speech.focus_dock = Some(if ty == "permission.asked" { session_view::dock::permission::PERMISSION_FOCUS } else { "q-opt-0" });
    }
    let (turn, say) = step(std::mem::take(&mut app.speech.turn), heard);
    app.speech.turn = turn;
    if say.is_some() {
        app.speech.queued = say;
    }
}

/// Says what waits, and hands the dock focus it was owed (from `poll`).
pub fn poll(app: &mut App, tx: &mut Tx) {
    if let Some(say) = app.speech.queued.take() {
        tx.announce(say.words, say.how);
    }
    if let Some(key) = app.speech.focus_dock.take()
        && tx.focused().is_none_or(|k| k == "prompt")
    {
        tx.focus(key, tx.focus_visible());
    }
}

fn open_root(app: &App) -> Option<String> {
    match &*app.route {
        crate::Route::Session { id, .. } => Some(app.root_of(id).to_owned()),
        _ => None,
    }
}

fn hear(app: &App, ty: &str, data: &Value, sid: &str) -> Option<Heard> {
    let open = open_root(app).is_some_and(|r| r == sid);
    let session = sid.to_owned();
    Some(match ty {
        "session.execution.started" if open => Heard::Started { session },
        "session.tool.called" if open => Heard::Tool { session, step: crate::agent_rows::step_of(data["name"].as_str().unwrap_or(""), &data["input"]), now: *app.clock },
        "session.execution.succeeded" if open => Heard::Succeeded { session, reply: first_sentence(app, sid) },
        "session.execution.failed" if open => Heard::Failed { session, error: data["error"]["message"].as_str().or(data["error"].as_str()).map(str::to_owned) },
        "session.execution.interrupted" if open => Heard::Interrupted { session },
        "session.execution.succeeded" | "session.execution.failed" => {
            let title = app.tabs.iter().any(|t| t.id == sid).then(|| app.session(sid).map(|s| s.title.clone())).flatten()?;
            Heard::Elsewhere { title, failed: ty == "session.execution.failed" }
        }
        "permission.asked" | "form.created" if !app.tabs.iter().any(|t| t.id == sid || t.id == app.root_of(sid)) => return None,
        "permission.asked" => {
            let action = data["action"].as_str().or(data["permission"].as_str()).unwrap_or("");
            let what = session_view::dock::permission::description(action).map_or(action.to_owned(), str::to_owned);
            let target = data["resources"].as_array().and_then(|r| r.first()).and_then(Value::as_str).unwrap_or("");
            let title = opencode_i18n::t!("notification.permission.title");
            Heard::Asked { words: [title, &what, target].into_iter().filter(|w| !w.is_empty()).collect::<Vec<_>>().join(". ") }
        }
        "form.created" if data["metadata"]["kind"].as_str().or(data["form"]["metadata"]["kind"].as_str()) == Some("question") => {
            let form = if data["form"].is_object() { &data["form"] } else { data };
            let field = &form["fields"][0];
            let ask = field["description"].as_str().or(field["title"].as_str()).or(form["title"].as_str()).unwrap_or("");
            Heard::Asked { words: format!("{}. {ask}", opencode_i18n::t!("notification.question.title")) }
        }
        _ => return None,
    })
}

/// The first sentence of the session's last reply, cut to [`SENTENCE_MAX`].
fn first_sentence(app: &App, sid: &str) -> Option<String> {
    let text = app.transcripts.get(sid)?.messages.iter().rev().find_map(|m| match m {
        Message::Assistant(a) => a.parts.iter().find_map(|p| match p {
            Part::Text { text } if !text.trim().is_empty() => Some(text.as_str()),
            _ => None,
        }),
        _ => None,
    })?;
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?.trim_start_matches(['#', '*', '-', '>', ' ']);
    let end = line.char_indices().find(|&(i, c)| matches!(c, '.' | '!' | '?') && line[i + c.len_utf8()..].starts_with(' ')).map_or(line.len(), |(i, c)| i + c.len_utf8());
    let sentence: String = line[..end].chars().take(SENTENCE_MAX).collect();
    (!sentence.is_empty()).then_some(sentence)
}

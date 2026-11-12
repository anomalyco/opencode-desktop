//! What a screen reader hears of the agent's work without moving focus, through `Tx::announce`.
//!
//! The rule: never a reply's tokens, never silence. NVDA and Narrator queue polite UIA
//! live-region changes and speak each one whole, so every token as it streams would bury the
//! user; nothing at all would leave them guessing whether the agent heard. For the session on
//! screen the reader hears that work started, each tool as it starts (at most one every
//! [`TOOL_GAP`] seconds, never the same twice running), and the end with the reply's first
//! sentence; the reply itself is read at the user's pace in the timeline. A request that waits on
//! the user (a permission, a question) interrupts and, when focus was in the prompt it replaces,
//! takes focus; a failure interrupts; another tab's session finishing or asking is said too.
//!
//! None of it runs while no assistive technology reads the window (`Tx::assistive_technology`).

use rune::{LiveRegion, Tx};
use serde_json::Value;

use crate::App;
use opencode_kit::model::{Message, Part, Pending};

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
    /// Another tab's session asks for the user, in words.
    AskedElsewhere { words: String },
    /// Another tab's session's run ended: its title and how.
    Elsewhere { title: String, failed: bool },
}

/// One thing to say.
#[derive(Clone, Debug, PartialEq)]
pub struct Say {
    pub words: String,
    pub how: LiveRegion,
}

/// The speech machine and what waits to be said on the next poll.
#[derive(Debug, Default)]
pub struct Speech {
    turn: Turn,
    queued: Vec<Say>,
    /// An assistive client read the window at the last poll.
    listening: bool,
    /// Focus was in the prompt, or nowhere, at the last poll: a request that replaces the prompt
    /// may take it.
    in_prompt: bool,
    /// The request the open page's dock showed at the last poll.
    dock: Option<String>,
    /// The open run's tool calls by id, and their tools' names: `session.tool.called` brings
    /// a call's input and `session.tool.input.started` its name.
    tools: Vec<(String, String)>,
}

impl Speech {
    /// Queues `words` for the next poll (a toast's message).
    pub fn say(&mut self, words: String, how: LiveRegion) {
        if self.listening && !words.trim().is_empty() {
            self.queued.push(Say { words, how });
        }
    }

    /// The last thing queued interrupts rather than waits (a failure's toast).
    pub fn urgent(&mut self) {
        if let Some(say) = self.queued.last_mut() {
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
                Some(r) => opencode_i18n::t!("rune.a11y.replied", reply = r),
                None => opencode_i18n::t!("rune.a11y.done").to_owned(),
            };
            (Turn::Idle, polite(words))
        }
        (Turn::Working { session, .. }, Heard::Failed { session: s, error }) if s == session => {
            let error = error.unwrap_or_else(|| opencode_i18n::t!("notification.session.error.fallbackDescription").to_owned());
            (Turn::Idle, assertive(opencode_i18n::t!("rune.a11y.failed", error = error)))
        }
        (Turn::Working { session, .. }, Heard::Interrupted { session: s }) if s == session => {
            (Turn::Idle, polite(opencode_i18n::t!("rune.a11y.stopped").to_owned()))
        }
        (turn, Heard::AskedElsewhere { words }) => (turn, assertive(words)),
        (turn, Heard::Elsewhere { title, failed }) => {
            let words = if failed { opencode_i18n::t!("sidebar.inbox.announce.failed", title = title) } else { opencode_i18n::t!("sidebar.inbox.announce.done", title = title) };
            (turn, polite(words))
        }
        (turn, _) => (turn, None),
    }
}

/// Feeds one stream event to the machine (from `App::react`).
pub fn event(app: &mut App, ty: &str, data: &Value, sid: &str) {
    if !app.speech.listening {
        return;
    }
    if crate::flag("--log-speech") && (ty.starts_with("session.") || ty.ends_with(".asked") || ty.starts_with("form.")) {
        eprintln!("speech: {ty} session {sid:?} open {:?}", open_root(app));
    }
    if ty == "session.tool.input.started"
        && let (Some(id), Some(name)) = (data["id"].as_str(), data["name"].as_str())
    {
        app.speech.tools.push((id.to_owned(), name.to_owned()));
    }
    let Some(heard) = hear(app, ty, data, sid) else { return };
    if matches!(heard, Heard::Started { .. } | Heard::Succeeded { .. } | Heard::Failed { .. } | Heard::Interrupted { .. }) {
        app.speech.tools.clear();
    }
    let (turn, say) = step(std::mem::take(&mut app.speech.turn), heard);
    app.speech.turn = turn;
    app.speech.queued.extend(say);
}

/// Says what waits, and announces and focuses a request the open page's dock started showing
/// (from `poll`). A poll in one frame says everything queued in it as one announcement, so a
/// reader hears it all rather than only the last.
pub fn poll(app: &mut App, tx: &mut Tx) {
    let listening = tx.assistive_technology();
    if !listening {
        if app.speech.listening {
            app.speech = Speech::default();
        }
        return;
    }
    app.speech.listening = true;
    let shown = dock_request(app);
    if shown.as_ref().map(|(id, _, _)| id) != app.speech.dock.as_ref() {
        if let Some((_, words, focus)) = &shown {
            app.speech.queued.push(Say { words: words.clone(), how: LiveRegion::Assertive });
            if app.speech.in_prompt {
                tx.focus(*focus, tx.focus_visible());
            }
        }
        app.speech.dock = shown.map(|(id, _, _)| id);
    }
    let queued = std::mem::take(&mut app.speech.queued);
    if !queued.is_empty() {
        let how = if queued.iter().any(|s| s.how == LiveRegion::Assertive) { LiveRegion::Assertive } else { LiveRegion::Polite };
        let words = queued.into_iter().map(|s| s.words).filter(|w| !w.trim().is_empty()).collect::<Vec<_>>().join(". ");
        if crate::flag("--log-speech") {
            eprintln!("speech: say {how:?} {words:?}");
        }
        tx.announce(words, how);
    }
    app.speech.in_prompt = tx.focused().is_none_or(|k| k == "prompt");
}

/// The request the open page's dock shows: its id, what a reader hears, and the key of what
/// takes focus.
fn dock_request(app: &App) -> Option<(String, String, &'static str)> {
    if let Some(Pending::Permission { id, action, resources, .. }) = session_view::dock::permission::asked(app) {
        return Some((id, permission_words(&action, resources.first().map(String::as_str)), session_view::dock::permission::PERMISSION_FOCUS));
    }
    let form = session_view::dock::question::asking(app)?;
    let field = form.fields.first();
    let ask = field.map(|f| if f.description.is_empty() { f.title.as_str() } else { f.description.as_str() }).unwrap_or("");
    Some((form.id.clone(), format!("{}. {ask}", opencode_i18n::t!("notification.question.title")), "q-opt-0"))
}

/// "Permission required. Run shell commands. git push origin main".
fn permission_words(action: &str, target: Option<&str>) -> String {
    let what = session_view::dock::permission::description(action).unwrap_or_else(|| crate::turns::tool_title(action));
    [opencode_i18n::t!("notification.permission.title"), what, target.unwrap_or("")].into_iter().filter(|w| !w.is_empty()).collect::<Vec<_>>().join(". ")
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
    let has_tab = || app.tabs.iter().any(|t| t.id == sid || t.id == app.root_of(sid));
    Some(match ty {
        "session.execution.started" if open => Heard::Started { session },
        "session.tool.called" if open => {
            let id = data["id"].as_str().unwrap_or("");
            let name = data["name"].as_str().or_else(|| app.speech.tools.iter().find(|(c, _)| c == id).map(|(_, n)| n.as_str()))?;
            Heard::Tool { session, step: crate::agent_rows::step_of(name, &data["input"]), now: *app.clock }
        }
        "session.execution.succeeded" if open => Heard::Succeeded { session, reply: first_sentence(app, sid) },
        "session.execution.failed" if open => Heard::Failed { session, error: data["error"]["message"].as_str().or(data["error"].as_str()).map(str::to_owned) },
        "session.execution.interrupted" if open => Heard::Interrupted { session },
        "session.execution.succeeded" | "session.execution.failed" if has_tab() => {
            let title = app.session(sid).map(|s| s.title.clone())?;
            Heard::Elsewhere { title, failed: ty == "session.execution.failed" }
        }
        // The open page's own requests are said as its dock shows them (`poll`).
        "permission.asked" if !open && has_tab() => {
            let target = data["resources"].as_array().and_then(|r| r.first()).and_then(Value::as_str);
            let title = app.session(sid).map(|s| s.title.clone()).unwrap_or_default();
            Heard::AskedElsewhere { words: format!("{title}: {}", permission_words(data["action"].as_str().unwrap_or(""), target)) }
        }
        "form.created" if !open && has_tab() => {
            let title = app.session(sid).map(|s| s.title.clone()).unwrap_or_default();
            Heard::AskedElsewhere { words: opencode_i18n::t!("sidebar.inbox.announce.input", title = title) }
        }
        _ => return None,
    })
}

/// The first sentence of the session's last reply, cut to [`SENTENCE_MAX`], without Markdown's
/// emphasis marks.
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
    let sentence: String = line[..end].chars().filter(|c| !matches!(c, '*' | '`' | '_')).take(SENTENCE_MAX).collect();
    (!sentence.is_empty()).then_some(sentence)
}

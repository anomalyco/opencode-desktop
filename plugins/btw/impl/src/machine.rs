//! The side questions of every session (`machine.ts`): one chat per tab, each asked once and
//! retried on demand. Events in, the state's next value and the effects to run out; the plugin
//! (`lib.rs`) runs the effects against the host and the session store.

use std::collections::BTreeMap;

/// Longest one answer may take before it fails with Retry; a stalled connection never settles on
/// its own.
pub const GENERATE_TIMEOUT_MS: i64 = 300_000;

/// The model a chat was asked with.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ChatModel {
    pub provider: String,
    pub id: String,
    pub variant: String,
}

/// What a chat was asked with: its question, when, and the agent and model then.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Info {
    pub question: String,
    pub created: Option<i64>,
    pub agent: String,
    pub model: Option<ChatModel>,
}

/// One chat as the session store keeps it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Saved {
    pub id: String,
    pub info: Info,
    pub answer: Option<String>,
    pub duration: Option<i64>,
}

/// Why a chat has no answer. `Interrupted` is a stored chat without an answer: a reload, closed
/// session or disposed plugin abandoned its request. `Empty` is a reply without visible text.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Failure {
    Interrupted,
    Empty,
    Timeout,
    Error(Option<String>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    Running { attempt: u64, started: i64 },
    Answered { answer: String, duration: i64 },
    Failed(Failure),
}

/// A chat this plugin instance asked or retried; chats it never touched are read from the store.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Chat {
    pub info: Info,
    pub status: Status,
}

#[derive(Default)]
pub struct State {
    /// The last attempt id handed out.
    pub issued: u64,
    /// Chats by session, then chat id.
    pub sessions: BTreeMap<String, BTreeMap<String, Chat>>,
}

pub enum Effect {
    PersistQuestion { id: String, info: Info },
    OpenTab(String),
    Generate { id: String, attempt: u64, question: String },
    PersistAnswer { id: String, answer: String, duration: i64, agent: String, model: Option<ChatModel> },
    DeleteChat(String),
}

/// What a chat's tab shows.
pub enum TabView {
    Missing,
    Ready(String),
}

/// What a chat's panel shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    Missing,
    Running,
    Failed(Failure),
    Answered { answer: String, duration: Option<i64> },
}

impl State {
    fn live(&self, session: &str, id: &str) -> Option<&Chat> {
        self.sessions.get(session)?.get(id)
    }

    fn put(&mut self, session: &str, id: &str, chat: Chat) {
        self.sessions.entry(session.to_owned()).or_default().insert(id.to_owned(), chat);
    }

    pub fn asked(&mut self, session: &str, id: &str, info: Info) -> Vec<Effect> {
        let question = info.question.trim().to_owned();
        if question.is_empty() || self.live(session, id).is_some() {
            return Vec::new();
        }
        self.issued += 1;
        let attempt = self.issued;
        let info = Info { question: question.clone(), ..info };
        let started = info.created.unwrap_or(0);
        self.put(session, id, Chat { info: info.clone(), status: Status::Running { attempt, started } });
        vec![
            Effect::PersistQuestion { id: id.to_owned(), info },
            Effect::OpenTab(id.to_owned()),
            Effect::Generate { id: id.to_owned(), attempt, question },
        ]
    }

    /// A stored chat this instance never asked (one restored after a reload) is retryable too.
    pub fn retried(&mut self, session: &str, id: &str, now: i64, saved: Option<&Saved>, agent: &str, model: Option<ChatModel>) -> Vec<Effect> {
        let live = self.live(session, id).cloned();
        let info = match (&live, saved) {
            (Some(c), _) if matches!(c.status, Status::Failed(_)) => c.info.clone(),
            (Some(_), _) => return Vec::new(),
            (None, Some(s)) if s.answer.as_deref().is_none_or(|a| a.trim().is_empty()) => s.info.clone(),
            _ => return Vec::new(),
        };
        self.issued += 1;
        let attempt = self.issued;
        let info = Info {
            agent: if agent.is_empty() { info.agent } else { agent.to_owned() },
            model: model.or(info.model),
            ..info
        };
        let question = info.question.clone();
        self.put(session, id, Chat { info, status: Status::Running { attempt, started: now } });
        vec![Effect::Generate { id: id.to_owned(), attempt, question }]
    }

    /// The running chat of `attempt`, if it still is.
    fn running(&self, session: &str, id: &str, attempt: u64) -> Option<(Info, i64)> {
        match self.live(session, id) {
            Some(Chat { info, status: Status::Running { attempt: a, started } }) if *a == attempt => Some((info.clone(), *started)),
            _ => None,
        }
    }

    pub fn succeeded(&mut self, session: &str, id: &str, attempt: u64, text: &str, now: i64) -> Vec<Effect> {
        let Some((info, started)) = self.running(session, id, attempt) else { return Vec::new() };
        let answer = text.trim().to_owned();
        // A reply without visible text is a failure the user can retry, not an empty answer.
        if answer.is_empty() {
            self.put(session, id, Chat { info, status: Status::Failed(Failure::Empty) });
            return Vec::new();
        }
        let duration = (now - started).max(0);
        let (agent, model) = (info.agent.clone(), info.model.clone());
        self.put(session, id, Chat { info, status: Status::Answered { answer: answer.clone(), duration } });
        vec![Effect::PersistAnswer { id: id.to_owned(), answer, duration, agent, model }]
    }

    pub fn failed(&mut self, session: &str, id: &str, attempt: u64, failure: Failure) -> Vec<Effect> {
        let Some((info, _)) = self.running(session, id, attempt) else { return Vec::new() };
        self.put(session, id, Chat { info, status: Status::Failed(failure) });
        Vec::new()
    }

    pub fn removed(&mut self, session: &str, id: &str) -> Vec<Effect> {
        let remove = Effect::DeleteChat(id.to_owned());
        let Some(chats) = self.sessions.get_mut(session) else { return vec![remove] };
        if chats.remove(id).is_none() {
            return vec![remove];
        }
        if chats.is_empty() {
            self.sessions.remove(session);
        }
        // A request still running answers into no chat (its attempt is gone), so it needs no abort.
        vec![remove]
    }

    /// What `id`'s tab shows: the live chat, else the stored one.
    pub fn tab(&self, saved: &[Saved], session: &str, id: &str) -> TabView {
        match (self.live(session, id), saved.iter().find(|s| s.id == id)) {
            (Some(c), _) => TabView::Ready(c.info.question.clone()),
            (None, Some(s)) => TabView::Ready(s.info.question.clone()),
            _ => TabView::Missing,
        }
    }

    /// What `id`'s panel shows, with what the chat was asked with.
    pub fn view(&self, saved: &[Saved], session: &str, id: &str) -> (Phase, Info) {
        if let Some(live) = self.live(session, id) {
            let phase = match &live.status {
                Status::Running { .. } => Phase::Running,
                Status::Failed(f) => Phase::Failed(f.clone()),
                Status::Answered { answer, duration } => Phase::Answered { answer: answer.clone(), duration: Some(*duration) },
            };
            return (phase, live.info.clone());
        }
        let Some(stored) = saved.iter().find(|s| s.id == id) else { return (Phase::Missing, Info::default()) };
        let phase = match stored.answer.as_deref().map(str::trim) {
            Some(a) if !a.is_empty() => Phase::Answered { answer: a.to_owned(), duration: stored.duration },
            // Builds before this machine stored a blank reply as the answer.
            Some(_) => Phase::Failed(Failure::Empty),
            None => Phase::Failed(Failure::Interrupted),
        };
        (phase, stored.info.clone())
    }

    /// The attempts still running, with their session, chat and deadline.
    pub fn running_attempts(&self) -> impl Iterator<Item = (&str, &str, u64, i64)> {
        self.sessions.iter().flat_map(|(s, chats)| {
            chats.iter().filter_map(move |(id, c)| match c.status {
                Status::Running { attempt, started } => Some((s.as_str(), id.as_str(), attempt, started + GENERATE_TIMEOUT_MS)),
                _ => None,
            })
        })
    }
}

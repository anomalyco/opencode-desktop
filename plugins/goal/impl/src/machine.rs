//! What the window knows of each session's goal (`machine.ts`): only the routed session is
//! fetched; another keeps the goal it last showed (`Stale`), hidden, so switching back shows it
//! while the refetch runs. Events in; the state's next value and the effects to run out.

use std::collections::BTreeMap;

/// How long `get` may take before the snapshot counts as failed.
pub const SNAPSHOT_TIMEOUT_MS: i64 = 15_000;
/// How long a pause, resume or clear may take before its button frees up again.
pub const COMMAND_TIMEOUT_MS: i64 = 15_000;
/// Waits before the 1st, 2nd and 3rd refetch of a snapshot that failed; then the session shows
/// no goal.
const RETRY_DELAYS: [i64; 3] = [1_000, 2_000, 4_000];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    Active,
    Paused,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Goal {
    pub objective: String,
    pub status: Status,
}

impl Goal {
    /// `Goal.State` as the server sends it; `None` for `null` or anything unreadable.
    pub fn parse(v: &serde_json::Value) -> Option<Goal> {
        let status = match v["status"].as_str()? {
            "active" => Status::Active,
            "paused" => Status::Paused,
            "complete" => Status::Complete,
            _ => return None,
        };
        Some(Goal { objective: v["objective"].as_str().unwrap_or_default().to_owned(), status })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Pause,
    Resume,
    Clear,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::Pause => "pause",
            Action::Resume => "resume",
            Action::Clear => "clear",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Entry {
    /// Fetching the snapshot; `cached` shows until the answer lands.
    Loading { attempt: u64, retries: usize, cached: Option<Goal> },
    /// The last fetch failed; refetches when the timer of `attempt` fires. Shows no goal.
    Retrying { attempt: u64, retries: usize },
    /// The snapshot or a later change: authoritative while subscribed.
    Live(Option<Goal>),
    /// Not routed, or its server disconnected: kept to show while revalidating, never alone.
    Stale(Goal),
    /// Every fetch failed (a server without the goal plugin). A change still brings it live.
    Failed,
}

/// The routed session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub key: String,
    pub directory: String,
}

#[derive(Default)]
pub struct State {
    issued: u64,
    target: Option<Target>,
    sessions: BTreeMap<String, Entry>,
    /// The one pause, resume or clear in flight per session.
    commands: BTreeMap<String, (u64, Action)>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    Fetch { attempt: u64, session: String, directory: String },
    ScheduleRetry { attempt: u64, delay: i64 },
    Command { attempt: u64, session: String, action: Action },
    /// Stops the timer or fetch of `attempt`; its result, if any, is ignored.
    Cancel(u64),
    ToastError(Option<String>),
}

impl State {
    pub fn target(&self) -> Option<&Target> {
        self.target.as_ref()
    }

    fn next(&mut self) -> u64 {
        self.issued += 1;
        self.issued
    }

    /// The goal the pill shows: a live answer, or the cached one while it revalidates.
    pub fn goal(&self, key: &str) -> Option<&Goal> {
        match self.sessions.get(key)? {
            Entry::Live(goal) => goal.as_ref(),
            Entry::Loading { cached, .. } => cached.as_ref(),
            _ => None,
        }
    }

    /// Whether a pause, resume or clear for the session is in flight.
    pub fn busy(&self, key: &str) -> bool {
        self.commands.contains_key(key)
    }

    pub fn attach(&mut self, key: &str, directory: &str) -> Vec<Effect> {
        if self.target.as_ref().is_some_and(|t| t.key == key && t.directory == directory) {
            return Vec::new();
        }
        let mut effects = self.detach();
        let cached = match self.sessions.get(key) {
            Some(Entry::Stale(goal)) => Some(goal.clone()),
            _ => None,
        };
        let attempt = self.next();
        self.sessions.insert(key.to_owned(), Entry::Loading { attempt, retries: 0, cached });
        self.target = Some(Target { key: key.to_owned(), directory: directory.to_owned() });
        effects.push(Effect::Fetch { attempt, session: key.to_owned(), directory: directory.to_owned() });
        effects
    }

    /// Leaving the routed session keeps only a goal it showed, as `Stale`.
    pub fn detach(&mut self) -> Vec<Effect> {
        let Some(target) = self.target.take() else { return Vec::new() };
        let shown = self.goal(&target.key).cloned();
        let cancel = match self.sessions.get(&target.key) {
            Some(Entry::Loading { attempt, .. } | Entry::Retrying { attempt, .. }) => vec![Effect::Cancel(*attempt)],
            _ => Vec::new(),
        };
        match shown {
            Some(goal) => self.sessions.insert(target.key, Entry::Stale(goal)),
            None => self.sessions.remove(&target.key),
        };
        cancel
    }

    /// A change is newer than any snapshot in flight. Another session's change updates only a
    /// goal this window showed.
    pub fn changed(&mut self, key: &str, goal: Option<Goal>) -> Vec<Effect> {
        let Some(target) = &self.target else { return Vec::new() };
        if key == target.key {
            let cancel = match self.sessions.get(key) {
                Some(Entry::Loading { attempt, .. } | Entry::Retrying { attempt, .. }) => vec![Effect::Cancel(*attempt)],
                _ => Vec::new(),
            };
            self.sessions.insert(key.to_owned(), Entry::Live(goal));
            return cancel;
        }
        if !self.sessions.contains_key(key) {
            return Vec::new();
        }
        match goal {
            Some(goal) => self.sessions.insert(key.to_owned(), Entry::Stale(goal)),
            None => self.sessions.remove(key),
        };
        Vec::new()
    }

    /// The routed session's entry while its snapshot `attempt` is in flight.
    fn loading(&self, attempt: u64) -> Option<(String, usize)> {
        let key = &self.target.as_ref()?.key;
        match self.sessions.get(key) {
            Some(Entry::Loading { attempt: a, retries, .. }) if *a == attempt => Some((key.clone(), *retries)),
            _ => None,
        }
    }

    pub fn snapshot_loaded(&mut self, attempt: u64, goal: Option<Goal>) -> Vec<Effect> {
        if let Some((key, _)) = self.loading(attempt) {
            self.sessions.insert(key, Entry::Live(goal));
        }
        Vec::new()
    }

    /// A failed refetch drops the cached goal at once: it can no longer be trusted as current.
    pub fn snapshot_failed(&mut self, attempt: u64) -> Vec<Effect> {
        let Some((key, retries)) = self.loading(attempt) else { return Vec::new() };
        let Some(delay) = RETRY_DELAYS.get(retries).copied() else {
            self.sessions.insert(key, Entry::Failed);
            return Vec::new();
        };
        let next = self.next();
        self.sessions.insert(key, Entry::Retrying { attempt: next, retries: retries + 1 });
        vec![Effect::ScheduleRetry { attempt: next, delay }]
    }

    pub fn retry_due(&mut self, attempt: u64) -> Vec<Effect> {
        let Some(target) = self.target.clone() else { return Vec::new() };
        let Some(Entry::Retrying { attempt: a, retries }) = self.sessions.get(&target.key).cloned() else { return Vec::new() };
        if a != attempt {
            return Vec::new();
        }
        self.sessions.insert(target.key.clone(), Entry::Loading { attempt, retries, cached: None });
        vec![Effect::Fetch { attempt, session: target.key, directory: target.directory }]
    }

    pub fn command(&mut self, key: &str, action: Action) -> Vec<Effect> {
        // A second click while the first is in flight would fail on the server.
        if self.commands.contains_key(key) {
            return Vec::new();
        }
        let attempt = self.next();
        self.commands.insert(key.to_owned(), (attempt, action));
        vec![Effect::Command { attempt, session: key.to_owned(), action }]
    }

    /// The command of `attempt` settled; `failed` with the server's message, or `None` on a
    /// timeout.
    pub fn command_settled(&mut self, key: &str, attempt: u64, failed: Option<Option<String>>) -> Vec<Effect> {
        if self.commands.get(key).map(|c| c.0) != Some(attempt) {
            return Vec::new();
        }
        self.commands.remove(key);
        match failed {
            Some(message) => vec![Effect::ToastError(message)],
            None => Vec::new(),
        }
    }
}

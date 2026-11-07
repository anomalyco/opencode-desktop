//! The SSH main side's state machine (`main-machine.ts`): each server's connection, fenced by the
//! `attempt` that started it. An interactive attempt (the window asked) shows its sign-in
//! prompts; a background one (a saved server restored at launch) pauses in `authentication` at
//! its first. A saved server that fails for a reason another try can fix reconnects by itself,
//! five times, 3 s apart. The app has one window, so "the window that started it" is any start
//! that isn't a background restore.

use crate::command::Code;

pub type Attempt = u64;

pub const RETRY_LIMIT: u32 = 5;
pub const RETRY_DELAY_MS: u64 = 3_000;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Config {
    pub id: String,
    pub target: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Http {
    pub url: String,
    pub password: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Prompt {
    pub id: String,
    pub text: String,
    pub confirm: bool,
}

/// Where a saved server returns when its attempt is cancelled.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Resting {
    Disconnected,
    Authentication,
    Incompatible(String),
    Failed { code: Code, detail: String, failures: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    Disconnected,
    Authentication,
    Incompatible(String),
    Connecting { attempt: Attempt, interactive: bool, failures: u32, resting: Resting },
    Authenticating { attempt: Attempt, failures: u32, resting: Resting, prompt: Prompt },
    Ready { attempt: Attempt, http: Http },
    Failed { code: Code, detail: String, failures: u32, retry: Option<Attempt> },
}

impl Phase {
    fn active(&self) -> Option<(Attempt, u32, &Resting)> {
        match self {
            Phase::Connecting { attempt, failures, resting, .. } | Phase::Authenticating { attempt, failures, resting, .. } => Some((*attempt, *failures, resting)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Record {
    pub config: Config,
    /// The config last saved; a saved server keeps it while an attempt tries another.
    pub saved: Option<Config>,
    pub destination: Option<String>,
    pub phase: Phase,
}

/// What windows see of a server (`SshItem`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    Disconnected,
    Connecting,
    Ready,
    Authentication,
    Incompatible,
    Failed,
}

impl Record {
    pub fn stage(&self) -> Stage {
        match self.phase {
            Phase::Disconnected => Stage::Disconnected,
            Phase::Authentication | Phase::Authenticating { .. } => Stage::Authentication,
            Phase::Incompatible(_) => Stage::Incompatible,
            Phase::Connecting { .. } => Stage::Connecting,
            Phase::Ready { .. } => Stage::Ready,
            Phase::Failed { .. } => Stage::Failed,
        }
    }

    pub fn prompt(&self) -> Option<&Prompt> {
        match &self.phase {
            Phase::Authenticating { prompt, .. } => Some(prompt),
            _ => None,
        }
    }

    pub fn http(&self) -> Option<&Http> {
        match &self.phase {
            Phase::Ready { http, .. } => Some(http),
            _ => None,
        }
    }

    pub fn error(&self) -> Option<Code> {
        match &self.phase {
            Phase::Failed { code, .. } => Some(*code),
            Phase::Incompatible(_) => Some(Code::Version),
            _ => None,
        }
    }

}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct State {
    pub issued: Attempt,
    pub servers: Vec<Record>,
}

#[derive(Clone, Debug)]
pub enum Event {
    Start { config: Config, replace: bool, background: bool },
    DestinationResolved { id: String, attempt: Attempt, destination: String },
    PromptReceived { id: String, attempt: Attempt, prompt: Prompt },
    PromptCleared { id: String, attempt: Attempt, prompt: String },
    Respond { id: String, prompt: String, value: String },
    Ready { id: String, attempt: Attempt, http: Http },
    Failed { id: String, attempt: Attempt, code: Code, detail: String },
    RetryDue { id: String, attempt: Attempt },
    Cancel { id: String },
    Forget { id: String },
}

#[derive(Clone, Debug)]
pub enum Effect {
    AttemptStart { id: String, attempt: Attempt, config: Config, replace: bool },
    AttemptStop(Attempt),
    AttemptRespond { attempt: Attempt, prompt: String, value: String },
    RetrySchedule { id: String, attempt: Attempt },
    Save(Vec<Config>),
}

pub fn create(configs: Vec<Config>) -> State {
    State { issued: 0, servers: configs.into_iter().map(|c| Record { saved: Some(c.clone()), config: c, destination: None, phase: Phase::Disconnected }).collect() }
}

fn save(s: &State) -> Effect {
    Effect::Save(s.servers.iter().filter_map(|r| r.saved.clone()).collect())
}

/// Stops whatever runs for a server: its connect, its tunnel, its retry.
fn halt(phase: &Phase) -> Vec<Effect> {
    match phase {
        Phase::Connecting { attempt, .. } | Phase::Authenticating { attempt, .. } | Phase::Ready { attempt, .. } => vec![Effect::AttemptStop(*attempt)],
        Phase::Failed { retry: Some(attempt), .. } => vec![Effect::AttemptStop(*attempt)],
        _ => vec![],
    }
}

fn resting(phase: &Phase) -> Resting {
    match phase {
        Phase::Connecting { resting, .. } | Phase::Authenticating { resting, .. } => resting.clone(),
        Phase::Authentication => Resting::Authentication,
        Phase::Incompatible(d) => Resting::Incompatible(d.clone()),
        Phase::Failed { code, detail, failures, .. } => Resting::Failed { code: *code, detail: detail.clone(), failures: *failures },
        _ => Resting::Disconnected,
    }
}

fn rest(r: Resting) -> Phase {
    match r {
        Resting::Disconnected => Phase::Disconnected,
        Resting::Authentication => Phase::Authentication,
        Resting::Incompatible(d) => Phase::Incompatible(d),
        Resting::Failed { code, detail, failures } => Phase::Failed { code, detail, failures, retry: None },
    }
}

pub fn transition(s: &mut State, event: Event) -> Vec<Effect> {
    let find = |s: &State, id: &str| s.servers.iter().position(|r| r.config.id == id);
    match event {
        Event::Start { config, replace, background } => {
            let id = config.id.clone();
            if id.is_empty() || id.len() > 80 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return vec![];
            }
            let at = find(s, &id);
            let phase = at.map_or(Phase::Disconnected, |i| s.servers[i].phase.clone());
            // A background restore joins a server already connecting or up.
            if background && (phase.active().is_some() || matches!(phase, Phase::Ready { .. })) {
                return vec![];
            }
            s.issued += 1;
            let attempt = s.issued;
            let config = Config { id: id.clone(), target: config.target.trim().to_owned(), name: config.name.trim().to_owned() };
            let failures = match &phase {
                Phase::Failed { failures, .. } if background => *failures,
                _ => 0,
            };
            let record = Record {
                config: config.clone(),
                saved: at.and_then(|i| s.servers[i].saved.clone()),
                destination: at.and_then(|i| s.servers[i].destination.clone()),
                phase: Phase::Connecting { attempt, interactive: !background, failures, resting: resting(&phase) },
            };
            match at {
                Some(i) => s.servers[i] = record,
                None => s.servers.push(record),
            }
            let mut effects = halt(&phase);
            effects.push(Effect::AttemptStart { id, attempt, config, replace });
            effects
        }
        Event::Cancel { id } => {
            let Some(i) = find(s, &id) else { return vec![] };
            let Some((attempt, _, resting)) = s.servers[i].phase.active() else { return vec![] };
            let resting = resting.clone();
            // A server the window was adding is gone with its attempt.
            match s.servers[i].saved.clone() {
                None => {
                    s.servers.remove(i);
                }
                Some(saved) => {
                    s.servers[i].config = saved;
                    s.servers[i].phase = rest(resting);
                }
            }
            vec![Effect::AttemptStop(attempt)]
        }
        Event::Forget { id } => {
            let Some(i) = find(s, &id) else { return vec![] };
            let r = s.servers.remove(i);
            let mut effects = halt(&r.phase);
            if r.saved.is_some() {
                effects.push(save(s));
            }
            effects
        }
        Event::RetryDue { id, attempt } => {
            let Some(i) = find(s, &id) else { return vec![] };
            let Phase::Failed { code, detail, failures, retry: Some(a) } = s.servers[i].phase.clone() else { return vec![] };
            if a != attempt {
                return vec![];
            }
            s.issued += 1;
            let attempt = s.issued;
            s.servers[i].phase = Phase::Connecting { attempt, interactive: false, failures, resting: Resting::Failed { code, detail, failures } };
            vec![Effect::AttemptStart { id, attempt, config: s.servers[i].config.clone(), replace: false }]
        }
        Event::Respond { id, prompt, value } => {
            let Some(i) = find(s, &id) else { return vec![] };
            match &s.servers[i].phase {
                Phase::Authenticating { attempt, prompt: p, .. } if p.id == prompt => vec![Effect::AttemptRespond { attempt: *attempt, prompt, value }],
                _ => vec![],
            }
        }
        Event::Failed { id, attempt, code, detail } => {
            let Some(i) = find(s, &id) else { return vec![] };
            let failures = match &s.servers[i].phase {
                Phase::Ready { attempt: a, .. } if *a == attempt => 0,
                p => match p.active() {
                    Some((a, f, _)) if a == attempt => f,
                    _ => return vec![],
                },
            } + 1;
            failed(s, i, code, detail, failures)
        }
        Event::DestinationResolved { id, attempt, destination } => {
            let Some(i) = find(s, &id) else { return vec![] };
            if s.servers[i].phase.active().map(|a| a.0) == Some(attempt) {
                s.servers[i].destination = Some(destination);
            }
            vec![]
        }
        Event::PromptReceived { id, attempt, prompt } => {
            let Some(i) = find(s, &id) else { return vec![] };
            match s.servers[i].phase.clone() {
                // A background attempt has nobody to answer: the server waits for the user to sign in.
                Phase::Connecting { attempt: a, interactive: false, .. } if a == attempt => {
                    s.servers[i].phase = Phase::Authentication;
                    vec![Effect::AttemptStop(attempt)]
                }
                Phase::Connecting { attempt: a, failures, resting, .. } | Phase::Authenticating { attempt: a, failures, resting, .. } if a == attempt => {
                    s.servers[i].phase = Phase::Authenticating { attempt, failures, resting, prompt };
                    vec![]
                }
                _ => vec![],
            }
        }
        Event::PromptCleared { id, attempt, prompt } => {
            let Some(i) = find(s, &id) else { return vec![] };
            if let Phase::Authenticating { attempt: a, failures, resting, prompt: p } = s.servers[i].phase.clone()
                && a == attempt
                && p.id == prompt
            {
                s.servers[i].phase = Phase::Connecting { attempt, interactive: true, failures, resting };
            }
            vec![]
        }
        Event::Ready { id, attempt, http } => {
            let Some(i) = find(s, &id) else { return vec![] };
            if s.servers[i].phase.active().map(|a| a.0) != Some(attempt) {
                return vec![];
            }
            let r = &mut s.servers[i];
            r.saved = Some(r.config.clone());
            r.phase = Phase::Ready { attempt, http };
            vec![save(s)]
        }
    }
}

fn failed(s: &mut State, i: usize, reported: Code, detail: String, failures: u32) -> Vec<Effect> {
    let code = if detail.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") || detail.contains("Host key verification failed") {
        Code::HostKey
    } else if detail.contains("spawn") && detail.contains("ENOENT") {
        Code::SshMissing
    } else {
        reported
    };
    // A failed attempt shows the saved server again, not the target it tried.
    let config = s.servers[i].saved.clone().unwrap_or_else(|| s.servers[i].config.clone());
    s.servers[i].config = config;
    if code == Code::Version {
        s.servers[i].phase = Phase::Incompatible(detail);
        return vec![];
    }
    // Only a saved server reconnects by itself, and only from failures another try can fix.
    let retry = s.servers[i].saved.is_some() && failures < RETRY_LIMIT && !matches!(code, Code::Input | Code::Platform | Code::Unpublished | Code::HostKey | Code::SshMissing) && !detail.contains("Permission denied");
    if !retry {
        s.servers[i].phase = Phase::Failed { code, detail, failures, retry: None };
        return vec![];
    }
    s.issued += 1;
    let attempt = s.issued;
    let id = s.servers[i].config.id.clone();
    s.servers[i].phase = Phase::Failed { code, detail, failures, retry: Some(attempt) };
    vec![Effect::RetrySchedule { id, attempt }]
}

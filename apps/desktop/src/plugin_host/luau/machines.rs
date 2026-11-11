//! The Luau host's state machines, each an `enum` and a pure `step(state, event) -> (state,
//! effects)`: a plugin's life ([`Life`]), the trust its files have ([`Trust`]), its hot reload
//! ([`Hot`]) and a cached server read ([`Ticket`]). The host carries the effects out; nothing
//! here touches the app.
//!
//! Official Desktop's main-process plugin host keeps a plugin's life in `lifecycle-machine.ts`;
//! trust and hot reload have no Desktop counterpart.

use std::rc::Rc;

use rune_luau::Script;

use super::hot::Prepared;

// ---- Life ------------------------------------------------------------------------------------

/// Where a plugin is in its life.
///
/// ```text
/// Discovered ─┬─ Refuse ──▶ Refused
///             ├─ NeedTrust ▶ AwaitingTrust ─ Load ─┐
///             ├─ Disable ──▶ Disabled ───── Load ──┤
///             └─ Load ─────────────────────────────┴▶ Loading ─ Started ──▶ Running ⇄ Reloading
///                                                       └─ StartFailed ▶ Errored ─ Load ▶ Loading
/// Running / Reloading / Errored ─ Disable ▶ Disabled
/// ```
pub enum Life {
    Discovered,
    /// Its manifest, platform or id keeps it from loading.
    Refused(String),
    /// A third-party plugin whose files the user hasn't OKed ([`Trust`]).
    AwaitingTrust,
    Disabled,
    /// Its window half's script is being started.
    Loading,
    Running(Script),
    /// A new VM for its new files is being swapped in; the running one keeps serving until then.
    Reloading(Script),
    /// Its setup failed. The script stays, so its surfaces show the error card with Reload.
    Errored { error: String, script: Script },
}

pub enum LifeEvent {
    Refuse(String),
    NeedTrust,
    Disable,
    Load,
    Started(Script),
    StartFailed(Script, String),
    Reload,
    Reloaded(Result<(), String>),
}

pub enum LifeEffect {
    /// Start the window half's script ([`super::start`]).
    Start,
    /// Stop this script: fire `Disposing`, withdraw what it contributed, reap its processes.
    Stop(Script),
    /// Write a line to the console.
    Report(String),
}

impl Life {
    pub fn step(self, event: LifeEvent) -> (Life, Vec<LifeEffect>) {
        use Life as L;
        use LifeEffect as F;
        use LifeEvent as E;
        match (self, event) {
            (L::Discovered, E::Refuse(why)) => (L::Refused(why), vec![]),
            (L::Discovered | L::Disabled, E::NeedTrust) => (L::AwaitingTrust, vec![]),
            (L::Discovered | L::AwaitingTrust, E::Disable) => (L::Disabled, vec![]),
            (L::Running(s) | L::Reloading(s) | L::Errored { script: s, .. }, E::Disable) => (L::Disabled, vec![F::Stop(s)]),
            (L::Discovered | L::AwaitingTrust | L::Disabled, E::Load) => (L::Loading, vec![F::Start]),
            (L::Errored { script, .. }, E::Load | E::Reload) => (L::Loading, vec![F::Stop(script), F::Start]),
            (L::Loading, E::Started(s)) => (L::Running(s), vec![]),
            (L::Loading, E::StartFailed(script, error)) => (L::Errored { error: error.clone(), script }, vec![F::Report(error)]),
            (L::Running(s), E::Reload) => (L::Reloading(s), vec![]),
            (L::Reloading(s), E::Reloaded(Ok(()))) => (L::Running(s), vec![]),
            // A failed reload keeps the running VM; the console says why.
            (L::Reloading(s), E::Reloaded(Err(e))) => (L::Running(s), vec![F::Report(e)]),
            (state, _) => (state, vec![]),
        }
    }

    /// The script, in the states that have one.
    pub fn script(&self) -> Option<&Script> {
        match self {
            Life::Running(s) | Life::Reloading(s) | Life::Errored { script: s, .. } => Some(s),
            _ => None,
        }
    }

    /// Whether its contributions are live: running, or swapping in new files.
    pub fn running(&self) -> bool {
        matches!(self, Life::Running(_) | Life::Reloading(_))
    }

    /// The word Settings shows.
    pub fn label(&self) -> &str {
        match self {
            Life::Discovered => "Found",
            Life::Refused(_) => "Not loaded",
            Life::AwaitingTrust => "Waiting for your OK",
            Life::Disabled => "Off",
            Life::Loading => "Loading",
            Life::Running(_) => "On",
            Life::Reloading(_) => "Reloading",
            Life::Errored { .. } => "Failed",
        }
    }

    /// Why it isn't running, when something stopped it.
    pub fn why(&self) -> Option<&str> {
        match self {
            Life::Refused(why) | Life::Errored { error: why, .. } => Some(why),
            _ => None,
        }
    }
}

// ---- Trust -----------------------------------------------------------------------------------

/// What the user said about a plugin's files, as their SHA-256 (`hash`) names them. Built-ins
/// and `--plugin-dir` plugins never ask; a third-party plugin asks once, and again when its files
/// change.
///
/// ```text
/// Unknown ─ Accept ▶ Granted        Unknown ─ Later ▶ Deferred ─ Ask ▶ Unknown
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    NotNeeded,
    /// The prompt shows for it.
    Unknown { hash: String },
    /// "Not now": no prompt again this launch unless the user turns it on.
    Deferred { hash: String },
    Granted { hash: String },
}

pub enum TrustEvent {
    Accept,
    Later,
    /// The user turned it on again: ask once more.
    Ask,
}

pub enum TrustEffect {
    /// Save `hash` as trusted for the plugin.
    Remember(String),
    /// Load the plugin ([`LifeEvent::Load`]).
    Load,
}

impl Trust {
    /// Its trust at launch: `saved` is the hash the user OKed before, if any.
    pub fn at_launch(needs_trust: bool, hash: &str, saved: Option<&String>) -> Trust {
        match (needs_trust, saved) {
            (false, _) => Trust::NotNeeded,
            (true, Some(s)) if s == hash => Trust::Granted { hash: hash.to_owned() },
            (true, _) => Trust::Unknown { hash: hash.to_owned() },
        }
    }

    pub fn step(self, event: TrustEvent) -> (Trust, Vec<TrustEffect>) {
        match (self, event) {
            (Trust::Unknown { hash } | Trust::Deferred { hash }, TrustEvent::Accept) => (Trust::Granted { hash: hash.clone() }, vec![TrustEffect::Remember(hash), TrustEffect::Load]),
            (Trust::Unknown { hash }, TrustEvent::Later) => (Trust::Deferred { hash }, vec![]),
            (Trust::Deferred { hash }, TrustEvent::Ask) => (Trust::Unknown { hash }, vec![]),
            (state, _) => (state, vec![]),
        }
    }

    /// Whether the plugin may run.
    pub fn allows(&self) -> bool {
        matches!(self, Trust::NotNeeded | Trust::Granted { .. })
    }
}

// ---- Hot reload ------------------------------------------------------------------------------

/// A plugin folder's save-to-reload: the watcher's thread reads and compiles; meanwhile the UI
/// thread builds the next VM's base, then swaps.
///
/// ```text
/// Idle ─ Changed ▶ Compiling + Prewarm ─ Compiled(Ok) ▶ Idle + Swap
///                            └ Compiled(Err) ▶ Failed + Report ─ Changed ▶ Compiling
/// ```
pub enum Hot {
    Idle,
    Compiling,
    /// The last save didn't compile; the running VM stays until a save that does.
    Failed(String),
}

pub enum HotEvent {
    Changed,
    Compiled(Result<Prepared, String>),
}

pub enum HotEffect {
    /// Build the next VM's base while the compile runs (`Script::prewarm`).
    Prewarm,
    Swap(Prepared),
    Report(String),
}

impl Hot {
    pub fn step(self, event: HotEvent) -> (Hot, Vec<HotEffect>) {
        match (self, event) {
            (_, HotEvent::Changed) => (Hot::Compiling, vec![HotEffect::Prewarm]),
            (_, HotEvent::Compiled(Ok(p))) => (Hot::Idle, vec![HotEffect::Swap(p)]),
            (_, HotEvent::Compiled(Err(e))) => (Hot::Failed(e.clone()), vec![HotEffect::Report(e)]),
        }
    }
}

// ---- Tickets: cached server reads --------------------------------------------------------------

/// A read's answer: its text and, once a render asked for it, the JSON value decoded in the VM.
#[derive(Clone)]
pub struct Body {
    pub text: Rc<str>,
    pub generation: i64,
}

/// One cached server read (`Server:Resource(path, version)`): asked for at a version, answered
/// later. A read asked for at a new version keeps showing the last answer while it loads.
///
/// ```text
/// Empty ─ Ask(v) ▶ Loading(v) ─ Land(v, Ok) ▶ Ready(v) ─ Ask(w≠v) ▶ Loading(w, last)
///                             └ Land(v, Err) ▶ Failed(v)
/// ```
#[derive(Default)]
pub enum Ticket {
    #[default]
    Empty,
    Loading { version: u64, last: Option<Body> },
    Ready { version: u64, body: Body },
    Failed { version: u64, error: String, last: Option<Body> },
}

pub enum TicketEvent {
    Ask(u64),
    Land(u64, Result<String, String>),
}

pub enum TicketEffect {
    Fetch(u64),
}

impl Ticket {
    pub fn step(self, event: TicketEvent) -> (Ticket, Vec<TicketEffect>) {
        use Ticket as T;
        match (self, event) {
            (T::Loading { version, last }, TicketEvent::Ask(v)) if v == version => (T::Loading { version, last }, vec![]),
            (T::Ready { version, body }, TicketEvent::Ask(v)) if v == version => (T::Ready { version, body }, vec![]),
            (T::Failed { version, error, last }, TicketEvent::Ask(v)) if v == version => (T::Failed { version, error, last }, vec![]),
            (state, TicketEvent::Ask(v)) => (T::Loading { version: v, last: state.body().cloned() }, vec![TicketEffect::Fetch(v)]),
            (T::Loading { version, last }, TicketEvent::Land(v, answer)) if v == version => {
                let generation = last.as_ref().map_or(0, |b| b.generation) + 1;
                match answer {
                    Ok(text) => (T::Ready { version, body: Body { text: text.into(), generation } }, vec![]),
                    Err(error) => (T::Failed { version, error, last: last.map(|b| Body { generation, ..b }).or(Some(Body { text: "".into(), generation })) }, vec![]),
                }
            }
            // An answer for a version nobody waits for any more.
            (state, TicketEvent::Land(..)) => (state, vec![]),
        }
    }

    /// The last answer's body, if any.
    pub fn body(&self) -> Option<&Body> {
        match self {
            Ticket::Empty => None,
            Ticket::Loading { last, .. } | Ticket::Failed { last, .. } => last.as_ref(),
            Ticket::Ready { body, .. } => Some(body),
        }
    }

    /// Changes when what a render reads off it does.
    pub fn generation(&self) -> (i64, bool) {
        (self.body().map_or(0, |b| b.generation), matches!(self, Ticket::Loading { .. }))
    }
}

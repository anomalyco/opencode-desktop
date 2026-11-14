//! The plugin host's state machines, each an `enum` and a pure `step(state, event) -> (state,
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
///             ├─ Doze ─────▶ Dormant ────── Load ──┤
///             └─ Load ─────────────────────────────┴▶ Loading ─ Started ──▶ Running ⇄ Reloading
///                                                       └─ StartFailed ▶ Errored ─ Load ▶ Loading
/// Running / Reloading / Errored / Dormant ─ Disable ▶ Disabled
/// Running / Reloading / Errored ─ NeedTrust ▶ AwaitingTrust (its files changed)
/// ```
///
/// `Loading` also covers the wait for the plugin's stored values: its scripts start once they're
/// in memory. `Dormant` is a plugin that may run but hasn't started: what it contributes is listed
/// from its last start's record (`dormant`), and its first use starts it (`Load`).
pub enum Life {
    Discovered,
    /// Its manifest, platform or id keeps it from loading.
    Refused(String),
    /// A third-party plugin whose files the user hasn't OKed ([`Trust`]).
    AwaitingTrust,
    Disabled,
    /// It may run and isn't started yet: its contributions come from its record until its first use.
    Dormant,
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
    /// It may run, and waits for its first use (`Dormant`).
    Doze,
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
            // Its files changed under it: it stops until the user OKs the new ones.
            (L::Running(s) | L::Reloading(s) | L::Errored { script: s, .. }, E::NeedTrust) => (L::AwaitingTrust, vec![F::Stop(s)]),
            (L::Discovered | L::AwaitingTrust | L::Dormant, E::Disable) => (L::Disabled, vec![]),
            (L::Discovered, E::Doze) => (L::Dormant, vec![]),
            (L::Running(s) | L::Reloading(s) | L::Errored { script: s, .. }, E::Disable) => (L::Disabled, vec![F::Stop(s)]),
            (L::Discovered | L::AwaitingTrust | L::Disabled | L::Dormant, E::Load) => (L::Loading, vec![F::Start]),
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

    /// Whether its contributions are listed: running, or dormant (from its record).
    pub fn listed(&self) -> bool {
        self.running() || matches!(self, Life::Dormant)
    }

    /// The word Settings shows.
    pub fn label(&self) -> &str {
        match self {
            Life::Discovered => "Found",
            Life::Refused(_) => "Not loaded",
            Life::AwaitingTrust => "Waiting for your OK",
            Life::Disabled => "Off",
            // On as far as the user can tell: its contributions are there, and it starts at first use.
            Life::Dormant => "On",
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

/// What the user said about a plugin's files, as their SHA-256 (`hash`) names them, and whether
/// that answer is durable. Built-ins and `--plugin-dir` plugins never ask; a third-party plugin
/// asks once, and again when its files change. The OK is a Tier 0 record (`plugin_trust`): the
/// plugin runs only once it's on disk, so a crash can't lose an answer the plugin ran under, and
/// it's for those bytes only (a reload that reads other bytes asks again).
///
/// ```text
/// Unknown ─ Accept ▶ Granting ─ Saved(Ok) ▶ Granted
///                             └ Saved(Err) ▶ GrantedThisRun     Unknown ─ Later ▶ Deferred ─ Ask ▶ Unknown
/// Granting / Granted / GrantedThisRun ─ Revoke ▶ Unknown (forgotten: turned off)
/// any but NotNeeded ─ FilesChanged(h) ▶ Unknown(h)
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    NotNeeded,
    /// The prompt shows for it.
    Unknown { hash: String },
    /// "Not now": no prompt again this launch unless the user turns it on.
    Deferred { hash: String },
    /// The user said yes; the record is being made durable and the plugin waits for it.
    Granting { hash: String },
    /// The user said yes, and the record is on disk.
    Granted { hash: String },
    /// The user said yes but the record couldn't be saved: it runs this launch and asks again next.
    GrantedThisRun { hash: String },
}

pub enum TrustEvent {
    Accept,
    Later,
    /// The user turned it on again: ask once more.
    Ask,
    /// The grant's record landed (or couldn't).
    Saved(Result<(), String>),
    /// The user turned it off: the OK is forgotten with it.
    Revoke,
    /// Its files were read again and hash to this: an OK for other bytes doesn't hold.
    FilesChanged(String),
}

pub enum TrustEffect {
    /// Write `hash` as trusted for the plugin (and it on) in one durable record; answer `Saved`.
    Save(String),
    /// Write the plugin's record without its trust.
    Forget,
    /// Load the plugin ([`LifeEvent::Load`]).
    Load,
    /// Write a line to the console.
    Report(String),
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
        use Trust as T;
        use TrustEffect as F;
        use TrustEvent as E;
        match (self, event) {
            (T::Unknown { hash } | T::Deferred { hash }, E::Accept) => (T::Granting { hash: hash.clone() }, vec![F::Save(hash)]),
            (T::Granting { hash }, E::Saved(Ok(()))) => (T::Granted { hash }, vec![F::Load]),
            (T::Granting { hash }, E::Saved(Err(e))) => (T::GrantedThisRun { hash }, vec![F::Load, F::Report(format!("Couldn't save that you enabled this plugin ({e}); it runs now and asks again next launch"))]),
            (T::Unknown { hash }, E::Later) => (T::Deferred { hash }, vec![]),
            (T::Deferred { hash }, E::Ask) => (T::Unknown { hash }, vec![]),
            (T::Granting { hash } | T::Granted { hash } | T::GrantedThisRun { hash }, E::Revoke) => (T::Unknown { hash }, vec![F::Forget]),
            (T::NotNeeded, E::FilesChanged(_)) => (T::NotNeeded, vec![]),
            (T::Granted { hash: old }, E::FilesChanged(hash)) if old == hash => (T::Granted { hash }, vec![]),
            (T::GrantedThisRun { hash: old }, E::FilesChanged(hash)) if old == hash => (T::GrantedThisRun { hash }, vec![]),
            (_, E::FilesChanged(hash)) => (T::Unknown { hash }, vec![]),
            (state, _) => (state, vec![]),
        }
    }

    /// Whether the plugin may run.
    pub fn allows(&self) -> bool {
        matches!(self, Trust::NotNeeded | Trust::Granted { .. } | Trust::GrantedThisRun { .. })
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

/// A read's answer: its text and, once a render asked for it, the JSON value decoded in the VM;
/// or, for a read that asked for them, the items its answer lists, split off the UI thread.
#[derive(Clone)]
pub struct Body {
    pub text: Rc<str>,
    pub items: Option<Rc<Items>>,
    pub generation: i64,
}

/// An answer split into the items it lists and the rest of it (its other members).
#[derive(Debug)]
pub struct Items {
    pub list: Vec<Item>,
    pub rest: serde_json::Value,
}

/// One item of an answer that lists them (`{ "data": [...] }` or `[...]`): its `id`, `type` and
/// `time.created`, and its JSON.
#[derive(Clone, Debug)]
pub struct Item {
    pub id: String,
    pub kind: String,
    pub created: Option<f64>,
    pub text: String,
}

/// What a read fetched, on the job thread.
pub struct Fetched {
    pub text: String,
    pub items: Option<Items>,
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
    Land(u64, Result<Fetched, String>),
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
                    Ok(f) => (T::Ready { version, body: Body { text: f.text.into(), items: f.items.map(Rc::new), generation } }, vec![]),
                    Err(error) => (T::Failed { version, error, last: last.map(|b| Body { generation, ..b }).or(Some(Body { text: "".into(), items: None, generation })) }, vec![]),
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

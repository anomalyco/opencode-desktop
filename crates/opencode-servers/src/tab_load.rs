//! Session tabs' metadata loads across every server a window shows
//! (`shell/titlebar/tab-load-machine.ts`, ported transition for transition). Only a tab that
//! needs its session has an entry: the active tab, and any tab without a saved title. A
//! background tab with a saved title shows it with no request at all, so a window full of tabs
//! from a server that is away paints from what it saved.

use opencode_kit::servers::key::{ServerKey, TabKey};
use rune::machine::{Attempt, Effects};

/// `SESSION_TAB_LOAD_TIMEOUT_MS` (`tab-load-machine.ts:5`).
pub const SESSION_TAB_LOAD_TIMEOUT_MS: u64 = 15_000;

/// One session tab, read in one pass with the strip (`SessionTabSnapshot`, `:7-19`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub key: TabKey,
    pub server: ServerKey,
    pub session_id: String,
    pub active: bool,
    /// Its session is still being created by a draft's promotion.
    pub pending: bool,
    pub cached: bool,
    pub missing: bool,
    pub persisted_title: bool,
    pub server_ready: bool,
    pub connected: bool,
}

/// A tab's fetch (`SessionTabLoad`, `:21-28`). `Unavailable`: it failed or timed out without the
/// server saying the session is gone; it retries once the connection comes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Load {
    Loading { attempt: Attempt },
    Unavailable { connected: bool },
}

/// `SessionTabLoadsState` (`:30-36`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Active { issued: Attempt, entries: Vec<(TabKey, Load)> },
    Disposed,
}

impl Default for State {
    fn default() -> State {
        State::Active { issued: Attempt(0), entries: Vec::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Sync(Vec<Snapshot>),
    LoadSettled { key: TabKey, attempt: Attempt, cached: bool, missing: bool, connected: bool },
    LoadTimedOut { key: TabKey, attempt: Attempt, connected: bool },
    Disposed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    LoadStart { key: TabKey, server: ServerKey, session_id: String, attempt: Attempt, timeout_ms: u64 },
    LoadClear { attempt: Attempt },
}

/// How a session tab shows (`SessionTabView.phase`, `:76-83`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    Preparing,
    Loaded,
    Missing,
    Deferred,
    WaitingServer,
    Unavailable,
    Loading,
}

/// The title a tab falls back to: the generic session label, its saved title, the label of a
/// session that couldn't load, or none (`fallbackTitle`, `:80-81`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FallbackTitle {
    Session,
    Persisted,
    Unknown,
    None,
}

/// What [`view`] reads beside the machine (`SessionTabViewInput`, `:67-74`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewInput<'a> {
    pub active: bool,
    /// The pending draft's directory, while the session is being created.
    pub pending: Option<Option<&'a str>>,
    pub loaded: bool,
    pub missing: bool,
    pub persisted_title: Option<&'a str>,
    pub persisted_directory: Option<&'a str>,
    pub server_ready: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View<'a> {
    pub phase: Phase,
    pub visible: bool,
    pub fallback_title: FallbackTitle,
    pub fallback_directory: Option<&'a str>,
}

/// `transitionSessionTabLoads` (`tab-load-machine.ts:95-120`).
pub fn step(state: &mut State, event: Event) -> Effects<Effect> {
    let State::Active { issued, entries } = state else { return Effects::none() };
    match event {
        Event::Disposed => {
            let fx = clear_all(entries, |_| true);
            *state = State::Disposed;
            fx
        }
        Event::Sync(tabs) => sync(issued, entries, tabs),
        Event::LoadSettled { key, attempt, .. } | Event::LoadTimedOut { key, attempt, .. } if !loading(entries, &key, attempt) => Effects::none(),
        // A loaded or missing session leaves the entry: the tab reads both from the cache (`:109-111`).
        Event::LoadSettled { key, attempt, cached, missing, .. } if cached || missing => {
            entries.retain(|(k, _)| *k != key);
            Effects::one(Effect::LoadClear { attempt })
        }
        Event::LoadSettled { key, attempt, connected, .. } | Event::LoadTimedOut { key, attempt, connected } => {
            if let Some((_, load)) = entries.iter_mut().find(|(k, _)| *k == key) {
                *load = Load::Unavailable { connected };
            }
            Effects::one(Effect::LoadClear { attempt })
        }
    }
}

fn loading(entries: &[(TabKey, Load)], key: &TabKey, attempt: Attempt) -> bool {
    entries.iter().any(|(k, l)| k == key && *l == Load::Loading { attempt })
}

/// `selectSessionTabView` (`:122-160`).
pub fn view<'a>(state: &State, key: &TabKey, input: ViewInput<'a>) -> View<'a> {
    let persisted = input.persisted_title.filter(|t| !t.is_empty()).map(|_| FallbackTitle::Persisted);
    if let Some(draft_dir) = input.pending {
        return View { phase: Phase::Preparing, visible: true, fallback_title: FallbackTitle::Session, fallback_directory: draft_dir.or(input.persisted_directory) };
    }
    let view = |phase, visible, fallback_title| View { phase, visible, fallback_title, fallback_directory: input.persisted_directory };
    if input.loaded {
        return view(Phase::Loaded, true, persisted.unwrap_or(FallbackTitle::None));
    }
    if input.missing {
        return view(Phase::Missing, true, persisted.unwrap_or(FallbackTitle::Unknown));
    }
    // A background tab with a saved title shows it without fetching its session.
    if let (false, Some(p)) = (input.active, persisted) {
        return view(Phase::Deferred, true, p);
    }
    if !input.server_ready {
        return view(Phase::WaitingServer, persisted.is_some(), persisted.unwrap_or(FallbackTitle::None));
    }
    let unavailable = matches!(state, State::Active { entries, .. } if entries.iter().any(|(k, l)| k == key && matches!(l, Load::Unavailable { .. })));
    if unavailable {
        return view(Phase::Unavailable, true, persisted.unwrap_or(FallbackTitle::Unknown));
    }
    view(Phase::Loading, persisted.is_some(), persisted.unwrap_or(FallbackTitle::None))
}

/// `sync` (`:162-217`).
fn sync(issued: &mut Attempt, entries: &mut Vec<(TabKey, Load)>, tabs: Vec<Snapshot>) -> Effects<Effect> {
    let mut starts = Vec::new();
    let mut next: Vec<(TabKey, Load)> = Vec::new();
    let mut counter = *issued;
    for tab in tabs {
        let needed = !tab.pending && !tab.cached && !tab.missing && (tab.active || !tab.persisted_title) && tab.server_ready;
        if !needed || next.iter().any(|(k, _)| *k == tab.key) {
            continue;
        }
        let entry = entries.iter().find(|(k, _)| *k == tab.key).map(|(_, l)| *l);
        match entry {
            Some(load @ Load::Loading { .. }) => next.push((tab.key, load)),
            // A failure while connected waits for a reconnect, or for the tab to be activated again.
            Some(Load::Unavailable { connected }) if connected || !tab.connected => next.push((tab.key, Load::Unavailable { connected: tab.connected })),
            _ => {
                counter = Attempt(counter.0 + 1);
                starts.push(Effect::LoadStart { key: tab.key.clone(), server: tab.server, session_id: tab.session_id, attempt: counter, timeout_ms: SESSION_TAB_LOAD_TIMEOUT_MS });
                next.push((tab.key, Load::Loading { attempt: counter }));
            }
        }
    }
    let mut fx = clear_all(entries, |k| !next.iter().any(|(n, _)| n == k));
    if starts.is_empty() && next.len() == entries.len() && next.iter().all(|e| entries.contains(e)) {
        return Effects::none();
    }
    fx.extend(starts);
    *issued = counter;
    *entries = next;
    fx
}

/// `clearAll` (`:219-223`): a loading entry that leaves clears its timeout.
fn clear_all(entries: &[(TabKey, Load)], gone: impl Fn(&TabKey) -> bool) -> Effects<Effect> {
    let mut fx = Effects::none();
    for (key, load) in entries {
        if let (Load::Loading { attempt }, true) = (load, gone(key)) {
            fx.push(Effect::LoadClear { attempt: *attempt });
        }
    }
    fx
}

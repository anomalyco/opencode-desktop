//! Luau server sources (`ctx:AddServerSource`, Desktop's `Server` registry), shared by every
//! window: what each source lists (`ServerSource:SetEntries`), as the records the app's server
//! lists already read (`plugin_host::servers`), and the window halves that draw their rows.
//!
//! Each source is one explicit machine, [`Source`]: `Pending` until its first complete list,
//! then `Listed`; a list that isn't complete yet keeps what the host knows about servers it
//! leaves out (their health), and withdrawing a source forgets its servers.

use std::cell::RefCell;

use opencode_plugin_spec::{ServerEntry, ServerState};

use crate::plugin_host::instance::ServerRec;

/// One source's state.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Source {
    /// Added; its list (maybe partial) isn't complete yet.
    Pending { entries: Vec<ServerRec> },
    /// Its list is complete: servers it leaves out are gone.
    Listed { entries: Vec<ServerRec> },
}

enum Event {
    Set { entries: Vec<ServerRec>, ready: bool },
}

impl Source {
    fn step(self, event: Event) -> Source {
        match event {
            Event::Set { entries, ready: true } => Source::Listed { entries },
            Event::Set { entries, ready: false } => match self {
                // A listed source that lists again partially keeps its servers listed.
                Source::Listed { .. } => Source::Listed { entries },
                Source::Pending { .. } => Source::Pending { entries },
            },
        }
    }

    fn entries(&self) -> &[ServerRec] {
        match self {
            Source::Pending { entries } | Source::Listed { entries } => entries,
        }
    }

    fn complete(&self) -> bool {
        matches!(self, Source::Listed { .. })
    }
}

struct Sourced {
    plugin: String,
    id: String,
    order: i64,
    state: Source,
}

thread_local! {
    static SOURCES: RefCell<Vec<Sourced>> = const { RefCell::new(Vec::new()) };
}

/// `AddServerSource`: source `source` of plugin `plugin`, in `order`, nothing listed yet.
pub fn add(plugin: &str, source: &str, order: i64) {
    SOURCES.with_borrow_mut(|all| match all.iter_mut().find(|s| s.plugin == plugin && s.id == source) {
        Some(s) => s.order = order,
        None => all.push(Sourced { plugin: plugin.to_owned(), id: source.to_owned(), order, state: Source::Pending { entries: Vec::new() } }),
    });
    super::bump();
}

/// `ServerSource:SetEntries`.
pub fn set_entries(plugin: &str, source: &str, entries: Vec<ServerEntry>, ready: bool) {
    let recs: Vec<ServerRec> = entries
        .into_iter()
        .map(|e| ServerRec {
            source: source.to_owned(),
            id: e.id,
            name: e.name,
            label: e.label.unwrap_or_default(),
            state: match e.state {
                ServerState::Stopped => 0,
                ServerState::Starting => 1,
                ServerState::Auth => 2,
                ServerState::Ready => 3,
                ServerState::Failed => 4,
                ServerState::Incompatible => 5,
            },
            url: e.http.as_ref().map(|h| h.url.clone()).unwrap_or_default(),
            password: e.http.and_then(|h| h.password).unwrap_or_default(),
            listed: e.listed.unwrap_or(true),
        })
        .collect();
    SOURCES.with_borrow_mut(|all| {
        let at = match all.iter().position(|s| s.plugin == plugin && s.id == source) {
            Some(at) => at,
            None => {
                all.push(Sourced { plugin: plugin.to_owned(), id: source.to_owned(), order: 0, state: Source::Pending { entries: Vec::new() } });
                all.len() - 1
            }
        };
        let s = &mut all[at];
        s.state = std::mem::replace(&mut s.state, Source::Pending { entries: Vec::new() }).step(Event::Set { entries: recs, ready });
    });
    super::bump();
}

/// Every listed server as `(plugin, source order, record)`.
pub fn all() -> Vec<(String, i64, ServerRec)> {
    SOURCES.with_borrow(|all| all.iter().flat_map(|s| s.state.entries().iter().map(|r| (s.plugin.clone(), s.order, r.clone()))).collect())
}

/// Keys (`<plugin>:<id>`) of servers whose source may still list them: those of sources whose
/// list isn't complete keep what the host knows about them.
pub fn pending_sources() -> Vec<(String, String)> {
    SOURCES.with_borrow(|all| all.iter().filter(|s| !s.state.complete()).map(|s| (s.plugin.clone(), s.id.clone())).collect())
}

/// A hash of every source's list, for `Servers.Changed`.
pub fn version() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    SOURCES.with_borrow(|all| all.iter().for_each(|s| (&s.plugin, &s.id, &s.state).hash(&mut h)));
    h.finish()
}

/// `ServerSource:Withdraw`: forgets source `source` of plugin `plugin` and what it listed.
pub fn withdraw(plugin: &str, source: &str) {
    SOURCES.with_borrow_mut(|all| all.retain(|s| !(s.plugin == plugin && s.id == source)));
    super::bump();
}

/// Forgets what plugin `plugin`'s sources listed.
pub fn forget(plugin: &str) {
    SOURCES.with_borrow_mut(|l| l.retain(|x| x.plugin != plugin));
    super::bump();
}

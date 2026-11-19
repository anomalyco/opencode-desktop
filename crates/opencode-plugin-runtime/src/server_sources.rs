//! Pure Luau server source state machine, record types, and inventory JSON serialization.
//!
//! Each source is one explicit machine ([`Source`]): `Pending` until its first complete list,
//! then `Listed`; a list that isn't complete yet keeps what the host knows about servers it
//! leaves out, and withdrawing a source forgets its servers.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

use opencode_plugin_spec::{ServerEntry, ServerState};
use serde_json::{Value, json};

/// One server a server source listed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ServerRec {
    pub source: String,
    pub id: String,
    pub name: String,
    /// Its kind's badge (`WSL`, `SSH`).
    pub label: String,
    pub state: ServerState,
    pub url: String,
    pub password: String,
    /// It joins the app's servers rather than only showing in Settings.
    pub listed: bool,
    /// Its plugin keeps it reachable (an SSH tunnel): a check of its new address shows the
    /// connecting spinner (`ServerEntry.managed`).
    pub managed: bool,
}

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

/// A plugin's last complete list, as saved (`Inventories`): `{ order, servers: [{ id, name,
/// label?, listed? }] }`, each server's source too, which Desktop's one source per plugin
/// doesn't need.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inventory {
    pub order: i64,
    pub servers: Vec<ServerRec>,
}

/// Process-wide registry of plugin server sources and remembered inventories.
#[derive(Default)]
pub struct Registry {
    sources: Vec<Sourced>,
    known: Option<BTreeMap<String, Inventory>>,
}

impl Registry {
    /// `AddServerSource`: source `source` of plugin `plugin`, in `order`, nothing listed yet.
    pub fn add(&mut self, plugin: &str, source: &str, order: i64) {
        match self.sources.iter_mut().find(|s| s.plugin == plugin && s.id == source) {
            Some(s) => s.order = order,
            None => self.sources.push(Sourced { plugin: plugin.to_owned(), id: source.to_owned(), order, state: Source::Pending { entries: Vec::new() } }),
        }
    }

    /// `ServerSource:SetEntries`. Returns `Some(json)` when the remembered inventory changed and
    /// should be persisted by the host.
    pub fn set_entries(&mut self, plugin: &str, source: &str, entries: Vec<ServerEntry>, ready: bool, load_known: impl FnOnce() -> Option<Value>) -> Option<Value> {
        let recs: Vec<ServerRec> = entries
            .into_iter()
            .map(|e| ServerRec {
                source: source.to_owned(),
                id: e.id,
                name: e.name,
                label: e.label.unwrap_or_default(),
                state: e.state,
                url: e.http.as_ref().map(|h| h.url.clone()).unwrap_or_default(),
                password: e.http.and_then(|h| h.password).unwrap_or_default(),
                listed: e.listed.unwrap_or(true),
                managed: e.managed.unwrap_or(false),
            })
            .collect();
        let at = match self.sources.iter().position(|s| s.plugin == plugin && s.id == source) {
            Some(at) => at,
            None => {
                self.sources.push(Sourced { plugin: plugin.to_owned(), id: source.to_owned(), order: 0, state: Source::Pending { entries: Vec::new() } });
                self.sources.len() - 1
            }
        };
        let s = &mut self.sources[at];
        s.state = std::mem::replace(&mut s.state, Source::Pending { entries: Vec::new() }).step(Event::Set { entries: recs, ready });
        self.remember(plugin, load_known)
    }

    /// Every listed server as `(plugin, source order, record)`: a plugin with a complete list lists
    /// its sources' servers; one without lists what it has so far, else its last launch's, stopped.
    pub fn all(&mut self, load_known: impl FnOnce() -> Option<Value>) -> Vec<(String, i64, ServerRec)> {
        let live: Vec<(String, i64, ServerRec)> = self.sources.iter().flat_map(|s| s.state.entries().iter().map(|r| (s.plugin.clone(), s.order, r.clone()))).collect();
        let listing: Vec<String> = self.sources.iter().filter(|s| s.state.complete() || !s.state.entries().is_empty()).map(|s| s.plugin.clone()).collect();
        let known = self.ensure_known(load_known);
        let remembered: Vec<(String, i64, ServerRec)> = known
            .iter()
            .filter(|(plugin, _)| !listing.contains(plugin))
            .flat_map(|(plugin, inv)| inv.servers.iter().map(|r| (plugin.clone(), inv.order, ServerRec { state: ServerState::Stopped, url: String::new(), password: String::new(), ..r.clone() })))
            .collect();
        live.into_iter().chain(remembered).collect()
    }

    /// `(plugin, source)` pairs whose list isn't complete yet.
    pub fn pending_sources(&self) -> Vec<(String, String)> {
        self.sources.iter().filter(|s| !s.state.complete()).map(|s| (s.plugin.clone(), s.id.clone())).collect()
    }

    /// A hash of every source's list and display order, for `Servers.Changed` and Home sync.
    pub fn version(&self) -> u64 {
        let mut h = std::hash::DefaultHasher::new();
        self.sources.iter().for_each(|s| (&s.plugin, &s.id, s.order, &s.state).hash(&mut h));
        h.finish()
    }

    /// `ServerSource:Withdraw`: forgets source `source` of plugin `plugin` and what it listed.
    pub fn withdraw(&mut self, plugin: &str, source: &str) {
        self.sources.retain(|s| !(s.plugin == plugin && s.id == source));
    }

    /// Forgets what plugin `plugin`'s sources listed; its last complete list stays, stopped.
    pub fn forget(&mut self, plugin: &str) {
        self.sources.retain(|x| x.plugin != plugin);
    }

    /// Whether plugin `plugin`'s servers were listed at the last run.
    pub fn remembers(&mut self, plugin: &str, load_known: impl FnOnce() -> Option<Value>) -> bool {
        self.ensure_known(load_known).get(plugin).is_some_and(|inv| !inv.servers.is_empty())
    }

    fn ensure_known(&mut self, load_known: impl FnOnce() -> Option<Value>) -> &mut BTreeMap<String, Inventory> {
        self.known.get_or_insert_with(|| load_known().map_or_else(BTreeMap::new, parse_inventory))
    }

    fn remember(&mut self, plugin: &str, load_known: impl FnOnce() -> Option<Value>) -> Option<Value> {
        let lists: Vec<(i64, Vec<ServerRec>)> = self.sources.iter().filter(|s| s.plugin == plugin && s.state.complete()).map(|s| (s.order, s.state.entries().to_vec())).collect();
        if lists.is_empty() {
            return None;
        }
        let inventory = Inventory {
            order: lists.iter().map(|(o, _)| *o).min().unwrap_or(0),
            servers: lists.into_iter().flat_map(|(_, l)| l).map(|r| ServerRec { state: ServerState::Stopped, url: String::new(), password: String::new(), ..r }).collect(),
        };
        let known = self.ensure_known(load_known);
        if known.get(plugin) == Some(&inventory) {
            return None;
        }
        known.insert(plugin.to_owned(), inventory);
        Some(serialize_inventory(known))
    }
}

/// Parses Desktop's global `plugin.servers` JSON (`plugin/servers.tsx`).
pub fn parse_inventory(v: Value) -> BTreeMap<String, Inventory> {
    let text = |v: &Value| v.as_str().map(str::to_owned);
    v.as_object()
        .into_iter()
        .flatten()
        .map(|(plugin, inv)| {
            let servers = inv["servers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| {
                    Some(ServerRec {
                        source: text(&s["source"]).unwrap_or_else(|| plugin.clone()),
                        id: text(&s["id"])?,
                        name: text(&s["name"])?,
                        label: text(&s["label"]).unwrap_or_default(),
                        state: ServerState::Stopped,
                        url: String::new(),
                        password: String::new(),
                        listed: s["listed"].as_bool().unwrap_or(true),
                        managed: false,
                    })
                })
                .collect();
            (plugin.clone(), Inventory { order: inv["order"].as_i64().unwrap_or(0), servers })
        })
        .collect()
}

/// Serializes remembered inventories into Desktop's `plugin.servers` JSON format.
pub fn serialize_inventory(known: &BTreeMap<String, Inventory>) -> Value {
    let value: serde_json::Map<String, Value> = known
        .iter()
        .map(|(plugin, inv)| {
            let servers: Vec<Value> = inv
                .servers
                .iter()
                .map(|r| {
                    let mut s = json!({ "id": r.id, "name": r.name, "source": r.source });
                    if !r.label.is_empty() {
                        s["label"] = json!(r.label);
                    }
                    if !r.listed {
                        s["listed"] = json!(false);
                    }
                    s
                })
                .collect();
            (plugin.clone(), json!({ "order": inv.order, "servers": servers }))
        })
        .collect();
    Value::Object(value)
}

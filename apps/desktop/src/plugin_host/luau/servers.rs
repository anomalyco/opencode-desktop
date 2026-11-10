//! Luau server sources (`ctx:AddServerSource`, Desktop's `Server` registry), shared by every
//! window: what each source lists (`ServerSource:SetEntries`), as the records the app's server
//! lists already read (`plugin_host::servers`), and the window halves that draw their rows.
//!
//! Each source is one explicit machine, [`Source`]: `Pending` until its first complete list,
//! then `Listed`; a list that isn't complete yet keeps what the host knows about servers it
//! leaves out (their health), and withdrawing a source forgets its servers.
//!
//! Each plugin's last complete list is kept across launches (`opencode.global.dat`'s
//! `plugin.servers`, as Desktop's `PluginServersProvider` keeps it, without addresses or
//! passwords): until the plugin lists its servers again (it's starting, reloading or off), they
//! stay listed as stopped, so their tabs, drafts and Settings rows outlive the outage.

use std::cell::RefCell;
use std::collections::BTreeMap;

use mlua::Function;
use opencode_plugin_spec::{ServerEntry, ServerState};
use rune::Tx;
use serde_json::{Value, json};

use crate::App;
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

/// A plugin's last complete list, as saved (`Inventories`): `{ order, servers: [{ id, name,
/// label?, listed? }] }`, each server's source too, which Desktop's one source per plugin
/// doesn't need.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Inventory {
    order: i64,
    servers: Vec<ServerRec>,
}

thread_local! {
    static SOURCES: RefCell<Vec<Sourced>> = const { RefCell::new(Vec::new()) };
    /// Each plugin's last complete list, read once from the last launch.
    static KNOWN: RefCell<Option<BTreeMap<String, Inventory>>> = const { RefCell::new(None) };
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
            managed: e.managed.unwrap_or(false),
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
    remember(plugin);
    super::bump();
}

/// Every listed server as `(plugin, source order, record)`: a plugin with a complete list lists
/// its sources' servers; one without lists what it has so far, else its last launch's, stopped.
pub fn all() -> Vec<(String, i64, ServerRec)> {
    let live: Vec<(String, i64, ServerRec)> = SOURCES.with_borrow(|all| all.iter().flat_map(|s| s.state.entries().iter().map(|r| (s.plugin.clone(), s.order, r.clone()))).collect());
    let listing: Vec<String> = SOURCES.with_borrow(|all| all.iter().filter(|s| s.state.complete() || !s.state.entries().is_empty()).map(|s| s.plugin.clone()).collect());
    let remembered: Vec<(String, i64, ServerRec)> = known(|k| {
        k.iter()
            .filter(|(plugin, _)| !listing.contains(plugin))
            .flat_map(|(plugin, inv)| inv.servers.iter().map(|r| (plugin.clone(), inv.order, ServerRec { state: opencode_plugin::abi::SERVER_STOPPED, url: String::new(), password: String::new(), ..r.clone() })))
            .collect()
    });
    live.into_iter().chain(remembered).collect()
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

/// Forgets what plugin `plugin`'s sources listed; its last complete list stays, stopped.
pub fn forget(plugin: &str) {
    SOURCES.with_borrow_mut(|l| l.retain(|x| x.plugin != plugin));
    super::bump();
}

/// What a source function's task answers with: true when it returned `true`.
pub type Settle = Box<dyn FnOnce(bool)>;

/// Runs source function `name` (`connect`, `reconnect`, `remove`) of the source that lists
/// server `key` with its entry's id, as a task: a window half's, else the app half's. `settle`
/// hears what it returned once it has (an error is false). False when the source has none.
pub fn call(app: &mut App, key: &str, name: &str, settle: Option<Settle>, tx: &mut Tx) -> bool {
    let Some((plugin, rec)) = all().into_iter().find(|(plugin, _, r)| format!("{plugin}:{}", r.id) == key).map(|(p, _, r)| (p, r)) else { return false };
    let window = app.plugins.luau.running().find_map(|(i, pl)| (pl.manifest.id == plugin).then(|| source_fn(&pl.half, &rec.source, name).map(|f| (i, f))).flatten());
    if let Some((i, f)) = window {
        if let Some(script) = app.plugins.luau.plugins[i].script() {
            super::cx::with_app(app, || run(script, &f, &rec.id, settle, tx));
        }
        super::requests::carry_out(app, tx);
        return true;
    }
    let mut settle = settle;
    let ran = super::app_half::each(|a| {
        if *a.half.id != *plugin {
            return None;
        }
        let f = source_fn(&a.half, &rec.source, name)?;
        super::cx::with_app(app, || run(&a.script, &f, &rec.id, settle.take(), tx));
        Some(())
    });
    super::requests::carry_out(app, tx);
    !ran.is_empty()
}

/// Spawns `f(id)`; with `settle`, inside `pcall`, telling it what came back.
fn run(script: &rune_luau::Script, f: &Function, id: &str, settle: Option<Settle>, tx: &mut Tx) {
    let Some(settle) = settle else {
        script.spawn_fn(f, id.to_owned(), Some(tx)).ok();
        return;
    };
    let settle = RefCell::new(Some(settle));
    let wrapped = script.with_lua(|lua| -> mlua::Result<(Function, Function)> {
        let wrapper: Function = lua.load("local f, id, done = ...\nlocal ok, result = pcall(f, id)\ndone(ok and result == true)").into_function()?;
        let done = lua.create_function(move |_, ready: bool| {
            if let Some(s) = settle.borrow_mut().take() {
                s(ready);
            }
            Ok(())
        })?;
        Ok((wrapper, done))
    });
    if let Some(Ok((wrapper, done))) = wrapped {
        script.spawn_fn(&wrapper, (f.clone(), id.to_owned(), done), Some(tx)).ok();
    }
}

fn source_fn(half: &super::Half, source: &str, name: &str) -> Option<Function> {
    half.contribs.borrow().server_sources.iter().find(|c| c.spec.id == source).and_then(|c| c.f(name).cloned())
}

/// Plugin `plugin`'s complete list is saved, so the next launch lists its servers before the
/// plugin starts.
fn remember(plugin: &str) {
    let lists: Vec<(i64, Vec<ServerRec>)> = SOURCES.with_borrow(|all| all.iter().filter(|s| s.plugin == plugin && s.state.complete()).map(|s| (s.order, s.state.entries().to_vec())).collect());
    if lists.is_empty() {
        return;
    }
    let inventory = Inventory {
        order: lists.iter().map(|(o, _)| *o).min().unwrap_or(0),
        servers: lists.into_iter().flat_map(|(_, l)| l).map(|r| ServerRec { state: opencode_plugin::abi::SERVER_STOPPED, url: String::new(), password: String::new(), ..r }).collect(),
    };
    let changed = known(|k| {
        if k.get(plugin) == Some(&inventory) {
            return false;
        }
        k.insert(plugin.to_owned(), inventory);
        true
    });
    if changed {
        save();
    }
}

/// Runs `f` on the saved lists, read from disk the first time.
fn known<R>(f: impl FnOnce(&mut BTreeMap<String, Inventory>) -> R) -> R {
    KNOWN.with_borrow_mut(|k| f(k.get_or_insert_with(load)))
}

fn file() -> Option<std::path::PathBuf> {
    if cfg!(test) {
        return None;
    }
    crate::dirs::data().map(|d| d.join("opencode.global.dat"))
}

fn load() -> BTreeMap<String, Inventory> {
    let Some(v) = file().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|s| serde_json::from_str::<Value>(&s).ok()) else { return BTreeMap::new() };
    let text = |v: &Value| v.as_str().map(str::to_owned);
    v["plugin.servers"]
        .as_object()
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
                        state: opencode_plugin::abi::SERVER_STOPPED,
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

/// Writes `plugin.servers` into `opencode.global.dat`, keeping its other keys.
fn save() {
    let Some(path) = file() else { return };
    let value: serde_json::Map<String, Value> = known(|k| {
        k.iter()
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
            .collect()
    });
    crate::disk::later("global.dat:plugin.servers", move || {
        let mut doc = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
        doc["plugin.servers"] = Value::Object(value);
        crate::disk::replace(&path, doc.to_string().as_bytes());
    });
}

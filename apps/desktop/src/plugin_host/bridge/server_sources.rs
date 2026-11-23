//! Desktop Luau server sources adapter over `opencode_plugin_runtime::server_sources`: persists
//! inventories via `crate::storage` and dispatches `connect`/`reconnect`/`remove` callbacks.

use std::cell::RefCell;

use mlua::Function;
use opencode_plugin_runtime::server_sources::{Registry, ServerRec};
use opencode_plugin_spec::ServerEntry;
use rune::Tx;

use crate::App;

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::default();
}

fn load_stored() -> Option<serde_json::Value> {
    crate::storage::json("opencode.global.dat", "plugin.servers")
}

/// `AddServerSource`: source `source` of plugin `plugin`, in `order`, nothing listed yet.
pub fn add(plugin: &str, source: &str, order: i64) {
    REGISTRY.with_borrow_mut(|r| r.add(plugin, source, order));
    crate::plugin_host::bump();
}

/// `ServerSource:SetEntries`.
pub fn set_entries(plugin: &str, source: &str, entries: Vec<ServerEntry>, ready: bool) {
    let save = REGISTRY.with_borrow_mut(|r| r.set_entries(plugin, source, entries, ready, load_stored));
    if let Some(value) = save {
        crate::storage::set_json("opencode.global.dat", "plugin.servers", &value);
    }
    crate::plugin_host::bump();
}

/// Every listed server as `(plugin, source order, record)`: a plugin with a complete list lists
/// its sources' servers; one without lists what it has so far, else its last launch's, stopped.
pub fn all() -> Vec<(String, i64, ServerRec)> {
    REGISTRY.with_borrow_mut(|r| r.all(load_stored))
}

/// Keys (`<plugin>:<id>`) of servers whose source may still list them: those of sources whose
/// list isn't complete keep what the host knows about them.
pub fn pending_sources() -> Vec<(String, String)> {
    REGISTRY.with_borrow(Registry::pending_sources)
}

/// A hash of every source's list and display order, for `Servers.Changed` and Home sync.
pub fn version() -> u64 {
    REGISTRY.with_borrow(Registry::version)
}

/// `ServerSource:Withdraw`: forgets source `source` of plugin `plugin` and what it listed.
pub fn withdraw(plugin: &str, source: &str) {
    REGISTRY.with_borrow_mut(|r| r.withdraw(plugin, source));
    crate::plugin_host::bump();
}

/// Forgets what plugin `plugin`'s sources listed; its last complete list stays, stopped.
pub fn forget(plugin: &str) {
    REGISTRY.with_borrow_mut(|r| r.forget(plugin));
    crate::plugin_host::bump();
}

/// Whether plugin `plugin`'s servers were listed at the last run, so their rows show before it starts.
pub fn remembers(plugin: &str) -> bool {
    REGISTRY.with_borrow_mut(|r| r.remembers(plugin, load_stored))
}

/// What a source function's task answers with: true when it returned `true`.
pub type Settle = Box<dyn FnOnce(bool)>;

/// Runs source function `name` (`connect`, `reconnect`, `remove`) of the source that lists
/// server `key` with its entry's id, as a task: a window half's, else the app half's. `settle`
/// hears what it returned once it has (an error is false). False when the source has none.
pub fn call(app: &mut App, key: &str, name: &str, settle: Option<Settle>, tx: &mut Tx) -> bool {
    let Some((plugin, rec)) = all().into_iter().find(|(plugin, _, r)| format!("{plugin}:{}", r.id) == key).map(|(p, _, r)| (p, r)) else { return false };
    let window = app.plugins.running().find_map(|(i, pl)| (pl.manifest.id == plugin).then(|| source_fn(&pl.half, &rec.source, name).map(|f| (i, f))).flatten());
    if let Some((i, f)) = window {
        if let Some(script) = app.plugins.list[i].script() {
            super::cx::enter(app, || run(script, &f, &rec.id, settle, tx));
        }
        super::requests::carry_out(app, tx);
        return true;
    }
    let mut settle = settle;
    let ran = crate::plugin_host::app_half::each(|a| {
        if *a.half.id != *plugin {
            return None;
        }
        let f = source_fn(&a.half, &rec.source, name)?;
        super::cx::enter(app, || run(&a.script, &f, &rec.id, settle.take(), tx));
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
        let wrapper: Function = opencode_plugin_runtime::chunk::load(lua, "local f, id, done = ...\nlocal ok, result = pcall(f, id)\ndone(ok and result == true)").into_function()?;
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

fn source_fn(half: &crate::plugin_host::Half, source: &str, name: &str) -> Option<Function> {
    half.contribs.borrow().server_sources.iter().find(|c| c.spec.id == source).and_then(|c| c.f(name).cloned())
}

//! Each plugin's own values (the `Store` service): rows of `plugin.<id>` in the app's database,
//! as Desktop's main process keeps them (`plugin/storage.ts`), one copy for both halves and every
//! window. Reads come from memory: the storage's cache once the plugin's namespace is loaded,
//! which happens before the plugin starts ([`load`]); the boot record for the plugins the first
//! frame starts, before anything else is read. Writes go behind with the rest of the app's state
//! (250 ms, and on hide, close and quit); an unchanged value writes nothing.
//!
//! Per-session values keep the [`MAX_SESSIONS`] sessions used last, per store (Desktop's
//! `MAX_SESSION_KEYS`): the order is a host row, `rune.recent:<name>`, written only when it
//! changes; a read of a session's value (once per change of it, as the value's memo reads) or a
//! write moves the session first, and the session that falls off the end goes with its value.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use opencode_storage::{BootPacket, Lookup, plugin_namespace};
use rune::Tx;

use crate::App;

/// The sessions a per-session store keeps.
pub const MAX_SESSIONS: usize = 50;

thread_local! {
    static STORES: RefCell<Stores> = RefCell::default();
}

#[derive(Default)]
struct Stores {
    /// The boot packet, while plugins started from it wait for their namespaces.
    boot: Option<Arc<BootPacket>>,
    /// Values read before anything could answer (stage B not in yet): read again once it is.
    provisional: Vec<(String, String)>,
    /// Each per-session store's sessions, most recent first, by namespace and store name.
    recent: HashMap<String, HashMap<String, Vec<String>>>,
    /// Sessions used before their plugin's namespace was in, in order.
    waiting: Vec<(String, String, String)>,
}

/// The key of store `name`'s value for scoped session `session` (none: the global value).
pub fn key(name: &str, session: &str) -> String {
    if session.is_empty() { name.to_owned() } else { format!("{name}@{session}") }
}

/// The first window sets up: the plugins it starts before stage B read from the boot packet.
pub fn begin() {
    if let Some(boot) = crate::storage::boot_packet() {
        STORES.with_borrow_mut(|s| s.boot.get_or_insert(boot).clone());
    }
}

/// Value `key` of plugin `plugin` (namespace `ns`, [`plugin_namespace`]) as JSON text, from memory.
pub fn text(plugin: &str, ns: &str, key: &str) -> Option<Arc<str>> {
    if let Lookup::Ready(v) = crate::storage::plugin_value(ns, key) {
        return v;
    }
    // Started from the boot packet and its namespace still on its way.
    if let Some(Lookup::Ready(v)) = STORES.with_borrow(|s| s.boot.as_ref().map(|b| b.plugin_value(ns, key))) {
        return v;
    }
    // A value the record couldn't hold: stage B has it if it's in (never waited for).
    if crate::storage::need() {
        if !crate::storage::is_loaded(ns) {
            load_now(ns);
        }
        if let Lookup::Ready(v) = crate::storage::plugin_value(ns, key) {
            return v;
        }
    }
    STORES.with_borrow_mut(|s| s.provisional.push((plugin.to_owned(), key.to_owned())));
    None
}

/// Sets value `key` of namespace `ns` (JSON text); false when it already held it (nothing written).
pub fn set(ns: &str, key: &str, json: String) -> bool {
    let same = match crate::storage::plugin_value(ns, key) {
        Lookup::Ready(v) => v.is_some_and(|v| *v == *json),
        Lookup::Pending => STORES.with_borrow(|s| s.boot.as_ref().is_some_and(|b| matches!(b.plugin_value(ns, key), Lookup::Ready(Some(v)) if *v == *json))),
    };
    if same {
        return false;
    }
    crate::storage::set(ns, key, json);
    true
}

/// Session value `key` of store `name` was read (a render's first read of it, which the value's
/// memo keeps to once per change) or written: its session moves first in the store's order.
pub fn touch(plugin: &str, ns: &str, name: &str, key: &str) {
    // The usual case is two lookups: the session in use is first already.
    if STORES.with_borrow(|s| s.recent.get(ns).and_then(|r| r.get(name)).and_then(|l| l.first()).is_some_and(|k| k == key)) {
        return;
    }
    if !crate::storage::is_loaded(ns) {
        STORES.with_borrow_mut(|s| s.waiting.push((plugin.to_owned(), name.to_owned(), key.to_owned())));
        return;
    }
    move_first(plugin, ns, name, key);
}

/// Puts `key` first in store `name`'s sessions; the one past [`MAX_SESSIONS`] goes.
fn move_first(plugin: &str, ns: &str, name: &str, key: &str) {
    let index = format!("{}recent:{name}", opencode_storage::HOST_KEY_PREFIX);
    let (changed, evicted) = STORES.with_borrow_mut(|s| {
        let list = s.recent.entry(ns.to_owned()).or_default().entry(name.to_owned()).or_insert_with(|| stored_order(ns, name, &index));
        if list.first().map(String::as_str) == Some(key) {
            return (None, Vec::new());
        }
        list.retain(|k| k != key);
        list.insert(0, key.to_owned());
        let evicted = if list.len() > MAX_SESSIONS { list.split_off(MAX_SESSIONS) } else { Vec::new() };
        (Some(serde_json::to_string(list).unwrap_or_default()), evicted)
    });
    if let Some(order) = changed {
        crate::storage::set(ns, &index, order);
    }
    for gone in evicted {
        crate::storage::delete(ns, &gone);
        crate::plugin_host::services::store::store_changed(plugin, &gone);
    }
}

/// Store `name`'s sessions as last saved, put right against the rows there are: a listed session
/// without a row drops out, a row the list doesn't name (an imported store) goes last.
fn stored_order(ns: &str, name: &str, index: &str) -> Vec<String> {
    let prefix = format!("{name}@");
    let keys: Vec<String> = crate::storage::items(ns).unwrap_or_default().into_iter().map(|(k, _)| k).filter(|k| k.starts_with(&prefix)).collect();
    let saved: Vec<String> = crate::storage::json(ns, index).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let mut list: Vec<String> = saved.into_iter().filter(|k| keys.contains(k)).collect();
    let rest: Vec<String> = keys.into_iter().filter(|k| !list.contains(k)).collect();
    list.extend(rest);
    list
}

/// Loads plugins `ids`' namespaces on the storage thread (after stage B, never before), then runs
/// `then` on this window's thread. `boot_done`: the plugins started from the boot packet are among
/// them, so it's no longer needed.
pub fn load(app: &App, ids: Vec<String>, boot_done: bool, then: impl FnOnce(&mut App, &mut Tx) + Send + 'static) {
    let deliver = app.deliverer();
    let names: Vec<String> = ids.iter().map(|id| plugin_namespace(id)).collect();
    crate::storage::load_state(
        names,
        Box::new(move || {
            deliver(Box::new(move |app: &mut App, tx: &mut Tx| {
                loaded(boot_done);
                then(app, tx);
            }))
        }),
    );
}

/// Namespaces came in: sessions used meanwhile move first, values read before anything could
/// answer are read again, and the boot packet goes once nothing reads from it.
fn loaded(boot_done: bool) {
    let waiting = STORES.with_borrow_mut(|s| std::mem::take(&mut s.waiting));
    let (ready, later): (Vec<_>, Vec<_>) = waiting.into_iter().partition(|(p, _, _)| crate::storage::is_loaded(&plugin_namespace(p)));
    for (plugin, name, key) in ready {
        move_first(&plugin, &plugin_namespace(&plugin), &name, &key);
    }
    let provisional = STORES.with_borrow_mut(|s| {
        s.waiting.extend(later);
        if boot_done {
            s.boot = None;
        }
        std::mem::take(&mut s.provisional)
    });
    for (plugin, key) in provisional {
        crate::plugin_host::services::store::store_changed(&plugin, &key);
    }
}

/// Proof builds only: `RUNE_STORAGE_DELAY_MS` holds a lazy namespace load's reply on the storage
/// thread that long, on scratch profiles (`RUNE_DIRS_ROOT`) only.
#[cfg(feature = "storage-delay")]
pub fn delayed(names: &[String], done: Box<dyn FnOnce() + Send>) -> Box<dyn FnOnce() + Send> {
    let ms = std::env::var_os("RUNE_DIRS_ROOT").and_then(|_| std::env::var("RUNE_STORAGE_DELAY_MS").ok()).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let what = names.join(",");
    Box::new(move || {
        if ms > 0 {
            eprintln!("storage-delay: holding the reply for {what} {ms} ms");
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
        done()
    })
}

/// Reads namespace `ns` now, waiting for the storage thread (a value nothing in memory holds,
/// asked for by a running plugin): a rare path, bounded.
fn load_now(ns: &str) {
    let (tx, rx) = std::sync::mpsc::channel();
    crate::storage::load_state(
        vec![ns.to_owned()],
        Box::new(move || {
            tx.send(()).ok();
        }),
    );
    #[cfg(feature = "storage-delay")]
    let started = std::time::Instant::now();
    rx.recv_timeout(Duration::from_secs(2)).ok();
    #[cfg(feature = "storage-delay")]
    eprintln!("storage-delay: load_now({ns}) held the UI thread {} µs", started.elapsed().as_micros());
}

/// Whether plugin `id`'s first reads have its saved values: they're in memory, or the boot record
/// answers for them (before its namespace is read).
pub fn answers_for(id: &str) -> bool {
    crate::storage::is_loaded(&plugin_namespace(id)) || answers()
}

/// Whether the boot record answers for every plugin's values (or there's no record to wait for:
/// the storage is in memory).
pub fn answers() -> bool {
    STORES.with_borrow(|s| s.boot.as_ref().is_some_and(|b| b.plugins_covered())) || crate::storage::in_memory()
}

//! Moving slots through their lifecycle: loading and setting up, hot swapping, failing, turning
//! off and on. Each step keeps contracts in order: what a slot provided goes inactive when it
//! stops, and a swap replaces it in one step.

use std::time::Instant;

use opencode_plugin::EditorHandle;
use opencode_plugin::abi::{AbiOwnedBytes, AbiStatus, AbiStr};
use rune::Tx;

use super::instance::{Instance, Scope};
use super::lifecycle::{Event, PluginState};
use super::loader::{self, Loaded};
use super::shared::Live;
use super::vtable::{Kind, Request, SetupCtx};
use super::{PluginHost, Source, app_half, contrib, tasks};
use crate::App;

/// Loads slot `i` (the library the watcher already loaded, when given) and sets it up:
/// `LoadingLib → PreparingStores → SettingUp → Active`.
pub(super) fn load(app: &mut App, i: usize, prepared: Option<Loaded>) {
    let host = &mut app.plugins;
    let slot = &mut host.slots[i];
    slot.go(Event::Load);
    slot.generation += 1;
    slot.reload_error = None;
    *slot.render_error.borrow_mut() = None;
    let loaded = match (prepared, &slot.source) {
        (Some(loaded), _) => Ok(loaded),
        (None, Source::Library(path)) => loader::load(path, host.shadow_dir.as_deref(), &slot.manifest.id),
        (None, Source::Static(entry)) => loader::load_static(*entry),
    };
    let loaded = match loaded {
        Ok(l) => l,
        Err(e) => {
            tasks::log(format!("plugin {}: {e}", slot.manifest.id));
            slot.error = Some(e);
            slot.go(Event::LibFailed);
            return;
        }
    };
    slot.go(Event::LibLoaded);
    host.shared.stores.load(&slot.manifest.id);
    host.slots[i].go(Event::StoresReady);
    let generation = host.slots[i].generation;
    host.shared.contracts.reopen(&host.slots[i].manifest.id);
    match set_up(host, i, loaded, generation, &[]) {
        Ok(instance) => {
            let woken = host.shared.contracts.settle_swap(i, generation, true);
            host.wake.wake(woken, false);
            let slot = &mut host.slots[i];
            slot.tabs = tabs(&instance);
            slot.instance = Some(instance);
            slot.error = None;
            slot.go(Event::SetupOk);
        }
        Err(e) => {
            let woken = host.shared.contracts.withdraw(i, Live::Failed);
            host.wake.wake(woken, false);
            let slot = &mut host.slots[i];
            tasks::log(format!("plugin {}: {e}", slot.manifest.id));
            slot.error = Some(e);
            slot.go(Event::SetupFailed);
        }
    }
    contrib::sync(&app.plugins);
}

/// Which build of a plugin `source` holds: a linked built-in's entry point, or a library
/// file's path and when it was written (each window loads its own copy of it).
fn library_identity(source: &Source) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    match source {
        Source::Static(entry) => (*entry as usize).hash(&mut h),
        Source::Library(path) => (path, std::fs::metadata(path).and_then(|m| m.modified()).ok()).hash(&mut h),
    }
    h.finish()
}

fn tabs(instance: &Instance) -> Vec<(String, String, String)> {
    instance.contributions.panels.iter().map(|p| (p.tab.to_owned(), p.title.clone(), p.icon.clone())).collect()
}

/// Runs a plugin's setups, its app half's first, collecting what they register. On failure,
/// whatever this generation published is rolled back before its states drop.
/// The app half runs once per process: a window setting up the same library another window
/// runs shares its app half (its state, app menu items and services); a rebuilt library gets
/// a new one, which the windows that reload after it share in turn.
fn set_up(host: &mut PluginHost, i: usize, loaded: Loaded, generation: u32, reuse: &[EditorHandle]) -> Result<Instance, String> {
    let id = host.slots[i].manifest.id.clone();
    let entry = library_identity(&host.slots[i].source);
    let running = app_half::find(&id, entry);
    let mut ctx = SetupCtx {
        kind: Kind::Setup,
        scope: Scope::App,
        plugin_id: &id,
        slot: i,
        generation,
        lib: &loaded.lib,
        editors: &mut host.editors,
        reuse,
        made: Default::default(),
        wake: &host.wake,
        shared: &mut host.shared,
        woken: 0,
        requests: Vec::new(),
        app_services: Vec::new(),
    };
    let mut instance = Instance {
        generation,
        vtable: loaded.vtable,
        app: None,
        state: std::ptr::null_mut(),
        contributions: Default::default(),
        servers: Vec::new(),
        ready_sources: Vec::new(),
        deadline: None,
        lib: loaded.lib.clone(),
    };
    let halves = [(Scope::App, loaded.vtable.app_setup), (Scope::Window, loaded.vtable.window_setup)];
    let mut failed = None;
    for (scope, setup) in halves {
        let Some(setup) = setup else { continue };
        ctx.scope = scope;
        if let (Scope::App, Some(half)) = (scope, &running) {
            // This window's contracts list the half's services, as its own setup would have.
            for s in &half.services {
                if let Ok(wake) = ctx.shared.contracts.provide(i, &id, generation, &s.id, s.type_hash, s.value, None, half.lib.clone()) {
                    ctx.woken |= wake;
                }
            }
            instance.app = Some(half.clone());
            tasks::log(format!("plugin {id}: this window shares the app half another window set up"));
            continue;
        }
        // SAFETY: the plugin's setup, with a setup context alive for the call.
        let out = unsafe { setup((&raw mut ctx).cast()) };
        match out.into_result() {
            Ok((_, state)) if !state.is_null() => match scope {
                Scope::App => {
                    let mut half = app_half::AppHalf::new(state, loaded.vtable.app_drop_state, entry, loaded.lib.clone());
                    // What it registered so far is its own: app menu items.
                    half.contribs = std::mem::take(&mut ctx.made.contribs);
                    half.services = std::mem::take(&mut ctx.app_services);
                    let half = std::rc::Rc::new(half);
                    app_half::publish(&id, &half);
                    instance.app = Some(half);
                    tasks::log(format!("plugin {id}: app half set up, once for every window"));
                }
                Scope::Window => instance.state = state,
            },
            Err((AbiStatus::Panicked, message)) => failed = Some(format!("panicked during setup: {message}")),
            Err((_, message)) => failed = Some(message),
            Ok(_) => failed = Some("setup returned no state".to_owned()),
        }
        if failed.is_some() {
            break;
        }
    }
    let (made, woken, requests) = (std::mem::take(&mut ctx.made), ctx.woken, std::mem::take(&mut ctx.requests));
    if loaded.vtable.app_setup.is_none() && loaded.vtable.window_setup.is_none() {
        failed = Some("the plugin has neither an app nor a window half".to_owned());
    }
    instance.contributions = made;
    if let Some(e) = failed {
        host.shared.processes.reap(i, |g| g == generation);
        let woken = host.shared.contracts.settle_swap(i, generation, false);
        host.wake.wake(woken, false);
        for h in instance.contributions.editors.iter().filter(|h| !reuse.contains(h)) {
            host.editors.remove(*h);
        }
        return Err(e);
    }
    host.wake.wake(woken, false);
    for request in requests {
        if let Request::SetServers { source, list, ready } = request {
            instance.set_servers(&source, list, ready);
        }
    }
    Ok(instance)
}

/// Reloads slot `id`: a hot swap that keeps the running generation until the new one is set
/// up (`Active → Reloading → Active`), or a fresh load for a failed or turned-off plugin.
pub fn reload(app: &mut App, id: &str, tx: &mut Tx) {
    let Some(i) = app.plugins.slots.iter().position(|s| s.manifest.id == id) else { return };
    reload_slot(app, i, None, tx);
}

/// Reloads slot `i` with the library the watcher loaded (`prepared`), or loads it here.
pub(super) fn reload_slot(app: &mut App, i: usize, prepared: Option<super::hot::Prepared>, tx: &mut Tx) {
    tx.instant();
    let started = Instant::now();
    let (off_thread, prepared) = match prepared {
        Some(p) => (Some(p.took), Some(p.loaded)),
        None => (None, None),
    };
    // A third-party library that changed since the user OKed it waits for their OK again.
    if retrust(app, i) {
        return;
    }
    let state = app.plugins.slots[i].state;
    if matches!(state, PluginState::Failed | PluginState::Disabled | PluginState::PendingTrust) {
        tx.invalidate_memos_by_prefix(app.plugins.slots[i].prefix.clone());
        match prepared {
            Some(Err(e)) => {
                let slot = &mut app.plugins.slots[i];
                tasks::log(format!("plugin {}: {e}", slot.manifest.id));
                slot.error = Some(e);
                if slot.state == PluginState::PendingTrust {
                    slot.go(Event::Load);
                    slot.go(Event::LibFailed);
                }
            }
            Some(Ok(loaded)) => {
                load(app, i, Some(loaded));
                let (name, took) = (&app.plugins.slots[i].manifest.name, off_thread.unwrap_or_default());
                tasks::log(format!("plugin {name}: loaded off the app's thread in {:.1} ms, set up on it in {:.2} ms", took.as_secs_f64() * 1000.0, started.elapsed().as_secs_f64() * 1000.0));
            }
            None => load(app, i, None),
        }
        settle(app);
        return;
    }
    if state != PluginState::Active {
        return;
    }
    let host = &mut app.plugins;
    let slot = &mut host.slots[i];
    slot.go(Event::Reload);
    slot.generation += 1;
    let generation = slot.generation;
    let loaded = match (prepared, &slot.source) {
        (Some(loaded), _) => loaded,
        (None, Source::Library(path)) => loader::load(path, host.shadow_dir.as_deref(), &slot.manifest.id),
        (None, Source::Static(entry)) => loader::load_static(*entry),
    };
    let Some(old) = slot.instance.as_ref() else { return };
    let (old_state, old_vtable, reuse) = (old.state, old.vtable, old.contributions.editors.clone());
    let swapped = loaded.and_then(|loaded| {
        let mut snapshot = AbiOwnedBytes::EMPTY;
        let saved = match (old_vtable.save_hot_state, old_state.is_null()) {
            // SAFETY: the running instance's own state and a valid out pointer.
            (Some(save), false) => unsafe { save(old_state, &mut snapshot) }.into_result().is_ok(),
            _ => false,
        };
        let fresh = set_up(host, i, loaded, generation, &reuse)?;
        if let (true, Some(load_state), false) = (saved, fresh.vtable.load_hot_state, fresh.state.is_null()) {
            // SAFETY: the new instance's state and bytes alive for the call.
            let loaded = unsafe { load_state(fresh.state, AbiStr::from_bytes(snapshot.as_bytes())) };
            if let Err((_, message)) = loaded.into_result() {
                tasks::log(format!("plugin: carrying state across the reload failed: {message}"));
            }
        }
        Ok(fresh)
    });
    match swapped {
        Ok(fresh) => {
            let woken = host.shared.contracts.settle_swap(i, generation, true);
            host.wake.wake(woken, false);
            let slot = &mut host.slots[i];
            let kept: Vec<EditorHandle> = fresh.contributions.editors.clone();
            slot.tabs = tabs(&fresh);
            let old = slot.instance.replace(fresh);
            slot.reload_error = None;
            *slot.render_error.borrow_mut() = None;
            slot.salt.set(slot.salt.get() + 1);
            tx.invalidate_memos_by_prefix(slot.prefix.clone());
            slot.go(Event::ReloadSettled);
            let name = slot.manifest.name.clone();
            host.shared.processes.reap(i, |g| g != generation);
            if let Some(old) = old {
                for h in old.contributions.editors.iter().filter(|h| !kept.contains(h)) {
                    host.editors.remove(*h);
                }
            }
            let swap = started.elapsed().as_secs_f64() * 1000.0;
            match off_thread {
                Some(took) => tasks::log(format!("plugin {name}: reloaded (generation {generation}): loaded off the app's thread in {:.1} ms, swapped on it in {swap:.2} ms", took.as_secs_f64() * 1000.0)),
                None => tasks::log(format!("plugin {name}: reloaded (generation {generation}) in {swap:.1} ms")),
            }
        }
        Err(e) => {
            let slot = &mut host.slots[i];
            tasks::log(format!("plugin {}: reload failed, keeping the running one: {e}", slot.manifest.id));
            slot.reload_error = Some(e.clone());
            slot.go(Event::ReloadSettled);
            let name = slot.manifest.name.clone();
            crate::status_toast::fail(app, format!("Couldn't reload {name}"), e);
        }
    }
    contrib::sync(&app.plugins);
}

/// Whether slot `i`'s library changed since the user trusted it; if so it waits for their OK.
fn retrust(app: &mut App, i: usize) -> bool {
    let host = &mut app.plugins;
    let slot = &mut host.slots[i];
    if !slot.tier.needs_trust() {
        return false;
    }
    slot.hash = slot.library().and_then(loader::hash);
    if host.trusted.get(&slot.manifest.id) == slot.hash.as_ref() {
        return false;
    }
    stop(app, i, Live::Disabled);
    let slot = &mut app.plugins.slots[i];
    if slot.state.next(Event::NeedsTrust).is_some() {
        slot.go(Event::NeedsTrust);
    }
    app.plugins.trust_later.remove(&app.plugins.slots[i].manifest.id);
    true
}

/// Drops slot `i`'s instance: its contracts go `live`, its editors and memos with it.
fn stop(app: &mut App, i: usize, live: Live) {
    let host = &mut app.plugins;
    let woken = host.shared.contracts.withdraw(i, live);
    host.wake.wake(woken, false);
    host.shared.contracts.forget_consumer(i);
    host.shared.processes.reap(i, |_| true);
    host.shared.images.borrow_mut().forget(&host.slots[i].manifest.id);
    if host.dialog.as_ref().is_some_and(|d| d.slot == i) {
        host.dialog = None;
    }
    if let Some(old) = host.slots[i].instance.take() {
        for h in &old.contributions.editors {
            host.editors.remove(*h);
        }
    }
    contrib::sync(&app.plugins);
}

/// Fails slot `i` (a handler or poll panicked): withdraws everything it contributed.
pub(super) fn fail(app: &mut App, i: usize, message: String, tx: &mut Tx) {
    let slot = &mut app.plugins.slots[i];
    tasks::log(format!("plugin {}: {message}", slot.manifest.id));
    slot.error = Some(message);
    slot.go(Event::Fault);
    slot.salt.set(slot.salt.get() + 1);
    tx.invalidate_memos_by_prefix(slot.prefix.clone());
    stop(app, i, Live::Failed);
    let name = app.plugins.slots[i].manifest.name.clone();
    crate::status_toast::fail(app, format!("{name} stopped"), "The plugin crashed. Reload it from Settings › Plugins.");
}

/// Turns slot `id` on or off, remembering the choice. Turning on a library that waits for the
/// user's OK shows its trust prompt instead.
pub fn toggle_enabled(app: &mut App, id: &str, tx: &mut Tx) {
    let Some(i) = app.plugins.slots.iter().position(|s| s.manifest.id == id) else { return };
    tx.instant();
    match app.plugins.slots[i].state {
        PluginState::Active => {
            let slot = &mut app.plugins.slots[i];
            slot.go(Event::Dispose);
            tx.invalidate_memos_by_prefix(slot.prefix.clone());
            stop(app, i, Live::Disabled);
            app.plugins.slots[i].go(Event::Disposed);
            app.plugins.disabled.insert(id.to_owned());
        }
        PluginState::PendingTrust => {
            app.plugins.trust_later.remove(id);
            return;
        }
        PluginState::Disabled | PluginState::Failed => {
            app.plugins.disabled.remove(id);
            if !retrust(app, i) {
                load(app, i, None);
            }
        }
        _ => return,
    }
    settle(app);
    super::save_state(app);
}

/// Contracts whose provider won't come go inactive (after loading, and each change).
pub(super) fn settle(app: &mut App) {
    let host = &mut app.plugins;
    let states: Vec<(String, PluginState)> = host.slots.iter().map(|s| (s.manifest.id.clone(), s.state)).collect();
    let woken = host.shared.contracts.settle(|id| {
        let state = states.iter().find(|(s, _)| s == id).map(|(_, st)| *st)?;
        Some(match state {
            PluginState::Active => Live::Active,
            PluginState::Failed => Live::Failed,
            PluginState::LoadingLib | PluginState::PreparingStores | PluginState::SettingUp | PluginState::Reloading => Live::Pending,
            _ => Live::Disabled,
        })
    });
    host.wake.wake(woken, false);
}
//! Plugin runtime lifecycle: stepping `Life`, `Trust`, and `Hot`, starting/stopping/reloading
//! Luau VMs, delivering host signals, and the per-frame `poll` loop.

use rune::Tx;
use rune_luau::{Data, Script};

use super::boot::save_record;
use super::{app_half, dormant, hot, plugin, sources, vm};
use crate::App;
use crate::plugin_host::{
    FlyoutEvent, Hot, HotEffect, Life, LifeEffect, LifeEvent, Origin, Overlay, Trust, TrustEffect, TrustEvent,
    bump, console, cx, epoch, mark, project_request, requests, server_sources, servers, services, stores, tasks,
};

/// Steps plugin `i`'s life with `event` and carries out what the step asks for.
pub fn drive(app: &mut App, i: usize, event: LifeEvent) {
    let life = std::mem::replace(&mut app.plugins.list[i].life, Life::Discovered);
    let (life, effects) = life.step(event);
    app.plugins.list[i].life = life;
    for effect in effects {
        match effect {
            LifeEffect::Start => start(app, i),
            LifeEffect::Stop(script) => stop(app, i, script),
            LifeEffect::Report(e) => console::push(&app.plugins.list[i].manifest.id, rune_luau::LogLevel::Error, &e),
        }
    }
    bump();
}

/// [`LifeEffect::Start`]: plugin `i`'s stores, catalogs, app half and window half; then
/// `Started` or `StartFailed`.
fn start(app: &mut App, i: usize) {
    // Its first read has its saved value: its values come into memory first.
    if !app.plugins.list[i].manifest.stores.is_empty() && !stores::answers_for(&app.plugins.list[i].manifest.id) {
        let id = app.plugins.list[i].manifest.id.clone();
        stores::load(app, vec![id.clone()], false, move |app, tx| {
            if let Some(i) = app.plugins.find(&id).filter(|&i| matches!(app.plugins.list[i].life, Life::Loading)) {
                start_loaded(app, i);
                rune::Tracked::touch(&app.plugins);
                tx.instant();
            }
        });
        return;
    }
    start_loaded(app, i);
}

/// The first piece of a dormant built-in's start in idle time (`dormant::poll`): its app half,
/// when it has one that doesn't run yet and its values are in memory. True when it started it.
pub(super) fn start_app_half(app: &mut App, id: &str) -> bool {
    let Some(i) = app.plugins.find(id) else { return false };
    let p = &app.plugins.list[i];
    let Some(entry) = p.manifest.app.clone() else { return false };
    if app_half::runs(id) || !stores::answers_for(id) {
        return false;
    }
    let files = p.origin.files();
    let catalogs = p.origin.catalogs(&files);
    let manifest = p.manifest.clone();
    let sources = sources::scripts(&p.origin, &files, &manifest.id, &entry);
    app_half::ensure(app, p.tier, &manifest, &files, catalogs, sources);
    true
}

/// [`start`] once plugin `i`'s values are in memory.
fn start_loaded(app: &mut App, i: usize) {
    // Keep screen active was the app's own setting before the Pairing plugin took the page: it
    // moves into the plugin's store once.
    if app.plugins.list[i].manifest.id == "pairing"
        && app.settings.keep_screen_active
        && stores::text("pairing", "plugin.pairing", "keepScreenActive").is_none()
    {
        stores::set("plugin.pairing", "keepScreenActive", "true".to_owned());
    }
    let files = app.plugins.list[i].origin.files();
    let p = &app.plugins.list[i];
    let (id, manifest) = (p.manifest.id.clone(), p.manifest.clone());
    let catalogs = p.origin.catalogs(&files);
    *p.half.catalogs.borrow_mut() = catalogs.clone();
    sources::intern_icons(&id, &files);
    if let Some(entry) = &manifest.app {
        app_half::ensure(app, p.tier, &manifest, &files, catalogs, sources::scripts(&p.origin, &files, &manifest.id, entry));
    }
    let folder = p
        .origin
        .dir()
        .and_then(|d| d.file_name())
        .map_or_else(|| id.clone(), |n| n.to_string_lossy().into_owned());
    let sources = sources::scripts(&p.origin, &files, &folder, &manifest.entry);
    let options = vm::options(p.half.clone());
    let script = cx::enter(app, || Script::with_options(&id, sources, options));
    if let Some(w) = app.plugins.waker.clone() {
        script.set_waker(w);
    }
    let half = app.plugins.list[i].half.clone();
    let vm = script.vm_id();
    let adopted = half.adopt(vm);
    services::reap(&half, vm);
    services::store::withdraw_owner(&id, plugin::Kind::Window, vm);
    app.plugins.clear_model_badges(&id, vm);
    let event = match (script.error(), adopted) {
        (None, true) => LifeEvent::Started(script),
        (error, _) => {
            let why = error.map_or_else(|| "its setup contributed nothing".to_owned(), |e| e.message);
            LifeEvent::StartFailed(script, why)
        }
    };
    drive(app, i, event);
    if app.plugins.list[i].running() {
        dormant::note_started(app, i);
    }
}

/// [`LifeEffect::Stop`]: fires `Disposing` in `script`, then withdraws what plugin `i`
/// contributed and reaps its processes; the script goes with this call.
fn stop(app: &mut App, i: usize, script: Script) {
    let half = app.plugins.list[i].half.clone();
    if let Some(vm) = script.vm_id() {
        let fire = half.fire.borrow().get(&vm).cloned();
        let signal = half.signals.borrow().get(&(vm, "ctx.Disposing".to_owned())).cloned();
        if let (Some(fire), Some(signal)) = (fire, signal) {
            cx::enter(app, || script.spawn_fn(&fire, signal, None).ok());
        }
    }
    drop(script);
    *half.contribs.borrow_mut() = plugin::Contribs::default();
    services::reap(&half, None);
    services::store::withdraw_owner(&half.id, plugin::Kind::Window, None);
    app.plugins.clear_model_badges(&half.id, None);
    app.plugins.shared.images.borrow_mut().forget(&half.id);
    app.plugins.overlay.close_for(&half.id);
}

/// Reloads plugin `i` from its files now (`prepared`, when the watcher compiled them), keeping
/// hot state. A failed compile or setup keeps the running VM; a failed plugin starts again.
pub fn reload(app: &mut App, i: usize, prepared: Option<hot::Prepared>) {
    let p = &app.plugins.list[i];
    if !p.running() && !matches!(p.life, Life::Errored { .. }) {
        return;
    }
    let prepared = match prepared {
        Some(prep) => prep,
        // A folder is read again on a worker, never on the app's thread.
        None if p.origin.dir().is_some() => {
            let (origin, manifest, id, deliver) = (p.origin.clone(), p.manifest.clone(), p.manifest.id.clone(), app.deliverer());
            tasks::run(move || {
                let prepared = hot::prepare(&origin, &manifest);
                deliver(Box::new(move |app: &mut App, tx: &mut Tx| {
                    let Some(i) = app.plugins.find(&id) else { return };
                    match prepared {
                        Ok(prep) => reload(app, i, Some(prep)),
                        Err(e) => console::push(&id, rune_luau::LogLevel::Error, &e),
                    }
                    tx.instant();
                }));
            });
            return;
        }
        None => match hot::prepare(&p.origin, &p.manifest) {
            Ok(prep) => prep,
            Err(e) => return console::push(&p.manifest.id, rune_luau::LogLevel::Error, &e),
        },
    };
    // A folder plugin runs from what was read now; one from outside the app runs it only under an
    // OK for those bytes.
    if let Origin::Dir { dir, .. } = &p.origin {
        app.plugins.list[i].origin = Origin::Dir { dir: dir.clone(), files: prepared.files.clone().into() };
    }
    if app.plugins.list[i].tier.needs_trust() {
        step_trust(app, i, TrustEvent::FilesChanged(prepared.hash.clone()));
        if !app.plugins.list[i].trust.allows() {
            app_half::stop(&app.plugins.list[i].manifest.id.clone());
            return drive(app, i, LifeEvent::NeedTrust);
        }
    }
    let p = &app.plugins.list[i];
    if matches!(p.life, Life::Errored { .. }) {
        return drive(app, i, LifeEvent::Reload);
    }
    *p.half.catalogs.borrow_mut() = prepared.catalogs;
    sources::intern_icons(&p.manifest.id, &prepared.files);
    drive(app, i, LifeEvent::Reload);
    let p = &app.plugins.list[i];
    let Some(script) = p.script() else { return };
    let half = p.half.clone();
    let result = cx::enter(app, || script.reload_compiled(prepared.sources, prepared.compiled, prepared.compile));
    let vm = script.vm_id();
    let id = app.plugins.list[i].manifest.id.clone();
    let outcome = match result {
        Ok(report) => {
            half.adopt(vm);
            services::reap(&half, vm);
            services::store::withdraw_owner(&id, plugin::Kind::Window, vm);
            app.plugins.clear_model_badges(&id, vm);
            console::reloaded(&id, report);
            Ok(())
        }
        Err(e) => {
            half.made.borrow_mut().take();
            Err(e.message)
        }
    };
    drive(app, i, LifeEvent::Reloaded(outcome));
}

/// Runs what is due in every plugin: finished work, timers, process output, server events,
/// store and contract changes, saved files, and steps their servers' sign-ins. At rest it enters
/// no VM.
/// `fresh`: the app's state may have moved since the last poll (`App::poll_panels`); only then
/// are the plugins' watched signals compared again.
pub fn poll(app: &mut App, fresh: bool, tx: &mut Tx) -> bool {
    let entry_epoch = epoch();
    servers::poll_actions(app, tx);
    let now = *app.clock;
    mark(&app.plugins, now);
    // Set and taken through a cell: a hover or visible-rows entry that asked for nothing writes
    // no host state, so the regions showing plugin surfaces keep (the poll that carries out what
    // it asked writes what that changes).
    let asked = app.plugins.asked.replace(false);
    let mut changed = false;
    changed |= dormant::poll(app, tx);
    if app.plugins.boot.later().is_empty() {
        dormant::save(app);
    }
    changed |= app_half::poll(app, now, tx);
    #[cfg(feature = "alloc-sites")]
    super::memory::publish_vms(app);
    project_request::poll(app, tx);
    // Images that landed or settled: the surfaces that drew them read them again.
    changed |= app.plugins.shared.images.borrow_mut().poll();
    // A flyout row's aim came due: the row under the resting pointer takes over.
    if let Overlay::Popover { flyout, .. } = &app.plugins.overlay
        && let Some((attempt, due)) = flyout.due()
        && now + 1e-9 >= due
    {
        requests::step_flyout(app, None, FlyoutEvent::AimDue { attempt }, tx);
        changed = true;
    }
    let saves = app.plugins.watcher.as_ref().map(hot::Watcher::take).unwrap_or_default();
    for (i, event) in saves {
        let hot = std::mem::replace(&mut app.plugins.list[i].hot, Hot::Idle);
        let (hot, effects) = hot.step(event);
        app.plugins.list[i].hot = hot;
        for effect in effects {
            match effect {
                HotEffect::Prewarm => {
                    if let Some(script) = app.plugins.list[i].script() {
                        cx::enter(app, || script.prewarm());
                    }
                }
                HotEffect::Swap(prepared) => reload(app, i, Some(*prepared)),
                HotEffect::Report(e) => console::push(&app.plugins.list[i].manifest.id, rune_luau::LogLevel::Error, &e),
            }
            changed = true;
        }
    }
    let events = std::mem::take(&mut *app.plugins.events.borrow_mut());
    let mut delivered = false;
    for i in 0..app.plugins.list.len() {
        let p = &app.plugins.list[i];
        let Some(vm) = p.script().and_then(Script::vm_id) else { continue };
        let half = p.half.clone();
        let before = half.epoch.get();
        let mut ran = false;
        for (signal, args) in services::process::drain(&half, vm) {
            ran |= fire_value(app, i, signal, args);
        }
        // Taken only when there is some: a write to the host rebuilds every region showing it.
        let caught = if app.plugins.dormant.catch_up.contains_key(&*half.id) {
            app.plugins.dormant.catch_up.remove(&*half.id).unwrap_or_default()
        } else {
            Vec::new()
        };
        for (ty, data) in caught.iter().chain(&events) {
            let listeners: Vec<mlua::Value> = half
                .events
                .borrow()
                .iter()
                .filter(|(v, prefix, _)| *v == vm && ty.starts_with(prefix.as_str()))
                .map(|(_, _, s)| s.clone())
                .collect();
            for signal in listeners {
                let event = services::map([
                    ("type", Data::String(ty.clone())),
                    ("data", services::json_to_data(data)),
                    ("serverId", Data::String(app.server_key().unwrap_or_else(|| "local".into()))),
                ]);
                ran |= fire_value(app, i, signal, Data::List(vec![event]));
            }
        }
        if fresh || changed || delivered || ran {
            ran |= services::fire_watched(app, i, vm);
        }
        let p = &app.plugins.list[i];
        let Some(script) = p.script() else { continue };
        ran |= cx::enter(app, || script.poll(now, tx));
        delivered |= ran;
        changed |= half.epoch.get() != before || (!half.owns_redraw && ran);
    }
    requests::carry_out(app, tx);
    changed |= entry_epoch != epoch();
    // What a plugin shows lives in its VM, which `Tracked` can't see: a poll that changed what
    // plugins draw marks the host written, so the regions that show plugin surfaces build again.
    if changed {
        rune::Tracked::touch(&app.plugins);
    }
    app.plugins.clock.set(now);
    if app.plugins.synced.get() != epoch() {
        app.plugins.synced.set(epoch());
        crate::plugin_host::contrib::sync(app);
    }
    changed || asked
}

/// Fires `signal` of plugin `i`'s running VM with `args` (a list is spread).
fn fire_value(app: &App, i: usize, signal: mlua::Value, args: Data) -> bool {
    let p = &app.plugins.list[i];
    let (Some(script), Some(vm)) = (p.script(), p.script().and_then(Script::vm_id)) else { return false };
    let Some(fire) = p.half.fire.borrow().get(&vm).cloned() else { return false };
    let args = match args {
        Data::List(items) => items,
        other => vec![other],
    };
    cx::enter(app, || {
        let mut values = mlua::MultiValue::new();
        values.push_back(signal);
        let converted = script.with_lua(|lua| args.into_iter().map(|a| mlua::IntoLua::into_lua(a, lua)).collect::<mlua::Result<Vec<_>>>());
        if let Some(Ok(items)) = converted {
            values.extend(items);
        }
        script.spawn_fn(&fire, values, None).is_ok()
    })
}

/// Fires plugin `i`'s host signal `name` with `payload`, if its running VM made it.
pub fn fire_named(app: &App, i: usize, name: &str, payload: Data) -> bool {
    let p = &app.plugins.list[i];
    let Some(vm) = p.script().and_then(Script::vm_id) else { return false };
    let Some(signal) = p.half.signals.borrow().get(&(vm, name.to_owned())).cloned() else { return false };
    fire_value(app, i, signal, payload)
}

/// Settings › Plugins' switch: turns plugin `id` off, or on (asking for trust first when its
/// files need an OK the user hasn't given).
pub fn toggle(app: &mut App, id: &str, tx: &mut Tx) {
    tx.instant();
    let Some(i) = app.plugins.find(id) else { return };
    let p = &app.plugins.list[i];
    match &p.life {
        Life::Running(_) | Life::Reloading(_) | Life::Errored { .. } | Life::AwaitingTrust | Life::Dormant => {
            dormant::forget(app, id);
            drive(app, i, LifeEvent::Disable);
            app_half::stop(id);
            server_sources::forget(id);
            app.plugins.disabled.insert(id.to_owned());
            // A plugin from outside the app: the OK goes with the switch, so on again asks again.
            step_trust(app, i, TrustEvent::Revoke);
        }
        Life::Disabled => {
            let allows = p.trust.allows();
            app.plugins.disabled.remove(id);
            if allows {
                drive(app, i, LifeEvent::Load);
            } else {
                step_trust(app, i, TrustEvent::Ask);
                drive(app, i, LifeEvent::NeedTrust);
            }
        }
        Life::Discovered | Life::Loading | Life::Refused(_) => return,
    }
    save_record(app, id, None);
}

/// Steps plugin `i`'s trust with `event` and carries out what the step asks for.
pub fn step_trust(app: &mut App, i: usize, event: TrustEvent) {
    let trust = std::mem::replace(&mut app.plugins.list[i].trust, Trust::NotNeeded);
    let (trust, effects) = trust.step(event);
    app.plugins.list[i].trust = trust;
    let id = app.plugins.list[i].manifest.id.clone();
    for effect in effects {
        match effect {
            TrustEffect::Save(hash) => {
                app.plugins.disabled.remove(&id);
                app.plugins.trusted.insert(id.clone(), hash);
                let who = id.clone();
                save_record(
                    app,
                    &id,
                    Some(Box::new(move |app: &mut App, tx: &mut Tx, result| {
                        if let Some(i) = app.plugins.find(&who) {
                            step_trust(app, i, TrustEvent::Saved(result));
                        }
                        tx.instant();
                    })),
                );
            }
            // The caller writes the record (with the switch it changed).
            TrustEffect::Forget => {
                app.plugins.trusted.remove(&id);
            }
            TrustEffect::Load => drive(app, i, LifeEvent::Load),
            TrustEffect::Report(e) => console::push(&id, rune_luau::LogLevel::Error, &e),
        }
    }
}

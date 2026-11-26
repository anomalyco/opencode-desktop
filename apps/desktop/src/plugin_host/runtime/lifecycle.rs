//! Plugin runtime lifecycle: stepping `Life`, `Trust`, and `Hot`, starting/stopping/reloading
//! Luau VMs, delivering host signals, and the per-frame `poll` loop.

use std::sync::atomic::{AtomicU64, Ordering};

use rune::Tx;
use rune_luau::{Data, Script};

use super::boot::save_record;
use super::{app_half, dormant, hot, plugin, sources, themes, vm};
use crate::App;
use crate::plugin_host::{
    FlyoutEvent, Hot, HotEffect, Life, LifeEffect, LifeEvent, Origin, Overlay, Trust, TrustEffect, TrustEvent,
    bump, console, cx, epoch, machines::HotEvent, mark, project_request, requests, servers, services, stores, tasks,
};

/// Steps plugin `i`'s life with `event` and carries out what the step asks for.
pub fn drive(app: &mut App, i: usize, event: LifeEvent) {
    let p = &mut app.plugins.list[i];
    if matches!(event, LifeEvent::Disable | LifeEvent::NeedTrust | LifeEvent::Remove | LifeEvent::Sleep) && matches!(p.hot, Hot::Manual(_)) {
        p.hot = Hot::Idle;
    }
    let life = std::mem::replace(&mut p.life, Life::Discovered);
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
    if !stores::answers_for(id) {
        return false;
    }
    let files = p.origin.files();
    let revision = app_half::Revision::new(&p.manifest, p.tier, &files);
    if !app_half::available(&revision) || app_half::runs(&revision) {
        return false;
    }
    let catalogs = p.origin.catalogs(&files);
    let sources = sources::scripts(&p.origin, &files, &p.manifest.id, &entry);
    app.plugins.shared_plugin_owners.hold(&p.half.token, &revision);
    app_half::ensure(app, revision, &files, catalogs, sources);
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
    let revision = app_half::Revision::new(&manifest, p.tier, &files);
    if !app_half::available(&revision) {
        drive(app, i, LifeEvent::WaitForAppHalf);
        return;
    }
    let catalogs = p.origin.catalogs(&files);
    *p.half.catalogs.borrow_mut() = catalogs.clone();
    sources::intern_icons(&id, &files);
    start_shared(app, p, revision, &files, catalogs);
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
    services::browser_contract::revoke_half(&half.token, vm);
    app.plugins.clear_model_badges(&id, vm);
    let timed_out = script.error().is_some_and(|e| e.kind == rune_luau::ErrorKind::Timeout);
    let event = match (script.error(), adopted) {
        (None, true) => LifeEvent::Started(script),
        (error, _) => {
            let why = error.map_or_else(|| "its setup contributed nothing".to_owned(), |e| e.message);
            LifeEvent::StartFailed(script, why)
        }
    };
    drive(app, i, event);
    if timed_out {
        restart_later(app, &id);
    }
    if app.plugins.list[i].running() {
        themes::publish(app, i);
        dormant::note_started(app, i);
    }
}

fn start_shared(app: &App, p: &crate::plugin_host::Plugin, revision: app_half::Revision, files: &[(String, String)], catalogs: crate::plugin_host::Catalogs) {
    let owners = &app.plugins.shared_plugin_owners;
    owners.hold(&p.half.token, &revision);
    owners.adopt_staged(&p.half.token, &revision);
    if let Some(entry) = &p.manifest.app {
        app_half::ensure(app, revision, files, catalogs, sources::scripts(&p.origin, files, &p.manifest.id, entry));
    }
}

fn poll_waiting_app_halves(app: &mut App, tx: &mut Tx) -> bool {
    let mut waiting = app.plugins.list.iter().enumerate().filter(|(_, p)| matches!(p.life, Life::WaitingForAppHalf)).peekable();
    if waiting.peek().is_none() {
        return false;
    }
    let ready = waiting.find_map(|(i, p)| {
        app_half::available(&app_half::Revision::new(&p.manifest, p.tier, &p.origin.files())).then_some(i)
    });
    tx.wake_after(std::time::Duration::from_millis(250));
    if let Some(i) = ready {
        drive(app, i, LifeEvent::Load);
        return true;
    }
    false
}

/// How long a plugin whose start ran out of time shows its error card before it starts again, in s.
const RESTART_AFTER: f64 = 2.0;
/// Restarts a plugin gets after its start ran out of time, per window, never reset: one. It repairs
/// a false positive of our own watchdog (a busy machine at boot); official has no such watchdog and
/// restarts nothing by itself. A start that times out again stays on its error card with Reload.
const RESTARTS: u32 = 1;

/// The window's plugins whose start ran out of time: each restart still to come (the plugin, and
/// when it is due on `app.clock`), and how many each plugin had. Leaving the error card any other
/// way (Reload, disable, trust) stops its script, which drops the plugin's pending restart.
#[derive(Default)]
pub struct Restarts {
    due: Vec<(String, f64)>,
    count: std::collections::BTreeMap<String, u32>,
}

impl Restarts {
    /// The window is closing or the app is quitting: no plugin starts again on the way out.
    pub fn cancel_all(&mut self) {
        self.due.clear();
    }
}

/// Plugin `id`'s start ran past the watchdog: it starts again in [`RESTART_AFTER`], up to
/// [`RESTARTS`] time; meanwhile its surfaces show the error card and the console says so.
fn restart_later(app: &mut App, id: &str) {
    let at = *app.clock + RESTART_AFTER;
    let restarts = &mut app.plugins.restarts;
    let n = restarts.count.entry(id.to_owned()).or_insert(0);
    if *n >= RESTARTS {
        return;
    }
    *n += 1;
    restarts.due.push((id.to_owned(), at));
    console::push(id, rune_luau::LogLevel::Warn, "its start ran out of time; starting it again");
    // A frame soon, whose poll sets the wake for the restart.
    if let Some(w) = app.plugins.waker.clone() {
        w();
    }
}

/// Starts again one plugin whose restart came due ([`restart_later`]) and wakes for the next. One a
/// frame: plugins that timed out together in a busy boot don't all start again in one frame.
fn poll_restarts(app: &mut App, tx: &mut Tx) -> bool {
    let now = *app.clock;
    // Read first: a `&mut` borrow of the host is a write, after which every region showing a
    // plugin surface (the sidebar, titlebar and session header) builds again.
    let id = app.plugins.restarts.due.iter().position(|(_, at)| *at <= now).map(|k| app.plugins.restarts.due.remove(k).0);
    if let Some(next) = app.plugins.restarts.due.iter().map(|(_, at)| *at).min_by(f64::total_cmp) {
        tx.wake_at(next.max(now));
    }
    let Some(i) = id.and_then(|id| app.plugins.find(&id)).filter(|&i| matches!(app.plugins.list[i].life, Life::Errored { .. })) else {
        return false;
    };
    drive(app, i, LifeEvent::Load);
    true
}

/// [`LifeEffect::Stop`]: fires `Disposing` in `script`, then withdraws what plugin `i`
/// contributed and reaps its processes; the script goes with this call.
fn stop(app: &mut App, i: usize, script: Script) {
    // Off its error card some other way first (Reload, disable, trust): no restart comes after.
    let id = app.plugins.list[i].manifest.id.clone();
    app.plugins.restarts.due.retain(|(k, _)| *k != id);
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
    half.themes_changed.set(false);
    themes::withdraw(app, &id);
    services::reap(&half, None);
    services::store::withdraw_owner(&half.id, plugin::Kind::Window, None);
    services::browser_contract::revoke_half(&half.token, None);
    app.plugins.clear_model_badges(&half.id, None);
    app.plugins.shared.images.borrow_mut().forget(&half.id);
    crate::plugin_host::icons::clear_plugin(&half.id);
    app.plugins.overlay.close_for(&half.id);
    if app.plugins.dialog.as_ref().is_some_and(|d| d.plugin == half.id) {
        app.plugins.dialog = None;
        opencode_kernel::AppOverlays::on_overlay_closed(app, "plugin.dialog");
    }
}

/// Reloads plugin `i` from its files now (`prepared`, when the watcher compiled them), keeping
/// hot state. A failed compile or setup keeps the running VM; a failed plugin starts again.
pub fn reload(app: &mut App, i: usize, prepared: Option<hot::Prepared>) {
    let p = &app.plugins.list[i];
    if !p.running() && !matches!(p.life, Life::Errored { .. } | Life::AwaitingTrust | Life::WaitingForAppHalf) {
        return;
    }
    let prepared = match prepared {
        Some(prep) => prep,
        // A folder is read again on a worker, never on the app's thread.
        None if p.origin.dir().is_some() => {
            static NEXT_RELOAD: AtomicU64 = AtomicU64::new(1);
            let attempt = NEXT_RELOAD.fetch_add(1, Ordering::Relaxed);
            let (origin, manifest, id, deliver) = (p.origin.clone(), p.manifest.clone(), p.manifest.id.clone(), app.deliverer());
            drive_hot(app, i, HotEvent::Manual(attempt));
            tasks::run(move || {
                let result = hot::prepare(&origin, &manifest).map(Box::new);
                deliver(Box::new(move |app: &mut App, tx: &mut Tx| {
                    let Some(i) = app.plugins.list.iter().position(|p| p.manifest.id == id && matches!(p.hot, Hot::Manual(current) if current == attempt)) else { return };
                    drive_hot(app, i, HotEvent::ManualReady { attempt, result });
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
    let replacement = if let Some(dir) = p.origin.dir() {
        let Some(manifest) = replacement_manifest(p, &prepared) else { return };
        Some((manifest, Origin::Dir { dir: dir.to_path_buf(), files: prepared.files.clone().into() }))
    } else {
        None
    };
    if matches!(app.plugins.list[i].life, Life::AwaitingTrust) {
        install_source(app, i, replacement);
        step_trust(app, i, TrustEvent::FilesChanged(prepared.hash));
        return;
    }
    if matches!(app.plugins.list[i].life, Life::WaitingForAppHalf) {
        install_source(app, i, replacement);
        step_trust(app, i, TrustEvent::FilesChanged(prepared.hash));
        if !app.plugins.list[i].trust.allows() {
            drive(app, i, LifeEvent::NeedTrust);
        }
        return;
    }
    if app.plugins.list[i].tier.needs_trust() {
        step_trust(app, i, TrustEvent::FilesChanged(prepared.hash.clone()));
        if !app.plugins.list[i].trust.allows() {
            install_source(app, i, replacement);
            let id = app.plugins.list[i].manifest.id.clone();
            drive(app, i, LifeEvent::NeedTrust);
            app.plugins.shared_plugin_owners.release(&app.plugins.list[i].half.token);
            return;
        }
    }
    let p = &app.plugins.list[i];
    if matches!(p.life, Life::Errored { .. }) {
        if let Some((manifest, _)) = &replacement {
            let revision = app_half::Revision::new(manifest, p.tier, &prepared.files);
            app.plugins.shared_plugin_owners.release_except(&p.half.token, Some(&revision));
        }
        install_source(app, i, replacement);
        return drive(app, i, LifeEvent::Reload);
    }
    let catalogs = p.half.catalogs.replace(prepared.catalogs);
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
            if half.adopt(vm) {
                // Hot reload swaps the window VM. The process-wide app VM keeps its
                // admitted bytes until the last owner stops.
                install_source(app, i, replacement);
                themes::publish(app, i);
            }
            services::reap(&half, vm);
            services::store::withdraw_owner(&id, plugin::Kind::Window, vm);
            services::browser_contract::revoke_half(&half.token, vm);
            app.plugins.clear_model_badges(&id, vm);
            if app.plugins.dialog.as_ref().is_some_and(|d| {
                *d.plugin == *id && !half.contribs.borrow().dialogs.iter().any(|c| c.spec.id == d.id)
            }) {
                app.plugins.dialog = None;
                opencode_kernel::AppOverlays::on_overlay_closed(app, "plugin.dialog");
            }
            console::reloaded(&id, report);
            Ok(())
        }
        Err(e) => {
            half.catalogs.replace(catalogs);
            half.made.borrow_mut().take();
            Err(e.message)
        }
    };
    drive(app, i, LifeEvent::Reloaded(outcome));
}

fn install_source(app: &mut App, i: usize, source: Option<(opencode_plugin_spec::Manifest, Origin)>) {
    if let Some((manifest, origin)) = source {
        let p = &mut app.plugins.list[i];
        p.manifest = manifest;
        p.origin = origin;
    }
}

fn removed(app: &mut App, i: usize) {
    let p = &app.plugins.list[i];
    let Some(dir) = p.origin.dir().map(std::path::Path::to_path_buf) else { return };
    let id = p.manifest.id.clone();
    dormant::forget(app, &id);
    drive(app, i, LifeEvent::Remove);
    app.plugins.shared_plugin_owners.release(&app.plugins.list[i].half.token);
    let p = &mut app.plugins.list[i];
    p.origin = Origin::Dir { dir, files: Vec::new().into() };
    p.trust = Trust::at_launch(p.tier.needs_trust(), "", None);
}

fn replacement_manifest(p: &crate::plugin_host::Plugin, prepared: &hot::Prepared) -> Option<opencode_plugin_spec::Manifest> {
    let (_, source) = prepared.files.iter().find(|(path, _)| path == "plugin.toml")?;
    let manifest = opencode_plugin_runtime::sources::parse(source).ok()?;
    // A watched slot belongs to one ID. Changing it requires a fresh discovery pass.
    let reason = if manifest.id != p.manifest.id {
        Some("A reload cannot change the plugin's id".to_owned())
    } else {
        super::boot::refusal(p.tier, &manifest)
    };
    if let Some(reason) = reason {
        console::push(&p.manifest.id, rune_luau::LogLevel::Error, &reason);
        return None;
    }
    Some(manifest)
}

fn rediscover(app: &mut App, i: usize, prepared: hot::Prepared) {
    let p = &app.plugins.list[i];
    let Some(dir) = p.origin.dir().map(std::path::Path::to_path_buf) else { return };
    let Some(manifest) = replacement_manifest(p, &prepared) else { return };
    let id = manifest.id.clone();
    let trust = Trust::at_launch(p.tier.needs_trust(), &prepared.hash, app.plugins.trusted.get(&id));
    let event = if app.plugins.disabled.contains(&id) {
        LifeEvent::Disable
    } else if !trust.allows() {
        LifeEvent::NeedTrust
    } else {
        LifeEvent::Load
    };
    let p = &mut app.plugins.list[i];
    p.manifest = manifest;
    p.origin = Origin::Dir { dir, files: prepared.files.into() };
    p.trust = trust;
    p.half = plugin::Half::new(&id, plugin::Kind::Window, p.tier);
    drive(app, i, LifeEvent::Rediscover);
    drive(app, i, event);
}

fn drive_hot(app: &mut App, i: usize, event: HotEvent) -> bool {
    let hot = std::mem::replace(&mut app.plugins.list[i].hot, Hot::Idle);
    let (hot, effects) = hot.step(event);
    app.plugins.list[i].hot = hot;
    let changed = !effects.is_empty();
    for effect in effects {
        match effect {
            HotEffect::Prewarm => {
                if let Some(script) = app.plugins.list[i].script() {
                    cx::enter(app, || script.prewarm());
                }
            }
            HotEffect::Swap(prepared) if matches!(app.plugins.list[i].life, Life::Missing) => rediscover(app, i, *prepared),
            HotEffect::Swap(prepared) => reload(app, i, Some(*prepared)),
            HotEffect::Withdraw => removed(app, i),
            HotEffect::Report(e) => console::push(&app.plugins.list[i].manifest.id, rune_luau::LogLevel::Error, &e),
        }
    }
    changed
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
    let focused_pill = tx.focused().and_then(|k| k.strip_prefix("plugin-titlebar-"));
    if app.plugins.titlebar_focus.as_deref() != focused_pill {
        app.plugins.titlebar_focus = focused_pill.map(str::to_owned);
        changed = true;
    }
    changed |= dormant::poll(app, tx);
    changed |= poll_restarts(app, tx);
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
        changed |= drive_hot(app, i, event);
    }
    changed |= poll_waiting_app_halves(app, tx);
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
    for (i, plugin) in app.plugins.running() {
        if plugin.half.themes_changed.get() {
            themes::publish(app, i);
        }
    }
    if app.plugins.themes.take_changed() {
        // The open picker captured its old options; its next open reads the window's new list.
        if app.popup.open_at("settings-theme-select-trigger") || app.popup.open_at("theme-select") {
            crate::popup::dismiss(app, tx);
        }
        tx.instant();
        changed = true;
    }
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
        Life::Running(_) | Life::Reloading(_) | Life::Errored { .. } | Life::AwaitingTrust | Life::Dormant | Life::WaitingForAppHalf => {
            dormant::forget(app, id);
            drive(app, i, LifeEvent::Disable);
            app.plugins.shared_plugin_owners.release(&app.plugins.list[i].half.token);
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
        Life::Discovered | Life::Missing | Life::Loading | Life::Refused(_) => return,
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
            TrustEffect::Save { hash, attempt } => {
                let who = id.clone();
                let granted_hash = hash.clone();
                save_record(
                    app,
                    &id,
                    Some((hash, Box::new(move |app: &mut App, tx: &mut Tx, result| {
                        let Some(i) = app.plugins.list.iter().position(|p| {
                            p.manifest.id == who && matches!(p.trust, Trust::Granting { attempt: pending, .. } if pending == attempt)
                        }) else { return };
                        app.plugins.disabled.remove(&who);
                        if result.is_ok() {
                            app.plugins.trusted.insert(who, granted_hash);
                        }
                        step_trust(app, i, TrustEvent::Saved { attempt, result });
                        tx.instant();
                    }))),
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

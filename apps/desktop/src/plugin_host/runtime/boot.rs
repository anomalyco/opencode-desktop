//! Plugin discovery, staged startup (first screen before first frame, dormant records, and
//! one-per-turn deferred loading), plugin folder scanning, and tier adoption.

pub(crate) use opencode_plugin_runtime::boot::{Boot, BootEffect, BootEvent, Doze};
use opencode_plugin_spec::Manifest;
use rune::{Tx, Waker};

use super::{app_half, dormant, first_screen, hot, plugin, sources};
use crate::App;
use crate::plugin_host::{
    Half, Hot, Life, LifeEvent, Plugin, Tier, Trust, contrib, drive, process, servers, services, stores, tasks,
};

/// Starts a single built-in plugin synchronously on `app` when not yet running (used by
/// standalone storybook stages, which construct a fresh `App` without booting every plugin).
pub fn boot_builtin(app: &mut App, id: &str) {
    if app.plugins.waker.is_none() {
        app.plugins.waker = (*app.waker).clone();
    }
    if app.plugins.find(id).is_some_and(|i| app.plugins.list[i].running()) {
        return;
    }
    if app.plugins.find(id).is_some() {
        dormant::wake(app, id, dormant::Why::Use);
        if app.plugins.find(id).is_some_and(|i| app.plugins.list[i].running()) {
            return;
        }
    }
    stores::begin();
    if let Some(found) = sources::builtins().into_iter().find(|f| f.manifest.id == id) {
        adopt_one(app, found, None);
    }
}

/// Discovers and starts the plugins once the window's waker exists (`--no-plugins` skips it).
pub fn setup(app: &mut App, waker: Waker) {
    let host = &mut app.plugins;
    host.shared.images.borrow_mut().start(waker.clone());
    host.waker = Some(waker);
    if host.boot.is_started() || crate::flag("--no-plugins") {
        return;
    }
    stores::begin();
    // Non-realtime headless runs (`--script`/`--snap`/`--tree` with `--harness-writes`) have no
    // `on_ready` hook to call `storage::presented()`, so join stage B here rather than stranding
    // `start_later` in `Phase::Booted` until process exit when SQLite WAL init is still running.
    if crate::headless() && !crate::realtime() {
        crate::storage::join_for_headless();
    }
    // What the user turned off and trusted: in memory, or in the boot record. A launch whose
    // plugin files weren't imported yet knows them only from stage B, so nothing starts before it.
    let known = refresh_records(app) || (crate::storage::need() && refresh_records(app));
    // Plugins start processes and compare against the app's own server's version.
    process::warm();
    servers::read_own_version(app);
    // The servers added by address are in the registry from launch, their health checked.
    crate::servers::refresh(app);
    // The built-ins this launch's first screen shows start now, the rest after its first frame
    // (`first_screen`); a headless run (no first-frame hook) starts them all.
    let builtins = sources::builtins();
    app.plugins.dormant.ensure_read();
    let first = (!crate::headless()).then(|| {
        first_screen::plugins(app, |id| {
            let found = builtins.iter().find(|f| f.manifest.id == id)?;
            let hash = sources::files_hash(id, &found.origin, found.tier)?;
            app.plugins.dormant.kept(id, &hash).cloned()
        })
    });
    // A theme is in the picker and paints the window from the moment it is published, whichever
    // route the first screen draws (`first_screen`): its static contributors start with the first
    // screen's.
    let (now, later): (Vec<_>, Vec<_>) = builtins.into_iter().partition(|f| {
        contributes_theme(&f.manifest.id, None)
            || first.as_ref().is_none_or(|ids| ids.contains(&f.manifest.id.as_str()))
    });
    let start_now = known && stores::answers();
    // The ones with a record of this build go to sleep with it just after the first frame
    // (`doze_owed`): nothing they contribute draws in it (`first_screen`).
    let doze = if known && first.is_some() { Doze::Owed } else { Doze::Done };
    let (adopt_now, later) = if start_now {
        (now, later)
    } else {
        let mut queued = now;
        queued.extend(later);
        (Vec::new(), queued)
    };
    app.plugins.boot.step(BootEvent::Start { later, doze });
    if !adopt_now.is_empty() {
        adopt(app, adopt_now);
    }
    // A side panel open on New tab lists the sleeping built-ins' entries from their records
    // (`first_screen`): those go to sleep before this frame, not after it.
    if app.active_session_id().is_some_and(|session| dormant::shows_launchers(app, session)) {
        doze_owed(app);
    }
    // A headless run (a capture, a check) reads the plugin folders before it draws; a window
    // reads them on a worker after its first frame ([`first_frame`]).
    if crate::headless() {
        let found = sources::folders();
        if start_now {
            adopt(app, found);
        } else {
            app.plugins.boot.extend_later(found);
        }
        folders_read(app);
        app.plugins.boot.step(BootEvent::FoldersLoaded);
        // It has no first-frame hook either: what waits for the storage's records (a scratch
        // profile the harness writes to has none yet) starts once they're read.
        if !start_now {
            start_later(app);
        }
    }
}

/// Takes the plugins' enabled overrides and trust from the storage (memory or the boot record);
/// false when neither knows them yet.
fn refresh_records(app: &mut App) -> bool {
    let Some((overrides, trusted)) = crate::storage::plugin_records() else { return false };
    app.plugins.disabled = overrides.into_iter().filter(|(_, on)| !on).map(|(id, _)| id).collect();
    app.plugins.trusted = trusted;
    true
}

pub(super) type SaveRecordCallback = Box<dyn FnOnce(&mut App, &mut Tx, Result<(), String>) + Send>;

/// Writes plugin `id`'s record (its off switch and the files it's trusted for) as the window holds
/// it, in one durable commit; `then` gets how it went, on this window's thread.
pub(super) fn save_record(app: &App, id: &str, grant: Option<(String, SaveRecordCallback)>) {
    let (record, then) = match grant {
        Some((hash, then)) => (opencode_storage::PluginRecord {
            enabled: None,
            trusted: Some(hash),
        }, Some(then)),
        None => (opencode_storage::PluginRecord {
        enabled: app.plugins.disabled.contains(id).then_some(false),
        trusted: app.plugins.trusted.get(id).cloned(),
        }, None),
    };
    let (deliver, who) = (app.deliverer(), id.to_owned());
    let line = format!(
        "{} enabled={} trusted={}",
        who,
        record.enabled.is_none(),
        record.trusted.as_deref().map_or("none", |h| &h[..h.len().min(12)])
    );
    eprintln!("plugins: record saving: {line}");
    crate::storage::plugin_record(
        id,
        record,
        Box::new(move |result| {
            let result = result.map_err(|e| e.0);
            // Proof runs kill the app around this line: it says the record is on disk.
            match &result {
                Ok(()) => eprintln!("plugins: record saved: {line}"),
                Err(e) => eprintln!("plugins: record not saved: {line}: {e}"),
            }
            if let Some(then) = then {
                deliver(Box::new(move |app: &mut App, tx: &mut Tx| then(app, tx, result)));
            }
        }),
    );
}

/// Starts the plugins waiting for their values: one read for all of them on the storage thread,
/// then one per turn ([`start_one_later`]).
fn start_later(app: &mut App) {
    let ids: Vec<String> = app
        .plugins
        .list
        .iter()
        .map(|p| p.manifest.id.clone())
        .chain(app.plugins.boot.later().iter().map(|f| f.manifest.id.clone()))
        .collect();
    stores::load(app, ids, true, |app, tx| {
        // Stage B is in: what the record couldn't say about overrides and trust, it does now.
        refresh_records(app);
        // Appearance lists the themes this window has live, so a queued theme contributor starts
        // with the values rather than in its turn after the first frame.
        adopt_later_kinds(app, &["Theme"]);
        settle_later(app);
        if app.kernel.slots.get(&crate::switcher::SWITCHER).is_open() {
            adopt_later_kinds(app, &["Command"]);
        }
        if app.settings_open() {
            dormant::wake_settings(app);
        }
        if let Some(sid) = app.active_session_id().map(str::to_owned) {
            let tab_plugins: Vec<String> = app
                .kernel
                .slots
                .get(&crate::panels::SIDE)
                .sessions
                .get(&sid)
                .map(|st| {
                    st.tab_state
                        .tabs
                        .all
                        .iter()
                        .filter_map(|k| crate::panels::tabs::split_panel_key(k).map(|(p, _)| p.to_owned()))
                        .collect()
                })
                .unwrap_or_default();
            for id in tab_plugins {
                adopt_later_id(app, &id);
                dormant::wake(app, &id, dormant::Why::Use);
            }
        }
        if let Some(kind) = app.composer_ext.pickers.open() {
            crate::plugin_host::notify_picker_after_boot(app, kind, tx);
        }
        #[cfg(target_arch = "wasm32")]
        {
            next_later(app, PAGE_LATER_AFTER);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            start_one_later(app, tx);
        }
    });
}

/// The built-ins the first screen doesn't draw: each with a record of this build of it in this
/// language sleeps until its first use (`dormant`), from before the first frame when the boot record
/// knows the plugins, else once stage B does; the rest start one per turn after the first frame and
/// record themselves. `--plugins-eager` starts them all, as before. A page does the same: its
/// records are in the boot record its storage reads before the first frame.
fn settle_later(app: &mut App) {
    if crate::flag("--plugins-eager") {
        return;
    }
    for found in app.plugins.boot.take_later() {
        let hash = (found.tier == Tier::Bundled)
            .then(|| sources::files_hash(&found.manifest.id, &found.origin, found.tier))
            .flatten();
        match hash.and_then(|h| app.plugins.dormant.record(&found.manifest.id, &h)) {
            Some(record) => adopt_one(app, found, Some(record)),
            None => app.plugins.boot.push_later(found),
        }
    }
}

/// Whether `id`, or the record saved for it (`record`), contributes a theme: Appearance lists it
/// and the palette paints it, so it starts at boot and stays live instead of sleeping with its
/// entries listed from the record (a recorded theme is not a live one).
pub(super) fn contributes_theme(id: &str, record: Option<&dormant::Record>) -> bool {
    sources::contributions(id).contains(&"Theme")
        || record.is_some_and(|r| r.window.get("themes").is_some_and(|es| !es.is_empty()))
}

/// Adopts, now, any queued plugins still in `later` whose static contribution kinds intersect
/// `kinds` (or non-bundled plugins, whose contributions aren't pre-scanned), so opening a surface
/// like the command palette, slash menu, Settings, or panel launchers on a first launch before
/// `start_one_later` reaches them lists their entries at once.
pub(in crate::plugin_host) fn adopt_later_kinds(app: &mut App, kinds: &[&str]) -> bool {
    if app.plugins.boot.later().is_empty() {
        return false;
    }
    if !refresh_records(app) && !(crate::storage::need() && refresh_records(app)) {
        return false;
    }
    let later = app.plugins.boot.take_later();
    let mut now = Vec::new();
    let mut keep = Vec::with_capacity(later.len());
    for found in later {
        let contrib_kinds = sources::contributions(&found.manifest.id);
        if found.tier != Tier::Bundled || kinds.iter().any(|k| contrib_kinds.contains(k)) {
            now.push(found);
        } else {
            keep.push(found);
        }
    }
    app.plugins.boot.set_later(keep);
    let any = !now.is_empty();
    if any {
        adopt(app, now);
    }
    any
}

/// Adopts, now, any queued plugins still in `later` that connect to watched host signal `signal`
/// (or non-bundled plugins, whose signals aren't pre-scanned), so opening a host surface before
/// `start_one_later` reaches them delivers the event in that same turn.
pub(in crate::plugin_host) fn adopt_later_signal(app: &mut App, signal: &str) -> bool {
    if app.plugins.boot.later().is_empty() {
        return false;
    }
    if !refresh_records(app) && !(crate::storage::need() && refresh_records(app)) {
        return false;
    }
    let later = app.plugins.boot.take_later();
    let mut now = Vec::new();
    let mut keep = Vec::with_capacity(later.len());
    for found in later {
        if found.tier != Tier::Bundled || sources::signals(&found.manifest.id).contains(&signal) {
            now.push(found);
        } else {
            keep.push(found);
        }
    }
    app.plugins.boot.set_later(keep);
    let any = !now.is_empty();
    if any {
        adopt(app, now);
    }
    any
}

/// Ensures plugin `id` is running now when it is an eligible dormant or deferred plugin (never
/// overriding user disable, trust refusal, or `--no-plugins`). Returns `true` if `id` is running.
pub fn ensure_plugin_running(app: &mut App, id: &str) -> bool {
    if app.plugins.find(id).is_some_and(|i| app.plugins.list[i].running()) {
        return true;
    }
    if !app.plugins.boot.is_started() {
        return false;
    }
    doze_owed(app);
    if let Some(i) = app.plugins.find(id) {
        if app.plugins.list[i].running() {
            return true;
        }
        return matches!(app.plugins.list[i].life, Life::Dormant)
            && dormant::wake(app, id, dormant::Why::Use);
    }
    adopt_later_id(app, id)
        && app.plugins.find(id).is_some_and(|i| app.plugins.list[i].running())
}

/// Adopts queued plugin `id` from `later` immediately when its enabled/trust records are known.
pub(in crate::plugin_host) fn adopt_later_id(app: &mut App, id: &str) -> bool {
    if !app.plugins.boot.later().iter().any(|f| f.manifest.id == id) {
        return false;
    }
    if !refresh_records(app) && !(crate::storage::need() && refresh_records(app)) {
        return false;
    }
    let later = app.plugins.boot.take_later();
    let mut now = Vec::new();
    let mut keep = Vec::with_capacity(later.len());
    for found in later {
        if found.manifest.id == id {
            now.push(found);
        } else {
            keep.push(found);
        }
    }
    app.plugins.boot.set_later(keep);
    if now.is_empty() {
        return false;
    }
    adopt(app, now);
    true
}

/// Whether built-in `id` has work of its own to do from launch, without a use: the updater's check,
/// pill and What's new, and Pairing while Keep screen active holds the display. Those start in idle
/// time rather than waiting for a use.
pub(super) fn background(id: &str) -> bool {
    match id {
        "updater" => true,
        "pairing" => stores::text("pairing", "plugin.pairing", "keepScreenActive").is_some_and(|v| &*v == "true"),
        _ => false,
    }
}

/// In a page, how long after the first frame its other built-ins start: a key typed in that time
/// isn't held behind their Luau start-up on the page's one thread (about 200 ms in all).
#[cfg(target_arch = "wasm32")]
const PAGE_LATER_AFTER: std::time::Duration = std::time::Duration::from_millis(400);

/// In a page, how long without input before the next built-in starts (`start_one_later`).
#[cfg(target_arch = "wasm32")]
const PAGE_IDLE: std::time::Duration = std::time::Duration::from_secs(1);

/// In a page, the next built-in starts from a timer task of its own: the page takes the input that
/// came meanwhile first, where the next turn (`App::later`) runs before it.
#[cfg(target_arch = "wasm32")]
fn next_later(app: &App, after: std::time::Duration) {
    let deliver = app.deliverer();
    let _ = rune::task::spawn_after("plugin-later", after, move || deliver(Box::new(start_one_later)));
}

/// The built-ins the first screen doesn't draw that have a record of this build go to sleep with
/// it, so their commands, keys and menus are listed: once, after the first frame is presented,
/// at its after-frame turn (`launch::ready`) or at the first key if that comes first.
pub fn doze_owed(app: &mut App) {
    if !matches!(app.plugins.boot, Boot::Started { doze: Doze::Owed, .. }) {
        return;
    }
    if !app.plugins.boot.step(BootEvent::Doze).contains(&BootEffect::Doze) {
        return;
    }
    rune::marks::mark("plugins.doze");
    settle_later(app);
    if app.settings_open() {
        dormant::wake_settings(app);
    }
    rune::marks::mark("plugins.dozed");
}

/// The window's first frame is up: the built-ins it didn't show start one per turn from the next
/// one, so the first key isn't held behind all of them, and its plugin folders are read now, off
/// the app's thread. True when some start (the window redraws for them).
pub fn first_frame(app: &mut App) -> bool {
    doze_owed(app);
    // What the routed session's screen draws that slept through the first frame (a composer
    // pill, `first_screen`) starts from the next turn, one per turn.
    if app.active_session_id().is_some() && dormant::want_pills(app)
        && let Some(wake) = &*app.waker
    {
        wake();
    }
    let later = !app.plugins.boot.later().is_empty();
    let effects = app.plugins.boot.step(BootEvent::FirstFrame);
    for effect in effects {
        match effect {
            BootEffect::Doze => {}
            BootEffect::StartLater => start_later(app),
            BootEffect::ReadFolders => spawn_read_folders(app),
        }
    }
    later
}

/// Starts the next built-in the first screen didn't show, and queues the one after for the next turn.
fn start_one_later(app: &mut App, tx: &mut Tx) {
    if app.plugins.boot.later().is_empty() {
        return;
    }
    // In a page, a start waits for a second without input: Safari's engine takes 200-650 ms to
    // start a built-in (Chromium about 17), so one started between keys holds the next key that
    // long. Typing pushes them back; the first pause starts them, one per timer task.
    #[cfg(target_arch = "wasm32")]
    if let Some(wait) = rune::last_input()
        .map(|at| PAGE_IDLE.saturating_sub(at.elapsed()))
        .filter(|w| !w.is_zero())
    {
        next_later(app, wait);
        return;
    }
    // The user's input waiting for this frame goes first: the start waits a turn, so the
    // input's frame comes without it.
    if tx.input_waiting() {
        app.later(Box::new(start_one_later));
        return;
    }
    // In pieces, one a turn: its app half first, then the rest, so the input that comes meanwhile
    // waits for one piece.
    let Some(first) = app.plugins.boot.later().first() else { return };
    if let Some(entry) = first.manifest.app.clone()
        && first.error.is_none()
        && refusal(first.tier, &first.manifest).is_none()
        && !app.plugins.disabled.contains(&first.manifest.id)
        && !first.tier.needs_trust()
        && app.plugins.find(&first.manifest.id).is_none()
        && stores::answers_for(&first.manifest.id)
    {
        let files = first.origin.files();
        let revision = app_half::Revision::new(&first.manifest, first.tier, &files);
        if app_half::available(&revision) && !app_half::runs(&revision) {
            let catalogs = first.origin.catalogs(&files);
            let sources = sources::scripts(&first.origin, &files, &first.manifest.id, &entry);
            app.plugins.shared_plugin_owners.stage(&revision);
            app_half::ensure(app, revision, &files, catalogs, sources);
            app.later(Box::new(start_one_later));
            tx.instant();
            return;
        }
    }
    let Some(found) = app.plugins.boot.pop_front_later() else { return };
    adopt(app, vec![found]);
    if !app.plugins.boot.later().is_empty() {
        #[cfg(target_arch = "wasm32")]
        next_later(app, std::time::Duration::ZERO);
        #[cfg(not(target_arch = "wasm32"))]
        app.later(Box::new(start_one_later));
    }
    tx.instant();
}

fn spawn_read_folders(app: &mut App) {
    let deliver = app.deliverer();
    tasks::run(move || {
        let found = sources::folders();
        deliver(Box::new(move |app: &mut App, tx: &mut Tx| {
            adopt(app, found);
            folders_read(app);
            app.plugins.boot.step(BootEvent::FoldersLoaded);
            tx.instant();
        }));
    });
}

/// Takes plugins found in tier order (higher first) into the window and starts the ones that
/// may run. One from `--plugin-dir` or an override with a built-in's id replaces the built-in;
/// an installed one can't, and shows refused beside it.
fn adopt(app: &mut App, found: Vec<sources::Found>) {
    for found in found {
        adopt_one(app, found, None);
    }
}

/// [`adopt`] for one plugin; with `record`, one that may run sleeps until its first use instead.
fn adopt_one(app: &mut App, found: sources::Found, record: Option<dormant::Record>) {
    let consumed = app.plugins.shared_plugin_owners.staged_for(&found.manifest.id)
        .filter(|staged| *staged == app_half::Revision::new(&found.manifest, found.tier, &found.origin.files()));
    adopt_slot(app, found, record);
    if let Some(revision) = consumed {
        app.plugins.shared_plugin_owners.discard_staged(&revision);
    }
}

fn adopt_slot(app: &mut App, found: sources::Found, record: Option<dormant::Record>) {
    let id = found.manifest.id.clone();
    let shadows = shadows_builtin(found.tier, &id);
    let existing = app.plugins.find(&id);
    let replaces = existing.filter(|&i| app.plugins.list[i].tier > found.tier);
    if existing.is_some() && replaces.is_none() && !shadows {
        return;
    }
    let trust = Trust::at_launch(
        found.tier.needs_trust(),
        found.hash.as_deref().unwrap_or_default(),
        app.plugins.trusted.get(&id),
    );
    let first = if let Some(reason) = found.error.or_else(|| refusal(found.tier, &found.manifest)) {
        LifeEvent::Refuse(reason)
    } else if app.plugins.disabled.contains(&id) {
        LifeEvent::Disable
    } else if !trust.allows() {
        LifeEvent::NeedTrust
    } else {
        LifeEvent::Load
    };
    if matches!(first, LifeEvent::Load)
        && let Some(staged) = app.plugins.shared_plugin_owners.staged_for(&id)
        && staged != app_half::Revision::new(&found.manifest, found.tier, &found.origin.files())
    {
        app.plugins.shared_plugin_owners.discard_staged(&staged);
    }
    let plugin = Plugin {
        manifest: found.manifest,
        origin: found.origin,
        tier: found.tier,
        life: Life::Discovered,
        trust,
        hot: Hot::Idle,
        half: Half::new(&id, plugin::Kind::Window, found.tier),
    };
    let i = match replaces {
        Some(i) => {
            drive(app, i, LifeEvent::Disable);
            app.plugins.shared_plugin_owners.release(&app.plugins.list[i].half.token);
            app.plugins.list[i] = plugin;
            i
        }
        None => {
            app.plugins.list.push(plugin);
            app.plugins.list.len() - 1
        }
    };
    match (first, record) {
        // A theme contributor runs: asleep, its theme would leave the picker and the palette.
        (LifeEvent::Load, Some(record)) if !contributes_theme(&id, Some(&record)) => {
            dormant::doze(app, i, record, background(&id))
        }
        (first, _) => drive(app, i, first),
    }
}

/// Every plugin is in: save-to-reload watches their folders, and `--plugin-index` lists them.
fn folders_read(app: &mut App) {
    if crate::flag("--plugin-hot") || (cfg!(debug_assertions) && !crate::headless()) {
        let dirs = app
            .plugins
            .list
            .iter()
            .enumerate()
            .filter_map(|(i, p)| Some((i, p.origin.dir()?.to_owned())))
            .collect();
        app.plugins.watcher = hot::spawn(dirs, app.plugins.waker.clone());
    }
    if crate::flag("--plugin-index") {
        contrib::print_index(app);
    }
}

fn shadows_builtin(tier: Tier, id: &str) -> bool {
    tier == Tier::User && sources::builtin_ids().any(|builtin| builtin == id)
}

pub(super) fn refusal(tier: Tier, manifest: &Manifest) -> Option<String> {
    if manifest.id == "opencode" || shadows_builtin(tier, &manifest.id) {
        Some("A built-in plugin already has this id".to_owned())
    } else if !opencode_plugin_runtime::boot::runs_here(manifest, services::app::channel()) {
        Some("Not for this operating system".to_owned())
    } else {
        None
    }
}

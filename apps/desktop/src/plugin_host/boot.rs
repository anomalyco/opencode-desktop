//! Plugin discovery, staged startup (first screen before first frame, dormant records, and
//! one-per-turn deferred loading), plugin folder scanning, and tier adoption.

use opencode_plugin_spec::Manifest;
use rune::{Tx, Waker};

use super::{
    Half, Hot, Life, LifeEvent, Plugin, Tier, Trust, app_half, contrib, dormant, drive, first_screen, hot, plugin,
    process, servers, services, sources, stores, tasks,
};
use crate::App;

/// The plugin folders (`--plugin-dir`, overrides, installed plugins) of a window: not read yet,
/// being read on a worker, or in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Folders {
    #[default]
    Unread,
    Reading,
    Read,
}

/// Discovers and starts the plugins once the window's waker exists (`--no-plugins` skips it).
pub fn setup(app: &mut App, waker: Waker) {
    let host = &mut app.plugins;
    host.shared.images.borrow_mut().start(waker.clone());
    host.waker = Some(waker);
    if host.started || crate::flag("--no-plugins") {
        return;
    }
    host.started = true;
    stores::begin();
    // What the user turned off and trusted: in memory, or in the boot record. A launch whose
    // plugin files weren't imported yet knows them only from stage B, so nothing starts before it.
    let known = refresh_records(app) || (crate::storage::need() && refresh_records(app));
    // Plugins start processes and compare against the app's own server's version.
    process::warm();
    servers::read_own_version(app);
    // The servers added by address are in the registry from launch, their health checked.
    crate::servers::refresh(app);
    // The built-ins this launch's first screen shows start now, the rest after its first frame
    // (`first_screen`); a headless run and a hot build (no first-frame hook) start them all.
    let first = (!crate::headless() && !cfg!(feature = "hot")).then(|| first_screen::plugins(app));
    let (now, later): (Vec<_>, Vec<_>) = sources::builtins()
        .into_iter()
        .partition(|f| first.as_ref().is_none_or(|ids| ids.contains(&f.manifest.id.as_str())));
    let start_now = known && stores::answers();
    app.plugins.later = later;
    // The ones with a record of this build go to sleep with it just after the first frame
    // (`doze_owed`): nothing they contribute draws in it (`first_screen`).
    app.plugins.doze_owed = known && first.is_some();
    if start_now {
        adopt(app, now);
    } else {
        app.plugins.later.splice(0..0, now);
    }
    // A headless run (a capture, a check) reads the plugin folders before it draws; a window
    // reads them on a worker after its first frame ([`first_frame`]).
    if crate::headless() {
        let found = sources::folders();
        if start_now {
            adopt(app, found);
        } else {
            app.plugins.later.extend(found);
        }
        folders_read(app);
        app.plugins.folders = Folders::Read;
        // It has no first-frame hook either: what waits for the storage's records (a scratch
        // profile the harness writes to has none yet) starts once they're read.
        if !start_now {
            start_later(app);
        }
    } else if cfg!(feature = "hot") {
        // A hot build has no first-frame hook.
        read_folders(app);
        start_later(app);
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

/// Writes plugin `id`'s record (its off switch and the files it's trusted for) as the window holds
/// it, in one durable commit; `then` gets how it went, on this window's thread.
pub(super) fn save_record(
    app: &App,
    id: &str,
    then: Option<Box<dyn FnOnce(&mut App, &mut Tx, Result<(), String>) + Send>>,
) {
    let record = opencode_storage::PluginRecord {
        enabled: app.plugins.disabled.contains(id).then_some(false),
        trusted: app.plugins.trusted.get(id).cloned(),
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
        .chain(app.plugins.later.iter().map(|f| f.manifest.id.clone()))
        .collect();
    stores::load(app, ids, true, |app, tx| {
        // Stage B is in: what the record couldn't say about overrides and trust, it does now.
        refresh_records(app);
        #[cfg(target_arch = "wasm32")]
        {
            let _ = tx;
            settle_later(app);
            next_later(app, PAGE_LATER_AFTER);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            settle_later(app);
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
    for found in std::mem::take(&mut app.plugins.later) {
        let hash = (found.tier == Tier::Bundled)
            .then(|| sources::files_hash(&found.manifest.id, &found.origin, found.tier))
            .flatten();
        match hash.and_then(|h| app.plugins.dormant.record(&found.manifest.id, &h)) {
            Some(record) => adopt_one(app, found, Some(record)),
            None => app.plugins.later.push(found),
        }
    }
}

/// Adopts, now, any queued plugins still in `later` that register panels or menu items, so opening
/// New tab or the `+` menu before `start_one_later` reaches them lists their entries at once.
pub(super) fn adopt_later_launchers(app: &mut App) -> bool {
    let mut now = Vec::new();
    let mut keep = Vec::with_capacity(app.plugins.later.len());
    for found in std::mem::take(&mut app.plugins.later) {
        let kinds = sources::contributions(&found.manifest.id);
        if found.tier != Tier::Bundled || kinds.contains(&"Panel") || kinds.contains(&"MenuItem") {
            now.push(found);
        } else {
            keep.push(found);
        }
    }
    app.plugins.later = keep;
    let any = !now.is_empty();
    if any {
        adopt(app, now);
    }
    any
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
    if !std::mem::take(&mut app.plugins.doze_owed) {
        return;
    }
    rune::marks::mark("plugins.doze");
    settle_later(app);
    rune::marks::mark("plugins.dozed");
}

/// The window's first frame is up: the built-ins it didn't show start one per turn from the next
/// one, so the first key isn't held behind all of them, and its plugin folders are read now, off
/// the app's thread. True when some start (the window redraws for them).
pub fn first_frame(app: &mut App) -> bool {
    doze_owed(app);
    let later = !app.plugins.later.is_empty();
    if app.plugins.started {
        start_later(app);
    }
    if app.plugins.started && app.plugins.folders == Folders::Unread {
        read_folders(app);
    }
    later
}

/// Starts the next built-in the first screen didn't show, and queues the one after for the next turn.
fn start_one_later(app: &mut App, tx: &mut Tx) {
    if app.plugins.later.is_empty() {
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
    let first = &app.plugins.later[0];
    if let Some(entry) = first.manifest.app.clone()
        && !app_half::runs(&first.manifest.id)
        && stores::answers_for(&first.manifest.id)
    {
        let files = first.origin.files();
        let (manifest, catalogs) = (first.manifest.clone(), first.origin.catalogs(&files));
        let sources = sources::scripts(&first.origin, &files, &manifest.id, &entry);
        app_half::ensure(app, first.tier, &manifest, &files, catalogs, sources);
        app.later(Box::new(start_one_later));
        tx.instant();
        return;
    }
    let found = app.plugins.later.remove(0);
    adopt(app, vec![found]);
    if !app.plugins.later.is_empty() {
        #[cfg(target_arch = "wasm32")]
        next_later(app, std::time::Duration::ZERO);
        #[cfg(not(target_arch = "wasm32"))]
        app.later(Box::new(start_one_later));
    }
    tx.instant();
}

fn read_folders(app: &mut App) {
    app.plugins.folders = Folders::Reading;
    let deliver = app.deliverer();
    tasks::run(move || {
        let found = sources::folders();
        deliver(Box::new(move |app: &mut App, tx: &mut Tx| {
            adopt(app, found);
            folders_read(app);
            app.plugins.folders = Folders::Read;
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
    let builtins: Vec<&str> = sources::builtin_ids().collect();
    let id = found.manifest.id.clone();
    let shadows = found.tier == Tier::User && builtins.contains(&id.as_str());
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
    let first = if let Some(e) = found.error {
        LifeEvent::Refuse(e)
    } else if shadows || id == "opencode" {
        LifeEvent::Refuse("A built-in plugin already has this id".to_owned())
    } else if !runs_here(&found.manifest) {
        LifeEvent::Refuse("Not for this operating system".to_owned())
    } else if app.plugins.disabled.contains(&id) {
        LifeEvent::Disable
    } else if !trust.allows() {
        LifeEvent::NeedTrust
    } else {
        LifeEvent::Load
    };
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
            app_half::stop(&id);
            app.plugins.list[i] = plugin;
            i
        }
        None => {
            app.plugins.list.push(plugin);
            app.plugins.list.len() - 1
        }
    };
    match (first, record) {
        (LifeEvent::Load, Some(record)) => dormant::doze(app, i, record, background(&id)),
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

fn runs_here(m: &Manifest) -> bool {
    use opencode_plugin_spec::Platform;
    let here = if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Linux
    };
    (m.os.is_empty() || m.os.contains(&here)) && (m.channels.is_empty() || m.channels.contains(&services::app::channel()))
}

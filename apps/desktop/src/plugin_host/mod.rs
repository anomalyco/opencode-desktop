//! `PluginHost`: plugins as `--!strict` Luau source and a `plugin.toml` against the
//! engine-agnostic contract (`opencode-plugin-spec`), run on `rune-luau`, one VM per plugin half.
//!
//! - **Two hosts.** A window's [`PluginHost`] (`WindowPluginHost`) runs every plugin's window
//!   half, `init.luau`; the process's `AppPluginHost` ([`app_half`]) runs each `app.luau` once,
//!   polled once per tick whatever the number of windows.
//! - **Loading** ([`sources`]): the built-ins are compiled into the binary (a debug build runs
//!   them from the repository, so saving one reloads it); `--plugin-dir`, overrides and installed
//!   plugins come from their folders, and one from outside the app waits for a one-time OK
//!   ([`trust`]) keyed by its files' SHA-256. `require` follows Luau's rules (`@self`,
//!   `.luaurc`). Specs decode with `Data::decode` into the contract's Rust records.
//! - **Contributions** reach the app's surfaces ([`surfaces`], [`contrib`], [`panels`],
//!   [`commands`], [`view`]): panels with their own launchers, commands with keybinds and
//!   `/slash`, menus and the app menu, settings pages, link handlers, titlebar items, composer
//!   pills, code block actions, shell slots, file viewers, server sources ([`servers`]) and MCP
//!   tools.
//! - **Rendering** ([`surfaces`]): each contribution's render is memoized on the script's
//!   version, its props, the theme and a stamp of what it read last time ([`cx::Dep`]); an
//!   unchanged frame enters no VM.
//! - **Host-kept state** ([`shared`]): each plugin's stores, what the app knows that plugins ask
//!   about, and their images; processes ([`process`]) and background work ([`tasks`]) off the
//!   app's thread; icons and strings interned for Rune's `&'static` data ([`icons`]).
//! - **Idle**: `poll` checks each script's inbox and deadline and returns; nothing at rest
//!   enters a VM or asks for a frame.
//! - **Hot reload** ([`hot`]): saving a `.luau` file compiles it off the UI thread and swaps the
//!   VM at the next poll, keeping hot state, editors and stores.

pub mod app_half;
mod catalogs;
pub mod commands;
pub mod console;
mod contrib;
mod ctx;
pub mod cx;
pub(crate) mod dormant;
pub(crate) mod editors;
mod first_screen;
pub mod flyout;
mod hot;
pub(crate) mod icons;
mod images;
pub mod machines;
pub(crate) mod memory;
pub mod native;
mod nodes;
#[cfg(feature = "office")]
mod office_nodes;
pub mod panels;
mod plugin;
pub(crate) mod process;
pub mod project_request;
pub mod requests;
mod redraw;
mod reveal;
pub mod server_sources;
pub mod servers;
pub mod services;
mod shared;
pub mod sources;
pub mod stores;
pub mod surfaces;
pub(crate) mod tasks;
pub mod trust;
pub mod types;
mod view;
mod vm;
mod widgets;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::rc::Rc;

use opencode_plugin_spec::Manifest;
use rune::{KeyEvent, Tx, Waker};
use rune_luau::{Data, Script};

pub use commands::{commands, key, run_command, run_slash, slash_commands};
pub use contrib::{code_block_actions, mcp_call, mcp_tools, mcp_tools_generation, menubar_items, open_link, run_menu_item, run_menubar, server_menu_items, settings_search};
pub use catalogs::Catalogs;
pub use flyout::{Flyout, FlyoutEvent};
pub use machines::{Hot, HotEffect, Life, LifeEffect, LifeEvent, Trust, TrustEffect, TrustEvent};
pub use plugin::Half;
pub use reveal::reveal;
pub use servers::{ServerRec, all as contributed_servers, authenticating as server_authenticating, changed as servers_changed, connect as connect_server, cover as server_cover, row as server_row, stage as servers_stage};
pub use sources::{Origin, Tier};
pub use surfaces::{dialog_view, file_viewer, pill_views, popover_view, settings_page, settings_page_actions, settings_page_icon, settings_page_ids, settings_page_key, settings_page_title, settings_pages, sidebar_items, slot, slot_with, titlebar_items};
pub use trust::dialog as trust_dialog;
pub use view::settings_section;

use crate::App;

thread_local! {
    static EPOCH: Cell<u64> = const { Cell::new(0) };
}

/// Notes that something every plugin surface may show changed outside a render.
pub fn bump() {
    EPOCH.set(EPOCH.get().wrapping_add(1));
}

pub fn epoch() -> u64 {
    EPOCH.get()
}

/// What every plugin surface shows outside `Tracked`: the epoch, and each plugin's script and
/// half versions, as the surfaces' own memos key on them (`surfaces::render`). A region showing
/// plugin surfaces takes it as an input, so a plugin that changed what it draws between polls
/// builds that region again.
pub fn surfaces_version(app: &App) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    epoch().hash(&mut h);
    for p in app.plugins.list.iter() {
        p.half.surface_version(p.script()).hash(&mut h);
    }
    h.finish()
}
/// One plugin in this window.
pub struct Plugin {
    pub manifest: Manifest,
    pub origin: Origin,
    pub tier: Tier,
    pub life: Life,
    pub trust: Trust,
    pub hot: Hot,
    pub half: Rc<Half>,
}

impl Plugin {
    pub fn running(&self) -> bool {
        self.life.running()
    }

    /// The window half's script, while it has one.
    pub fn script(&self) -> Option<&Script> {
        self.life.script()
    }
}

/// What a plugin shows over the window: nothing, a popover it placed (with the list a
/// `ui.flyoutRow` in it opened beside it), or a composer pill's menu (under the host's heading,
/// above its pill). One at a time: opening one closes the other.
#[derive(Default)]
pub enum Overlay {
    #[default]
    Closed,
    Popover { plugin: Rc<str>, id: String, anchor: String, side: rune::Side, align: rune::Align, gap: f32, data: Data, flyout: Flyout, back: Option<String> },
    PillMenu { plugin: Rc<str>, id: String, anchor: String, data: Data },
}

impl Overlay {
    /// Whether plugin `plugin`'s popover `id` is open, on its own or as a row's list.
    pub fn popover_open(&self, plugin: &str, id: &str) -> bool {
        match self {
            Overlay::Popover { plugin: p, id: i, flyout, .. } if &**p == plugin => i == id || flyout.list().is_some_and(|l| l.id == id),
            _ => false,
        }
    }

    /// The row of plugin `plugin`'s open popover whose list shows beside it.
    pub fn flyout_row(&self, plugin: &str) -> Option<&str> {
        match self {
            Overlay::Popover { plugin: p, flyout, .. } if &**p == plugin => flyout.row(),
            _ => None,
        }
    }

    /// The list the open popover's flyout shows, whoever's popover it is.
    pub fn flyout_list(&self) -> Option<&flyout::Row> {
        match self {
            Overlay::Popover { flyout, .. } => flyout.list(),
            _ => None,
        }
    }

    /// The open popover's flyout waits on the pointer's aim.
    pub fn flyout_due(&self) -> bool {
        matches!(self, Overlay::Popover { flyout, .. } if flyout.due().is_some())
    }

    /// Whether plugin `plugin`'s pill `id` has its menu open.
    pub fn pill_open(&self, plugin: &str, id: &str) -> bool {
        matches!(self, Overlay::PillMenu { plugin: p, id: i, .. } if &**p == plugin && i == id)
    }

    pub fn is_open(&self) -> bool {
        !matches!(self, Overlay::Closed)
    }

    /// Closes what plugin `plugin` shows (it stopped).
    pub fn close_for(&mut self, plugin: &str) {
        if matches!(self, Overlay::Popover { plugin: p, .. } | Overlay::PillMenu { plugin: p, .. } if &**p == plugin) {
            *self = Overlay::Closed;
        }
    }
}
/// The plugin dialog on screen.
pub struct OpenDialog {
    pub plugin: Rc<str>,
    pub id: String,
    pub data: Data,
}

/// A plugin's numbers for `Diagnostics:Metrics` and the console.
pub struct PluginMetrics {
    pub memory_kb: f64,
    pub render_us: f64,
    pub memo_hit_rate: f64,
}

/// The window's plugin host (`WindowPluginHost`).
#[derive(Default)]
pub struct PluginHost {
    pub list: Vec<Plugin>,
    started: bool,
    /// The plugins turned off in Settings › Plugins.
    disabled: BTreeSet<String>,
    /// Plugins from outside the app the user OKed: plugin id → the SHA-256 of their files.
    trusted: BTreeMap<String, String>,
    shared: shared::Shared,
    /// Contributed servers whose health check is on its way.
    checking: HashSet<String>,
    /// The window's page visits, for server covers' one sign-in offer per visit.
    pub cover_visit: servers::CoverVisit,
    /// Where reading the plugin folders stands.
    folders: Folders,
    /// Built-ins this launch's first screen doesn't show, started after its first frame.
    later: Vec<sources::Found>,
    /// The boot record knows the built-ins' records, and those of `later` that have one haven't
    /// gone to sleep yet (`doze_owed`).
    doze_owed: bool,
    /// The built-ins waiting for their first use, and their records.
    pub dormant: dormant::Dormant,
    pub overlay: Overlay,
    pub dialog: Option<OpenDialog>,
    /// The titlebar item the pointer is over, whose pill opens to show its label.
    pub titlebar_hover: Option<&'static str>,
    /// The window lost the system's focus (`ctx.FocusChanged`); a new window has it.
    pub window_blurred: Cell<bool>,
    pub editors: editors::Editors,
    /// The window's waker, for process output and finished work.
    pub waker: Option<rune::Waker>,
    /// The appearance the last surface rendered in.
    pub dark: Cell<bool>,
    /// The pointer's heading over a popover's flyout rows, and where their open list is.
    pub aim: flyout::Aim,
    /// The debug bar's FOCUS: every control draws its keyboard focus ring.
    pub force_focus: bool,
    /// A UI entry without a transaction asked for something: the next poll carries it out.
    pub asked: std::cell::Cell<bool>,
    /// Server events plugins listen for, waiting for the next poll.
    events: RefCell<Vec<(String, serde_json::Value)>>,
    watcher: Option<hot::Watcher>,
    /// Surface renders that ran a script, and those a memo answered, for the metrics.
    pub renders: Cell<u64>,
    pub hits: Cell<u64>,
    /// A plugin's `Layout:OpenProject` waiting for its server, and the last request's attempt.
    pub project: project_request::State,
    pub project_attempt: rune::machine::Attempt,
    /// The poll clock of the last poll.
    clock: Cell<f64>,
    /// The epoch the registries read off the app's state were last mirrored at.
    synced: Cell<u64>,
    /// `--luau-metrics`: each plugin's (VM calls, renders) and the surfaces' (hits, misses) at the mark.
    baseline: RefCell<Vec<(String, (u64, u64))>>,
    mark_hits: Cell<(u64, u64)>,
}

impl PluginHost {
    /// Whether a plugin dialog is open.
    pub fn dialog_open(&self) -> bool {
        self.dialog.is_some()
    }

    pub fn half(&self, id: &str) -> Option<&Rc<Half>> {
        self.list.iter().find(|p| &*p.half.id == id).map(|p| &p.half)
    }

    pub fn find(&self, id: &str) -> Option<usize> {
        self.list.iter().position(|p| p.manifest.id == id)
    }

    /// The running plugins.
    pub fn running(&self) -> impl Iterator<Item = (usize, &Plugin)> {
        self.list.iter().enumerate().filter(|(_, p)| p.running())
    }

    /// The plugins whose contributions are listed: the running ones, and the dormant ones from their
    /// records (`dormant`), whose functions are none until they start.
    pub fn listed(&self) -> impl Iterator<Item = (usize, &Plugin)> {
        self.list.iter().enumerate().filter(|(_, p)| p.life.listed())
    }

    pub fn metrics(&self, id: &str) -> Option<PluginMetrics> {
        let script = self.list.iter().find(|p| p.manifest.id == id)?.script()?;
        let m = script.metrics();
        let calls = m.renders.max(1) as f64;
        Some(PluginMetrics {
            memory_kb: script.memory() as f64 / 1024.0,
            render_us: m.vm_time.as_secs_f64() * 1e6 / m.vm_calls.max(1) as f64,
            memo_hit_rate: self.hits.get() as f64 / (self.hits.get() as f64 + calls),
        })
    }
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
    let (now, later): (Vec<_>, Vec<_>) = sources::builtins().into_iter().partition(|f| first.as_ref().is_none_or(|ids| ids.contains(&f.manifest.id.as_str())));
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
fn save_record(app: &App, id: &str, then: Option<Box<dyn FnOnce(&mut App, &mut Tx, Result<(), String>) + Send>>) {
    let record = opencode_storage::PluginRecord { enabled: app.plugins.disabled.contains(id).then_some(false), trusted: app.plugins.trusted.get(id).cloned() };
    let (deliver, who) = (app.deliverer(), id.to_owned());
    let line = format!("{} enabled={} trusted={}", who, record.enabled.is_none(), record.trusted.as_deref().map_or("none", |h| &h[..h.len().min(12)]));
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
    let ids: Vec<String> = app.plugins.list.iter().map(|p| p.manifest.id.clone()).chain(app.plugins.later.iter().map(|f| f.manifest.id.clone())).collect();
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
        let hash = (found.tier == Tier::Bundled).then(|| sources::files_hash(&found.manifest.id, &found.origin, found.tier)).flatten();
        match hash.and_then(|h| app.plugins.dormant.record(&found.manifest.id, &h)) {
            Some(record) => adopt_one(app, found, Some(record)),
            None => app.plugins.later.push(found),
        }
    }
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

/// The plugin folders (`--plugin-dir`, overrides, installed plugins) of a window: not read yet,
/// being read on a worker, or in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Folders {
    #[default]
    Unread,
    Reading,
    Read,
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
    if let Some(wait) = rune::last_input().map(|at| PAGE_IDLE.saturating_sub(at.elapsed())).filter(|w| !w.is_zero()) {
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
        let sources = first.origin.scripts(&files, &manifest.id, &entry);
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
    let trust = Trust::at_launch(found.tier.needs_trust(), found.hash.as_deref().unwrap_or_default(), app.plugins.trusted.get(&id));
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
    let plugin = Plugin { manifest: found.manifest, origin: found.origin, tier: found.tier, life: Life::Discovered, trust, hot: Hot::Idle, half: Half::new(&id, plugin::Kind::Window, found.tier) };
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
        let dirs = app.plugins.list.iter().enumerate().filter_map(|(i, p)| Some((i, p.origin.dir()?.to_owned()))).collect();
        app.plugins.watcher = hot::Watcher::spawn(dirs, app.plugins.waker.clone());
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
    if !stores::answers_for(&app.plugins.list[i].manifest.id) {
        let id = app.plugins.list[i].manifest.id.clone();
        stores::load(app, vec![id.clone()], false, move |app, tx| {
            if let Some(i) = app.plugins.find(&id).filter(|&i| matches!(app.plugins.list[i].life, Life::Loading)) {
                start_loaded(app, i);
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
    let sources = p.origin.scripts(&files, &manifest.id, &entry);
    app_half::ensure(app, p.tier, &manifest, &files, catalogs, sources);
    true
}

/// [`start`] once plugin `i`'s values are in memory.
fn start_loaded(app: &mut App, i: usize) {
    // Keep screen active was the app's own setting before the Pairing plugin took the page: it
    // moves into the plugin's store once.
    if app.plugins.list[i].manifest.id == "pairing" && app.settings.keep_screen_active && stores::text("pairing", "plugin.pairing", "keepScreenActive").is_none() {
        stores::set("plugin.pairing", "keepScreenActive", "true".to_owned());
    }
    let files = app.plugins.list[i].origin.files();
    let p = &app.plugins.list[i];
    let (id, manifest) = (p.manifest.id.clone(), p.manifest.clone());
    let catalogs = p.origin.catalogs(&files);
    *p.half.catalogs.borrow_mut() = catalogs.clone();
    sources::intern_icons(&id, &files);
    if let Some(entry) = &manifest.app {
        app_half::ensure(app, p.tier, &manifest, &files, catalogs, p.origin.scripts(&files, &manifest.id, entry));
    }
    let folder = p.origin.dir().and_then(|d| d.file_name()).map_or_else(|| id.clone(), |n| n.to_string_lossy().into_owned());
    let sources = p.origin.scripts(&files, &folder, &manifest.entry);
    let options = vm::options(p.half.clone());
    let script = cx::with_app(app, || Script::with_options(&id, sources, options));
    if let Some(w) = app.plugins.waker.clone() {
        script.set_waker(w);
    }
    let half = app.plugins.list[i].half.clone();
    let vm = script.vm_id();
    let adopted = half.adopt(vm);
    services::reap(&half, vm);
    services::store::withdraw_owner(&id, plugin::Kind::Window, vm);
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
            cx::with_app(app, || script.spawn_fn(&fire, signal, None).ok());
        }
    }
    drop(script);
    *half.contribs.borrow_mut() = plugin::Contribs::default();
    services::reap(&half, None);
    services::store::withdraw_owner(&half.id, plugin::Kind::Window, None);
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
    let result = cx::with_app(app, || script.reload_compiled(prepared.sources, prepared.compiled, prepared.compile));
    let vm = script.vm_id();
    let id = app.plugins.list[i].manifest.id.clone();
    let outcome = match result {
        Ok(report) => {
            half.adopt(vm);
            services::reap(&half, vm);
            services::store::withdraw_owner(&id, plugin::Kind::Window, vm);
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

/// Queues a server event for the plugins listening for its type, delivered on the next poll.
pub fn server_event(app: &mut App, ty: &str, data: &serde_json::Value) {
    if dormant::listens(app, ty) {
        dormant::buffer_event(app, ty, data);
    }
    let host = &app.plugins;
    let wanted = host.running().any(|(_, p)| p.half.events.borrow().iter().any(|(_, prefix, _)| ty.starts_with(prefix.as_str())));
    if wanted {
        host.events.borrow_mut().push((ty.to_owned(), data.clone()));
    }
}

/// Every plugin field gives up the keyboard (the side region switched tabs).
pub fn blur_fields(app: &mut App) {
    for edit in app.plugins.editors.get_mut().values_mut() {
        edit.focused = false;
    }
}

/// Whether a plugin's field has the keyboard.
pub fn field_focused(app: &App) -> bool {
    editors::any_focused(app)
}

/// A key a plugin's field hands over (`UiFieldProps.keys`): its editor's `KeyPressed` fires with
/// the key's name.
pub fn key_pressed(app: &mut App, plugin: &str, index: usize, key: &str, tx: &mut Tx) {
    if let Some(p) = app.plugins.list.iter().find(|p| p.manifest.id == plugin) {
        services::fire(app, p, &format!("editor.{index}.KeyPressed"), |lua| mlua::IntoLuaMulti::into_lua_multi(key.to_owned(), lua));
    }
    requests::carry_out(app, tx);
}

/// Enter in a plugin's field: its editor's `Submitted` fires with the text.
pub fn submitted(app: &mut App, plugin: &str, index: usize, tx: &mut Tx) {
    let text = editors::text_in(app, plugin, index);
    if let Some(p) = app.plugins.list.iter().find(|p| p.manifest.id == plugin) {
        services::fire(app, p, &format!("editor.{index}.Submitted"), |lua| mlua::IntoLuaMulti::into_lua_multi(text.clone(), lua));
    }
    requests::carry_out(app, tx);
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
    if app.plugins.later.is_empty() {
        dormant::save(app);
    }
    changed |= app_half::poll(app, now, tx);
    #[cfg(feature = "alloc-sites")]
    memory::publish_vms(app);
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
                        cx::with_app(app, || script.prewarm());
                    }
                }
                HotEffect::Swap(prepared) => reload(app, i, Some(prepared)),
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
        let caught = if app.plugins.dormant.catch_up.contains_key(&*half.id) { app.plugins.dormant.catch_up.remove(&*half.id).unwrap_or_default() } else { Vec::new() };
        for (ty, data) in caught.iter().chain(&events) {
            let listeners: Vec<mlua::Value> = half.events.borrow().iter().filter(|(v, prefix, _)| *v == vm && ty.starts_with(prefix.as_str())).map(|(_, _, s)| s.clone()).collect();
            for signal in listeners {
                let event = services::map([("type", Data::String(ty.clone())), ("data", services::json_to_data(data)), ("serverId", Data::String(app.server_key().unwrap_or_else(|| "local".into())))]);
                ran |= fire_value(app, i, signal, Data::List(vec![event]));
            }
        }
        if fresh || changed || delivered || ran {
            ran |= services::fire_watched(app, i, vm);
        }
        let p = &app.plugins.list[i];
        let Some(script) = p.script() else { continue };
        ran |= cx::with_app(app, || script.poll(now, tx));
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
    cx::with_app(app, || {
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
/// `--plugin-index`: each plugin, where it runs from, its state, and what it contributed.
pub fn print_index(app: &App) {
    for p in &app.plugins.list {
        eprintln!("plugin index: plugin: {} {} (contract {}, {}, {}) from {}", p.manifest.id, p.manifest.version, p.manifest.contract, p.tier.label(), p.life.label(), p.origin.describe());
        if let Some(e) = p.life.why() {
            eprintln!("plugin index: error: {}: {e}", p.manifest.id);
        }
        let c = p.half.contribs.borrow();
        for x in &c.panels {
            eprintln!("plugin index: panel: {} {}{}", x.key, x.spec.title, if x.spec.launcher == Some(false) { ", no launcher" } else { "" });
        }
        for x in &c.commands {
            eprintln!("plugin index: command: {} {} [{}] section {:?}{}", x.key, x.spec.title, x.spec.bind.as_deref().unwrap_or_default(), x.spec.section, if x.spec.hidden == Some(true) { ", hidden" } else { "" });
        }
        for x in &c.slots {
            eprintln!("plugin index: slot: {} {} (order {})", x.key, x.spec.at, x.spec.order.unwrap_or(0));
        }
        for x in &c.popovers {
            eprintln!("plugin index: popover: {}", x.key);
        }
        for x in &c.settings {
            eprintln!("plugin index: setting: {} {}", x.key, x.spec.title);
        }
    }
}

/// `--luau-metrics`: as the window goes, each plugin's VM entries, renders, memo hits, rows,
/// time in the VM and heap, in all and since the first poll at or after `--luau-metrics-from
/// <seconds>` (the idle window a proof measures).
impl Drop for PluginHost {
    fn drop(&mut self) {
        dormant::close(self);
        if !crate::flag("--luau-metrics") {
            return;
        }
        for p in &self.list {
            let Some(s) = p.script() else { continue };
            let m = s.metrics();
            let base = self.baseline.borrow().iter().find(|(id, _)| *id == p.manifest.id).map(|(_, b)| *b);
            let (calls, renders) = base.map_or((m.vm_calls, m.renders), |b| (m.vm_calls - b.0, m.renders - b.1));
            eprintln!(
                "luau metrics: {} vm_calls {} renders {} rows {} vm_time_us {:.0} heap_kb {:.0} reloads {} | since mark: vm_calls {calls} renders {renders}",
                p.manifest.id,
                m.vm_calls,
                m.renders,
                m.rows,
                m.vm_time.as_secs_f64() * 1e6,
                s.memory() as f64 / 1024.0,
                m.reloads,
            );
        }
        let (hits, misses) = (self.hits.get(), self.renders.get());
        let mark = self.mark_hits.get();
        eprintln!("luau metrics: surfaces memo hits {hits} misses {misses} | since mark: hits {} misses {}", hits - mark.0, misses - mark.1);
    }
}

/// Notes the counters at the mark `--luau-metrics-from` names, once.
fn mark(host: &PluginHost, now: f64) {
    if !host.baseline.borrow().is_empty() || host.mark_hits.get() != (0, 0) {
        return;
    }
    let Some(from) = crate::arg("--luau-metrics-from").and_then(|s| s.parse::<f64>().ok()) else { return };
    if now < from {
        return;
    }
    let base = host.list.iter().filter_map(|p| Some((p.manifest.id.clone(), p.script().map(|s| (s.metrics().vm_calls, s.metrics().renders))?))).collect();
    *host.baseline.borrow_mut() = base;
    host.mark_hits.set((host.hits.get(), host.renders.get()));
    eprintln!("luau metrics: mark at {now:.2} s");
}

/// Esc closes the open plugin popover.
pub fn popover_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key != rune::Key::Escape || !app.plugins.overlay.is_open() {
        return false;
    }
    requests::close_popover(app, tx);
    true
}

/// Esc closes the open plugin dialog, telling its plugin.
pub fn dialog_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key != rune::Key::Escape || app.plugins.dialog.is_none() {
        return false;
    }
    requests::dismiss_dialog(app, tx);
    true
}

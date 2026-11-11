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
pub mod commands;
pub mod console;
mod contrib;
mod ctx;
pub mod cx;
mod editors;
pub mod flyout;
mod hot;
pub(crate) mod icons;
mod images;
pub mod machines;
mod memory;
pub mod native;
mod nodes;
#[cfg(feature = "office")]
mod office_nodes;
pub mod panels;
mod plugin;
pub(crate) mod process;
pub mod project_request;
pub mod requests;
mod reveal;
pub mod server_sources;
pub mod servers;
pub mod services;
mod shared;
pub mod sources;
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
pub use flyout::{Flyout, FlyoutEvent};
pub use machines::{Hot, HotEffect, Life, LifeEffect, LifeEvent, Trust, TrustEffect, TrustEvent};
pub use plugin::Half;
pub use reveal::reveal;
pub use servers::{ServerRec, all as contributed_servers, authenticating as server_authenticating, changed as servers_changed, connect as connect_server, cover as server_cover, row as server_row, stage as servers_stage};
pub use sources::{Origin, Tier};
pub use surfaces::{dialog_view, file_viewer, pill_views, popover_view, settings_page, settings_page_actions, settings_page_icon, settings_page_ids, settings_page_key, settings_page_title, settings_pages, sidebar_items, slot, slot_with, titlebar_items};
pub use trust::{dialog as trust_dialog, save_state};
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
    Popover { plugin: Rc<str>, id: String, anchor: String, side: rune::Side, align: rune::Align, gap: f32, data: Data, flyout: Flyout },
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
    pub asked: bool,
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
    let persists = app.persists();
    let keep_screen = app.settings.keep_screen_active;
    let host = &mut app.plugins;
    host.shared.images.borrow_mut().start(waker.clone());
    host.waker = Some(waker);
    if host.started || crate::flag("--no-plugins") {
        return;
    }
    host.started = true;
    host.shared.stores.persist = persists;
    // Keep screen active was the app's own setting before the Pairing plugin took the page: it
    // moves into the plugin's store once.
    if keep_screen && persists {
        if host.shared.stores.text("pairing", "keepScreenActive", "").is_none() {
            host.shared.stores.set("pairing", "keepScreenActive", "", "true");
        }
    }
    (host.disabled, host.trusted) = trust::load_state();
    // Plugins start processes and compare against the app's own server's version.
    process::warm();
    servers::read_own_version(app);
    // The servers added by address are in the registry from launch, their health checked.
    crate::servers::refresh(app);
    adopt(app, sources::builtins());
    // Plugin folders are read off the app's thread, so a launch reads none of them before its
    // first frame; a headless run (a capture, a check) waits for them.
    if crate::headless() {
        let found = sources::folders();
        adopt(app, found);
        folders_read(app);
        return;
    }
    let deliver = app.deliverer();
    tasks::run(move || {
        let found = sources::folders();
        deliver(Box::new(move |app: &mut App, tx: &mut Tx| {
            adopt(app, found);
            folders_read(app);
            tx.instant();
        }));
    });
}

/// Takes plugins found in tier order (higher first) into the window and starts the ones that
/// may run. One from `--plugin-dir` or an override with a built-in's id replaces the built-in;
/// an installed one can't, and shows refused beside it.
fn adopt(app: &mut App, found: Vec<sources::Found>) {
    let builtins: Vec<&str> = sources::builtin_ids().collect();
    for found in found {
        let id = found.manifest.id.clone();
        let shadows = found.tier == Tier::User && builtins.contains(&id.as_str());
        let existing = app.plugins.find(&id);
        let replaces = existing.filter(|&i| app.plugins.list[i].tier > found.tier);
        if existing.is_some() && replaces.is_none() && !shadows {
            continue;
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
        let plugin = Plugin { manifest: found.manifest, origin: found.origin, tier: found.tier, life: Life::Discovered, trust, hot: Hot::Idle, half: Half::new(&id, plugin::Kind::Window) };
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
        drive(app, i, first);
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
    m.os.is_empty() || m.os.contains(&here)
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
    let files = app.plugins.list[i].origin.files();
    let p = &app.plugins.list[i];
    let (id, manifest) = (p.manifest.id.clone(), p.manifest.clone());
    *p.half.catalogs.borrow_mut() = sources::catalogs(&files);
    sources::intern_icons(&id, &files);
    if let Some(entry) = &manifest.app {
        app_half::ensure(app, &manifest, &files, entry);
    }
    let folder = p.origin.dir().and_then(|d| d.file_name()).map_or_else(|| id.clone(), |n| n.to_string_lossy().into_owned());
    let sources = sources::script_sources(&files, &folder, &manifest.entry);
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
    if matches!(p.life, Life::Errored { .. }) {
        return drive(app, i, LifeEvent::Reload);
    }
    if !p.running() {
        return;
    }
    let prepared = match prepared {
        Some(prep) => prep,
        None => match hot::prepare(&p.origin, &p.manifest) {
            Ok(prep) => prep,
            Err(e) => return console::push(&p.manifest.id, rune_luau::LogLevel::Error, &e),
        },
    };
    *p.half.catalogs.borrow_mut() = prepared.catalogs;
    sources::intern_icons(&p.manifest.id, &p.origin.files());
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
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    servers::poll_actions(app, tx);
    let now = app.clock;
    mark(&app.plugins, now);
    let mut changed = std::mem::take(&mut app.plugins.asked);
    changed |= app_half::poll(app, now, tx);
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
    for i in 0..app.plugins.list.len() {
        let p = &app.plugins.list[i];
        let Some(vm) = p.script().and_then(Script::vm_id) else { continue };
        let half = p.half.clone();
        for (signal, args) in services::process::drain(&half, vm) {
            changed |= fire_value(app, i, signal, args);
        }
        for (ty, data) in &events {
            let listeners: Vec<mlua::Value> = half.events.borrow().iter().filter(|(v, prefix, _)| *v == vm && ty.starts_with(prefix.as_str())).map(|(_, _, s)| s.clone()).collect();
            for signal in listeners {
                let event = services::map([("type", Data::String(ty.clone())), ("data", services::json_to_data(data)), ("serverId", Data::String(app.server_key().unwrap_or_else(|| "local".into())))]);
                changed |= fire_value(app, i, signal, Data::List(vec![event]));
            }
        }
        changed |= services::fire_watched(app, i, vm);
        let p = &app.plugins.list[i];
        let Some(script) = p.script() else { continue };
        changed |= cx::with_app(app, || script.poll(now, tx));
    }
    requests::carry_out(app, tx);
    app.plugins.clock.set(now);
    if app.plugins.synced.get() != epoch() {
        app.plugins.synced.set(epoch());
        crate::plugin_host::contrib::sync(app);
    }
    changed
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
        Life::Running(_) | Life::Reloading(_) | Life::Errored { .. } | Life::AwaitingTrust => {
            drive(app, i, LifeEvent::Disable);
            app_half::stop(id);
            server_sources::forget(id);
            app.plugins.disabled.insert(id.to_owned());
        }
        Life::Disabled => {
            app.plugins.disabled.remove(id);
            if p.trust.allows() {
                drive(app, i, LifeEvent::Load);
            } else {
                step_trust(app, i, TrustEvent::Ask);
                drive(app, i, LifeEvent::NeedTrust);
            }
        }
        Life::Discovered | Life::Loading | Life::Refused(_) => return,
    }
    crate::plugin_host::save_state(app);
}

/// Steps plugin `i`'s trust with `event` and carries out what the step asks for.
pub fn step_trust(app: &mut App, i: usize, event: TrustEvent) {
    let trust = std::mem::replace(&mut app.plugins.list[i].trust, Trust::NotNeeded);
    let (trust, effects) = trust.step(event);
    app.plugins.list[i].trust = trust;
    for effect in effects {
        match effect {
            TrustEffect::Remember(hash) => {
                let id = app.plugins.list[i].manifest.id.clone();
                app.plugins.disabled.remove(&id);
                app.plugins.trusted.insert(id, hash);
                crate::plugin_host::save_state(app);
            }
            TrustEffect::Load => drive(app, i, LifeEvent::Load),
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
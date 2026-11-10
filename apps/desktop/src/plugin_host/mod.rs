//! `PluginHost`: native plugins (`crates/opencode-plugin`) loaded over the C ABI and bound to
//! the app. Built-ins use the same boundary as third-party plugins.
//!
//! - **Discovery** reads `plugin.toml` files (`manifest.rs`) without loading a library; their
//!   settings search entries are known before any loads.
//! - **Trust**: a third-party library waits in `PendingTrust` until the user OKs it in a drawn
//!   prompt (`trust.rs`), which also offers to unblock a file Windows marked as downloaded.
//! - **Loading** shakes hands over the ABI (`loader.rs`), loads the plugin's stores, then sets up
//!   its app half (once per process) and its window half; each slot moves through an explicit
//!   state machine (`lifecycle.rs`, `swap.rs`).
//! - **Contributions** reach the app's surfaces (`contrib.rs`, `surfaces.rs`, `view.rs`): panels
//!   with their own launchers, commands with keybinds and `/slash`, menus and the app menu,
//!   settings pages, link handlers, titlebar items, composer pills, code block actions, shell
//!   slots, themes, file viewers, server sources and MCP tools.
//! - **Sharing**: contracts and services, read as `Live` values, and host-kept stores
//!   (`shared.rs`); background jobs on host workers (`tasks.rs`); icons and strings interned so
//!   nothing points into a library a reload unmaps (`icons.rs`).
//! - **Rendering** fills `PluginRenderArena` (`arena.rs`) with real `rune::El`s; memos live in
//!   the window's own cache under `p:<id>:`, so a reload evicts only that plugin's.
//! - **Idle**: `poll` swaps two `AtomicU64`s of bits and checks one deadline. With every plugin
//!   at rest it makes no call into any plugin and asks for no frame.
//! - **Hot reload** (debug builds, or `--plugin-hot`): the watcher thread copies and loads a
//!   rebuilt library (`hot.rs`); the app's thread only swaps instances, carrying editors, stores
//!   and `save_hot_state` over, and drops the old generation once the new one is set up.
//! - **Panics** never cross: a render panic shows an error card and the plugin keeps running; a
//!   handler, poll or setup panic fails that plugin alone and withdraws its contributions.
// The C ABI boundary is unsafe by nature; each block says what it relies on.
#![allow(unsafe_code)]

mod app_half;
mod arena;
mod builtins;
mod contrib;
mod dialog;
mod editors;
mod hot;
pub(crate) mod icons;
mod images;
pub(crate) mod instance;
mod lifecycle;
mod loader;
pub mod luau;
mod manifest;
mod memory;
mod motw;
mod popover;
pub(crate) mod process;
mod render;
mod requests;
mod run;
mod servers;
mod shared;
mod surfaces;
mod swap;
pub(crate) mod tasks;
mod trust;
pub(crate) mod view;
pub(crate) mod vtable;
pub(crate) mod widgets;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use opencode_plugin::abi::PluginEntryFn;
use rune::Waker;

pub use contrib::sync_luau as contrib_sync_luau;
pub use contrib::{code_block_actions, mcp_call, mcp_tools, mcp_tools_generation, menubar_items, open_link, run_menubar};
pub use dialog::{key as dialog_key, view as dialog_view};
pub use servers::{all as contributed_servers, authenticating as server_authenticating, changed as servers_changed, connect as connect_server, cover as server_cover, row as server_row, stage as servers_stage};
pub use editors::PluginEditors;
pub use popover::{key as popover_key, view as popover_view};
pub use run::{commands, key, run_command, run_slash, server_event, slash_commands};
pub use contrib::{run_menu_item, server_menu_items, settings_search};
pub use surfaces::{composer_pills, pill_views, file_viewer, settings_page, settings_page_actions, settings_page_icon, settings_page_ids, settings_page_key, settings_page_title, settings_pages, sidebar_items, slot, slot_with, titlebar_items};
pub use requests::reveal;
pub use trust::dialog as trust_dialog;
pub use view::settings_section;

use arena::PluginRenderArena;
use instance::Instance;
use lifecycle::{Event, PluginState};
use manifest::{Found, Manifest, Tier};
use vtable::WakeShared;

use crate::App;

#[derive(Default)]
pub struct PluginHost {
    slots: Vec<Slot>,
    pub editors: PluginEditors,
    arena: RefCell<PluginRenderArena>,
    wake: Arc<WakeShared>,
    /// The soonest `wake_after` deadline of any plugin, on the app clock.
    next_deadline: Option<f64>,
    /// The hot-reload watcher; dropping it stops its thread.
    _watcher: Option<hot::Watcher>,
    /// Libraries loaded off the app's thread (rebuilt, or just trusted), waiting to swap in.
    ready: hot::Ready,
    shadow_dir: Option<PathBuf>,
    disabled: BTreeSet<String>,
    /// Third-party libraries the user OKed: plugin id → the SHA-256 they OKed.
    trusted: BTreeMap<String, String>,
    shared: shared::Shared,
    /// Trust prompts put off for this launch.
    trust_later: BTreeSet<String>,
    started: bool,
    /// The plugin popover on screen.
    popover: Option<popover::Open>,
    /// Plugins' cached server reads (`resource`).
    resources: vtable::Resources,
    /// Server requests and saves in flight, by id: whose callback runs when they answer.
    pending: std::collections::HashMap<u64, requests::Pending>,
    next_pending: u64,
    /// The plugin dialog on screen.
    dialog: Option<dialog::Open>,
    /// Contributed servers whose health check is on its way.
    checking: HashSet<String>,
    /// Server events plugins listen for, waiting for the next poll (contrib::EVENTS).
    server_events: Vec<(String, String)>,
    /// The Luau plugins of this window (`luau::Host`, the window plugin host).
    pub luau: luau::Host,
}

impl PluginHost {
    /// Forgets every cached server read, as the window moves to another server.
    pub fn forget_reads(&mut self) {
        self.resources = vtable::Resources::default();
    }

    /// Whether a plugin dialog is open.
    pub fn dialog_open(&self) -> bool {
        self.dialog.is_some()
    }
}

pub enum Source {
    Library(PathBuf),
    Static(PluginEntryFn),
}

pub struct Slot {
    pub manifest: Manifest,
    pub dir: PathBuf,
    pub tier: Tier,
    pub source: Source,
    pub state: PluginState,
    pub instance: Option<Instance>,
    /// Why it failed to load, set up, or keep running.
    pub error: Option<String>,
    /// The last render panic; the plugin keeps running.
    pub render_error: RefCell<Option<String>>,
    /// Why the last reload kept the previous generation.
    pub reload_error: Option<String>,
    pub generation: u32,
    /// Bumped by a render panic, so no memo built before it is reused.
    pub salt: Cell<u64>,
    /// `p:<id>:`, the prefix of this plugin's memos.
    pub prefix: String,
    /// The tabs the last running generation offered (tab id, title, icon), kept so a failed
    /// plugin's open tab stays and shows why, with Reload.
    pub tabs: Vec<(String, String, String)>,
    /// A third-party library's SHA-256, what trusting it means.
    pub hash: Option<String>,
    /// Windows marked the library as downloaded from the internet.
    pub blocked: bool,
}

impl Slot {
    fn go(&mut self, event: Event) {
        match self.state.next(event) {
            Some(next) => self.state = next,
            None => tasks::log(format!("plugin {}: {event:?} doesn't apply in {:?}", self.manifest.id, self.state)),
        }
    }

    fn active(&self) -> Option<&Instance> {
        self.instance.as_ref().filter(|_| self.state == PluginState::Active)
    }

    fn library(&self) -> Option<&std::path::Path> {
        match &self.source {
            Source::Library(path) => Some(path),
            Source::Static(_) => None,
        }
    }
}

/// Discovers and loads the plugins once the window's waker exists (`--no-plugins` skips it).
pub fn setup(app: &mut App, waker: Waker) {
    let luau_waker = waker.clone();
    let persists = app.persists();
    let keep_screen = app.settings.keep_screen_active;
    let host = &mut app.plugins;
    host.shared.images.borrow_mut().start(waker.clone());
    host.wake.waker.set(waker).ok();
    if host.started || crate::flag("--no-plugins") {
        return;
    }
    host.started = true;
    host.shared.stores.persist = persists;
    // Keep screen active was the app's own setting before the Pairing plugin took the page: it
    // moves into the plugin's store once.
    if keep_screen && persists {
        host.shared.stores.load("pairing");
        if host.shared.stores.get("pairing", "keepScreenActive", "").is_none() {
            host.shared.stores.set("pairing", "keepScreenActive", "", "true");
        }
    }
    // In-process tests (the motion gate) load the linked built-ins alone: never the user's
    // folders, saved state or a watcher.
    let isolated = cfg!(test);
    let hot = !isolated && (crate::flag("--plugin-hot") || (cfg!(debug_assertions) && !crate::headless()));
    host.shadow_dir = hot.then(|| crate::dirs::cache().map(|c| c.join("plugin-hot"))).flatten();
    if let Some(dir) = &host.shadow_dir {
        loader::sweep(dir);
    }
    if !isolated {
        (host.disabled, host.trusted) = load_state();
    }
    let builtin_ids: HashSet<String> = builtins::all().iter().filter_map(|(src, _)| manifest::parse(src).ok()).map(|m| m.id).collect();
    let mut seen = HashSet::new();
    for (found, source) in discover(isolated) {
        let id = found.manifest.id.clone();
        // A third-party plugin can't take a built-in's id: it shows, refused, beside it.
        let shadows_builtin = found.tier == Tier::User && (builtin_ids.contains(&id) || seen_bundled(&host.slots, &id));
        if !seen.insert(id.clone()) && !shadows_builtin {
            continue;
        }
        let library = match &source {
            Source::Library(path) if found.tier.needs_trust() => Some(path.clone()),
            _ => None,
        };
        let mut slot = Slot {
            prefix: format!("p:{id}:"),
            manifest: found.manifest,
            dir: found.dir,
            tier: found.tier,
            source,
            state: PluginState::Discovered,
            instance: None,
            error: found.error,
            render_error: RefCell::new(None),
            tabs: Vec::new(),
            reload_error: None,
            generation: 0,
            salt: Cell::new(0),
            hash: library.as_deref().and_then(loader::hash),
            blocked: library.as_deref().is_some_and(motw::blocked),
        };
        let reserved = id == "opencode" || shadows_builtin;
        if slot.error.is_some() {
            slot.go(Event::Load);
            slot.go(Event::LibFailed);
        } else if !slot.manifest.runs_here() {
            slot.go(Event::WrongOs);
        } else if reserved {
            slot.go(Event::Reserved);
        } else if host.disabled.contains(&id) {
            slot.go(Event::UserDisabled);
        } else if slot.tier.needs_trust() && host.trusted.get(&id) != slot.hash.as_ref() {
            slot.go(Event::NeedsTrust);
        }
        host.slots.push(slot);
    }
    if !app.plugins.slots.is_empty() {
        tasks::start();
    }
    // Luau plugins start processes and compare against the app's own server's version too.
    process::warm();
    servers::read_own_version(app);
    // The servers added by address are in the registry from launch, their health checked.
    crate::servers::refresh(app);
    for i in 0..app.plugins.slots.len() {
        if app.plugins.slots[i].state == PluginState::Discovered {
            swap::load(app, i, None);
        }
    }
    swap::settle(app);
    luau::setup(app, luau_waker, isolated);
    let host = &mut app.plugins;
    if hot {
        // Third-party libraries aren't watched: a changed one waits for the user's OK again.
        let watches = host.slots.iter().enumerate().filter(|(_, s)| !s.tier.needs_trust()).filter_map(|(i, s)| {
            s.library().map(|path| hot::Watch { slot: i, path: path.to_owned(), id: s.manifest.id.clone() })
        });
        host._watcher = hot::Watcher::spawn(host.wake.clone(), host.ready.clone(), watches.collect(), host.shadow_dir.clone());
    }
    if crate::flag("--plugin-index") {
        contrib::print_index(app);
    }
}

fn seen_bundled(slots: &[Slot], id: &str) -> bool {
    slots.iter().any(|s| s.manifest.id == id && s.tier <= Tier::Bundled)
}

/// Every plugin folder in tier order, with how to load each; static built-ins join the
/// bundled tier ahead of its folders; `isolated`, the static built-ins alone.
fn discover(isolated: bool) -> Vec<(Found, Source)> {
    let statics = builtins::all().into_iter().filter_map(|(src, entry)| {
        let manifest = manifest::parse(src).ok()?;
        Some((Found { manifest, dir: PathBuf::new(), tier: Tier::Bundled, error: None }, Source::Static(entry)))
    });
    if isolated {
        return statics.collect();
    }
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_owned()));
    let mut tiers = Vec::new();
    if let Some(dir) = crate::arg("--plugin-dir") {
        tiers.push((Tier::Dev, PathBuf::from(dir)));
    }
    if let Some(config) = crate::dirs::config() {
        tiers.push((Tier::Override, config.join("plugins-override")));
    }
    let first = manifest::scan(&tiers).into_iter().map(with_library);
    let mut rest = Vec::new();
    if let Some(exe) = exe_dir {
        rest.push((Tier::Bundled, exe.join("plugins")));
    }
    if let Some(data) = crate::dirs::data() {
        rest.push((Tier::User, data.join("plugins")));
    }
    first.chain(statics).chain(manifest::scan(&rest).into_iter().map(with_library)).collect()
}

fn with_library(found: Found) -> (Found, Source) {
    let path = found.manifest.library_path(&found.dir);
    (found, Source::Library(path))
}

fn state_file() -> Option<PathBuf> {
    crate::dirs::data().map(|d| d.join("plugins-state.json"))
}

/// The plugins turned off, and the third-party libraries trusted.
fn load_state() -> (BTreeSet<String>, BTreeMap<String, String>) {
    let Some(text) = state_file().and_then(|f| std::fs::read_to_string(f).ok()) else { return Default::default() };
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let disabled = v["disabled"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect();
    let trusted = v["trusted"].as_object().into_iter().flatten().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect();
    (disabled, trusted)
}

pub(crate) fn save_state(app: &App) {
    if !app.persists() {
        return;
    }
    if let Some(file) = state_file() {
        let v = serde_json::json!({ "disabled": app.plugins.disabled, "trusted": app.plugins.trusted });
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        std::fs::write(file, v.to_string()).ok();
    }
}
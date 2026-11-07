//! `PluginHost`: native plugins (`crates/opencode-plugin`) loaded over the C ABI and bound to
//! the app: their panels in the session side region, their commands in the palette and on the
//! keyboard, their settings rows in Settings.
//!
//! - **Discovery** reads `plugin.toml` files (`manifest.rs`) without loading a library.
//! - **Loading** shakes hands over the ABI (`loader.rs`) and runs setup; each slot moves through
//!   an explicit state machine (`lifecycle.rs`).
//! - **Rendering** fills `PluginRenderArena` (`arena.rs`) with real `rune::El`s; memos live in
//!   the window's own cache under `p:<id>:`, so a reload evicts only that plugin's.
//! - **Idle**: `poll` swaps one `AtomicU64` of dirty bits and checks one deadline. With every
//!   plugin at rest it makes no call into any plugin and asks for no frame.
//! - **Hot reload** (debug builds, or `--plugin-hot`): a watcher thread notices a rebuilt
//!   library; the host loads a shadow copy of it next to the running one, sets it up, carries
//!   editors and `save_hot_state` over, and only then drops the old generation.
//! - **Panics** never cross: a render panic shows an error card and the plugin keeps running; a
//!   handler, poll or setup panic fails that plugin alone and withdraws its contributions.
// The C ABI boundary is unsafe by nature; each block says what it relies on.
#![allow(unsafe_code)]

mod arena;
mod builtins;
mod editors;
mod hot;
mod instance;
mod lifecycle;
mod loader;
mod manifest;
mod view;
mod vtable;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashSet};
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use opencode_plugin::EditorHandle;
use opencode_plugin::abi::{AbiEvent, AbiOutcome, AbiStatus, AbiStr, EVENT_COMMAND, EVENT_KEY, EVENT_PRESS, PluginEntryFn};
use rune::{KeyEvent, Tx, Waker};

pub use editors::PluginEditors;
pub use view::settings_section;

use arena::PluginRenderArena;
use instance::{Instance, Press};
use lifecycle::{Event, PluginState};
use loader::Loaded;
use manifest::{Found, Manifest, Tier};
use vtable::{ActionCtx, Kind, Request, SetupCtx, WakeShared, slot_bit};

use crate::App;

#[derive(Default)]
pub struct PluginHost {
    slots: Vec<Slot>,
    pub editors: PluginEditors,
    arena: RefCell<PluginRenderArena>,
    wake: Arc<WakeShared>,
    /// The soonest `wake_after` deadline of any plugin, on the app clock.
    next_deadline: Option<f64>,
    hot: Option<hot::Watcher>,
    shadow_dir: Option<PathBuf>,
    disabled: BTreeSet<String>,
    started: bool,
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
}

impl Slot {
    fn go(&mut self, event: Event) {
        match self.state.next(event) {
            Some(next) => self.state = next,
            None => eprintln!("plugin {}: {event:?} doesn't apply in {:?}", self.manifest.id, self.state),
        }
    }

    fn active(&self) -> Option<&Instance> {
        self.instance.as_ref().filter(|_| self.state == PluginState::Active)
    }
}

/// `s` as a `&'static str`, leaked once per distinct string: the palette and `rune::El::role`
/// take static strings, and plugins contribute few distinct ones.
pub fn intern(s: &str) -> &'static str {
    static POOL: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    let Ok(mut pool) = POOL.lock() else { return "" };
    if let Some(&s) = pool.get(s) {
        return s;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    pool.insert(leaked);
    leaked
}

/// Discovers and loads the plugins once the window's waker exists (`--no-plugins` skips it).
pub fn setup(app: &mut App, waker: Waker) {
    let host = &mut app.plugins;
    host.wake.waker.set(waker).ok();
    if host.started || crate::flag("--no-plugins") {
        return;
    }
    host.started = true;
    let hot = crate::flag("--plugin-hot") || (cfg!(debug_assertions) && !crate::headless());
    host.shadow_dir = hot.then(|| crate::dirs::cache().map(|c| c.join("plugin-hot"))).flatten();
    if let Some(dir) = &host.shadow_dir {
        loader::sweep(dir);
    }
    host.disabled = load_disabled();
    let builtin_ids: HashSet<String> = builtins::all().iter().filter_map(|(src, _)| manifest::parse(src).ok()).map(|m| m.id).collect();
    let mut seen = HashSet::new();
    for (found, source) in discover() {
        let id = found.manifest.id.clone();
        // A third-party plugin can't take a built-in's id: it shows, refused, beside it.
        let shadows_builtin = found.tier == Tier::User && (builtin_ids.contains(&id) || seen_bundled(&host.slots, &id));
        if !seen.insert(id.clone()) && !shadows_builtin {
            continue;
        }
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
        }
        host.slots.push(slot);
    }
    for i in 0..app.plugins.slots.len() {
        if app.plugins.slots[i].state == PluginState::Discovered {
            load(app, i);
        }
    }
    let host = &mut app.plugins;
    if hot {
        let watches = host.slots.iter().enumerate().filter_map(|(i, s)| match &s.source {
            Source::Library(path) => Some((i, path.clone())),
            Source::Static(_) => None,
        });
        host.hot = Some(hot::Watcher::spawn(host.wake.clone(), watches.collect()));
    }
}

fn seen_bundled(slots: &[Slot], id: &str) -> bool {
    slots.iter().any(|s| s.manifest.id == id && s.tier <= Tier::Bundled)
}

/// Every plugin folder in tier order, with how to load each; static built-ins join the
/// bundled tier ahead of its folders.
fn discover() -> Vec<(Found, Source)> {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_owned()));
    let mut tiers = Vec::new();
    if let Some(dir) = crate::arg("--plugin-dir") {
        tiers.push((Tier::Dev, PathBuf::from(dir)));
    }
    if let Some(config) = crate::dirs::config() {
        tiers.push((Tier::Override, config.join("plugins-override")));
    }
    let first = manifest::scan(&tiers).into_iter().map(with_library);
    let statics = builtins::all().into_iter().filter_map(|(src, entry)| {
        let manifest = manifest::parse(src).ok()?;
        Some((Found { manifest, dir: PathBuf::new(), tier: Tier::Bundled, error: None }, Source::Static(entry)))
    });
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

fn load_disabled() -> BTreeSet<String> {
    let Some(text) = state_file().and_then(|f| std::fs::read_to_string(f).ok()) else { return BTreeSet::new() };
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    v["disabled"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect()
}

fn save_disabled(app: &App) {
    if !app.persists() {
        return;
    }
    if let Some(file) = state_file() {
        let v = serde_json::json!({ "disabled": app.plugins.disabled });
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        std::fs::write(file, v.to_string()).ok();
    }
}

/// Loads slot `i`'s library and sets it up: `LoadingLib → PreparingStores → SettingUp → Active`.
fn load(app: &mut App, i: usize) {
    let host = &mut app.plugins;
    let slot = &mut host.slots[i];
    slot.go(Event::Load);
    slot.generation += 1;
    slot.reload_error = None;
    *slot.render_error.borrow_mut() = None;
    let loaded = match &slot.source {
        Source::Library(path) => loader::load(path, host.shadow_dir.as_deref(), &slot.manifest.id, slot.generation),
        Source::Static(entry) => loader::load_static(*entry),
    };
    let loaded = match loaded {
        Ok(l) => l,
        Err(e) => {
            eprintln!("plugin {}: {e}", slot.manifest.id);
            slot.error = Some(e);
            slot.go(Event::LibFailed);
            return;
        }
    };
    slot.go(Event::LibLoaded);
    // Declared stores land here once plugins have them; none load yet.
    slot.go(Event::StoresReady);
    let generation = slot.generation;
    match set_up(host, i, loaded, generation, &[]) {
        Ok(instance) => {
            let slot = &mut host.slots[i];
            slot.tabs = instance.contributions.panels.iter().map(|p| (p.tab.clone(), p.title.clone(), p.icon.clone())).collect();
            slot.instance = Some(instance);
            slot.error = None;
            slot.go(Event::SetupOk);
        }
        Err(e) => {
            let slot = &mut host.slots[i];
            eprintln!("plugin {}: {e}", slot.manifest.id);
            slot.error = Some(e);
            slot.go(Event::SetupFailed);
        }
    }
}

/// Runs a plugin's window setup, collecting what it registers.
fn set_up(host: &mut PluginHost, i: usize, loaded: Loaded, generation: u32, reuse: &[EditorHandle]) -> Result<Instance, String> {
    let id = host.slots[i].manifest.id.clone();
    let mut ctx = SetupCtx {
        kind: Kind::Setup,
        plugin_id: &id,
        slot: i,
        lib: &loaded.lib,
        editors: &mut host.editors,
        reuse,
        made: Default::default(),
        wake: &host.wake,
    };
    // SAFETY: the plugin's setup, with a setup context alive for the call.
    let out = unsafe { (loaded.vtable.window_setup)((&raw mut ctx).cast()) };
    let made = std::mem::take(&mut ctx.made);
    match out.into_result() {
        Ok((_, state)) if !state.is_null() => {
            Ok(Instance { generation, vtable: loaded.vtable, state, contributions: made, deadline: None, lib: loaded.lib })
        }
        failed => {
            for h in made.editors.iter().filter(|h| !reuse.contains(h)) {
                host.editors.remove(*h);
            }
            Err(match failed {
                Err((AbiStatus::Panicked, message)) => format!("panicked during setup: {message}"),
                Err((_, message)) => message,
                Ok(_) => "setup returned no state".to_owned(),
            })
        }
    }
}

/// Reloads slot `id`: a hot swap that keeps the running generation until the new one is set
/// up (`Active → Reloading → Active`), or a fresh load for a failed or turned-off plugin.
pub fn reload(app: &mut App, id: &str, tx: &mut Tx) {
    let Some(i) = app.plugins.slots.iter().position(|s| s.manifest.id == id) else { return };
    reload_slot(app, i, tx);
}

fn reload_slot(app: &mut App, i: usize, tx: &mut Tx) {
    tx.instant();
    let state = app.plugins.slots[i].state;
    if state == PluginState::Failed || state == PluginState::Disabled {
        tx.invalidate_memos_by_prefix(app.plugins.slots[i].prefix.clone());
        load(app, i);
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
    let loaded = match &slot.source {
        Source::Library(path) => loader::load(path, host.shadow_dir.as_deref(), &slot.manifest.id, generation),
        Source::Static(entry) => loader::load_static(*entry),
    };
    let Some(old) = slot.instance.as_ref() else { return };
    let (old_state, old_vtable, reuse) = (old.state, old.vtable, old.contributions.editors.clone());
    let swapped = loaded.and_then(|loaded| {
        let mut snapshot = opencode_plugin::abi::AbiOwnedBytes::EMPTY;
        // SAFETY: the running instance's own state and a valid out pointer.
        let saved = unsafe { (old_vtable.save_hot_state)(old_state, &mut snapshot) }.into_result().is_ok();
        let fresh = set_up(host, i, loaded, generation, &reuse)?;
        if saved {
            // SAFETY: the new instance's state and bytes alive for the call.
            let loaded = unsafe { (fresh.vtable.load_hot_state)(fresh.state, AbiStr::from_bytes(snapshot.as_bytes())) };
            if let Err((_, message)) = loaded.into_result() {
                eprintln!("plugin: carrying state across the reload failed: {message}");
            }
        }
        Ok(fresh)
    });
    let slot = &mut host.slots[i];
    match swapped {
        Ok(fresh) => {
            let kept: Vec<EditorHandle> = fresh.contributions.editors.clone();
            slot.tabs = fresh.contributions.panels.iter().map(|p| (p.tab.clone(), p.title.clone(), p.icon.clone())).collect();
            let old = slot.instance.replace(fresh);
            slot.reload_error = None;
            *slot.render_error.borrow_mut() = None;
            slot.salt.set(slot.salt.get() + 1);
            tx.invalidate_memos_by_prefix(slot.prefix.clone());
            slot.go(Event::ReloadSettled);
            let name = slot.manifest.name.clone();
            if let Some(old) = old {
                for h in old.contributions.editors.iter().filter(|h| !kept.contains(h)) {
                    host.editors.remove(*h);
                }
            }
            eprintln!("plugin {name}: reloaded (generation {generation})");
        }
        Err(e) => {
            eprintln!("plugin {}: reload failed, keeping the running one: {e}", slot.manifest.id);
            slot.reload_error = Some(e.clone());
            slot.go(Event::ReloadSettled);
            let name = slot.manifest.name.clone();
            crate::status_toast::fail(app, format!("Couldn't reload {name}"), e);
        }
    }
}

/// Fails slot `i` (a handler or poll panicked): withdraws everything it contributed.
fn fail(app: &mut App, i: usize, message: String, tx: &mut Tx) {
    let host = &mut app.plugins;
    let slot = &mut host.slots[i];
    eprintln!("plugin {}: {message}", slot.manifest.id);
    slot.error = Some(message);
    slot.go(Event::Fault);
    slot.salt.set(slot.salt.get() + 1);
    tx.invalidate_memos_by_prefix(slot.prefix.clone());
    if let Some(old) = slot.instance.take() {
        for h in &old.contributions.editors {
            host.editors.remove(*h);
        }
    }
    let name = host.slots[i].manifest.name.clone();
    crate::status_toast::fail(app, format!("{name} stopped"), "The plugin crashed. Reload it from Settings › Plugins.");
}

/// Turns slot `id` on or off, remembering the choice.
pub fn toggle_enabled(app: &mut App, id: &str, tx: &mut Tx) {
    let Some(i) = app.plugins.slots.iter().position(|s| s.manifest.id == id) else { return };
    tx.instant();
    let host = &mut app.plugins;
    let slot = &mut host.slots[i];
    match slot.state {
        PluginState::Active => {
            slot.go(Event::Dispose);
            tx.invalidate_memos_by_prefix(slot.prefix.clone());
            if let Some(old) = slot.instance.take() {
                for h in &old.contributions.editors {
                    host.editors.remove(*h);
                }
            }
            host.slots[i].go(Event::Disposed);
            host.disabled.insert(id.to_owned());
        }
        PluginState::Disabled | PluginState::Failed => {
            host.disabled.remove(id);
            load(app, i);
        }
        _ => return,
    }
    save_disabled(app);
}

/// The outcome of running a plugin callback.
enum Ran {
    Changed(bool),
    Declined,
    Gone,
}

/// Runs a callback of slot `i` (generation `generation`, when it must match) with a handler
/// context, then carries out what it asked for. A panic fails the plugin.
fn act(app: &mut App, tx: &mut Tx, i: usize, generation: Option<u32>, call: impl FnOnce(*mut c_void, *mut c_void) -> AbiOutcome<bool>) -> Ran {
    let host = &mut app.plugins;
    let Some(state) = host
        .slots
        .get(i)
        .and_then(Slot::active)
        .filter(|inst| generation.is_none_or(|g| g == inst.generation))
        .map(|inst| inst.state)
    else {
        return Ran::Gone;
    };
    let mut ctx = ActionCtx { kind: Kind::Action, editors: &mut host.editors, tx, requests: Vec::new(), wake_after: None };
    let out = call(state, (&raw mut ctx).cast());
    let (requests, wake_after) = (std::mem::take(&mut ctx.requests), ctx.wake_after);
    match out.into_result() {
        Ok((status, changed)) => {
            carry_out(app, i, requests, wake_after, tx);
            if status == AbiStatus::Declined { Ran::Declined } else { Ran::Changed(changed) }
        }
        Err((_, message)) => {
            fail(app, i, message, tx);
            Ran::Changed(true)
        }
    }
}

fn carry_out(app: &mut App, i: usize, requests: Vec<Request>, wake_after: Option<u64>, tx: &mut Tx) {
    if let Some(ms) = wake_after {
        let due = app.clock + ms as f64 / 1000.0;
        if let Some(inst) = app.plugins.slots[i].instance.as_mut() {
            inst.deadline = Some(inst.deadline.map_or(due, |d| d.min(due)));
        }
        app.plugins.next_deadline = Some(app.plugins.next_deadline.map_or(due, |d| d.min(due)));
        tx.wake_after(std::time::Duration::from_millis(ms));
    }
    let id = app.plugins.slots[i].manifest.id.clone();
    for request in requests {
        match request {
            Request::OpenPanel(panel) => {
                let key = crate::panels::panel_key(view::PANEL, &format!("{id}.{panel}"));
                app.open_side_tab(&key, None, false, false, tx);
            }
            Request::Toast(text) => crate::status_toast::say(app, text, ""),
        }
    }
}

/// An element's press handler, from a plugin's render.
pub(crate) fn press(app: &mut App, tx: &mut Tx, press: &Press) {
    let cb = press.env.cb;
    let Some(func) = cb.func else { return };
    let event = AbiEvent { kind: EVENT_PRESS, ..AbiEvent::default() };
    // SAFETY: the plugin's handler with its own data, on its instance's state.
    let ran = act(app, tx, press.slot, Some(press.generation), |state, ctx| unsafe { func(state, ctx, cb.data, &event) });
    if matches!(ran, Ran::Changed(false) | Ran::Gone) {
        tx.live_only();
    }
}

/// Runs plugin command `id` (`<plugin>.<command>`); false when it declined or is gone.
pub fn run_command(app: &mut App, id: &str, tx: &mut Tx) -> bool {
    run_command_as(app, id, EVENT_COMMAND, tx)
}

fn run_command_as(app: &mut App, id: &str, kind: u32, tx: &mut Tx) -> bool {
    let found = app.plugins.slots.iter().enumerate().find_map(|(i, s)| {
        let cmd = s.active()?.contributions.commands.iter().find(|c| c.id == id)?;
        Some((i, cmd.run.cb))
    });
    let Some((i, cb)) = found else { return false };
    let Some(func) = cb.func else { return false };
    let event = AbiEvent { kind, ..AbiEvent::default() };
    // SAFETY: as `press`.
    let ran = act(app, tx, i, None, |state, ctx| unsafe { func(state, ctx, cb.data, &event) });
    !matches!(ran, Ran::Declined | Ran::Gone)
}

/// A plugin command's keybind. Chords with Ctrl, Alt or ⌘ fire anywhere; a plain key only
/// outside a focused control, unless the command is `editable`.
pub fn key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let plain = !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt);
    let hit = app.plugins.slots.iter().filter_map(Slot::active).flat_map(|inst| &inst.contributions.commands).find(|c| {
        c.keybind.as_ref().is_some_and(|k| k.matches_tx(ev, tx)) && (!plain || c.editable || tx.focused().is_none())
    });
    let Some(id) = hit.map(|c| c.id) else { return false };
    run_command_as(app, id, EVENT_KEY, tx)
}

/// The palette's plugin commands.
pub fn commands(app: &App) -> Vec<crate::commands::Command> {
    app.plugins
        .slots
        .iter()
        .filter_map(Slot::active)
        .flat_map(|inst| &inst.contributions.commands)
        .filter(|c| !c.hidden)
        .map(|c| crate::commands::Command {
            id: c.id,
            title: c.title,
            description: c.description,
            slash: None,
            keys: None,
            chord: c.keybind.as_ref().and_then(|k| k.chords().first().copied()),
            run: crate::commands::Run::Plugin(c.id),
        })
        .collect()
}

/// Polls the plugins whose waker fired or whose `wake_after` came due, and swaps in rebuilt
/// libraries. At rest: one atomic swap, one comparison, no call into any plugin.
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    let now = app.clock;
    let host = &mut app.plugins;
    let dirty = host.wake.dirty.swap(0, Ordering::AcqRel);
    let hot = host.wake.hot.swap(0, Ordering::AcqRel);
    let due = host.next_deadline.is_some_and(|d| now >= d);
    if dirty == 0 && hot == 0 && !due {
        return false;
    }
    let mut changed = false;
    for i in 0..host_len(app) {
        if hot & slot_bit(i) != 0 {
            reload_slot(app, i, tx);
            changed = true;
        }
    }
    if dirty != 0 || due {
        for i in 0..host_len(app) {
            let deadline = app.plugins.slots[i].active().and_then(|inst| inst.deadline);
            if dirty & slot_bit(i) == 0 && deadline.is_none_or(|d| now < d) {
                continue;
            }
            if let Some(inst) = app.plugins.slots[i].instance.as_mut() {
                inst.deadline = None;
            }
            let Some(vtable) = app.plugins.slots[i].active().map(|inst| inst.vtable) else { continue };
            // SAFETY: the plugin's poll on its own state.
            if let Ran::Changed(c) = act(app, tx, i, None, |state, ctx| unsafe { (vtable.poll)(state, ctx) }) {
                changed |= c;
            }
        }
        app.plugins.next_deadline =
            app.plugins.slots.iter().filter_map(|s| s.active().and_then(|inst| inst.deadline)).min_by(f64::total_cmp);
    }
    changed
}

fn host_len(app: &App) -> usize {
    app.plugins.slots.len()
}

//! `PluginHost`: plugins as `--!strict` Luau source and a `plugin.toml` against the
//! engine-agnostic contract (`opencode-plugin-spec`), run on `rune-luau`, one VM per plugin half.
//!
//! - **Two hosts.** A window's [`PluginHost`] (`WindowPluginHost`) runs every plugin's window
//!   half, `init.luau`; the process's `AppPluginHost` ([`app_half`]) runs each `app.luau` once,
//!   polled once per tick whatever the number of windows.
//! - **Loading** ([`runtime::boot`], [`sources`]): the built-ins are compiled into the binary (a
//!   debug build runs them from the repository, so saving one reloads it); `--plugin-dir`,
//!   overrides and installed plugins come from their folders, and one from outside the app waits
//!   for a one-time OK ([`trust`]) keyed by its files' SHA-256.
//! - **Lifecycle** ([`runtime::lifecycle`]): `drive`, `reload`, `poll`, `toggle`, and `step_trust`.
//! - **Contributions** reach the app's surfaces ([`surfaces`], [`contrib`], [`panels`],
//!   [`commands`], [`ui::view`]).
//! - **Rendering** ([`surfaces`]): each contribution's render is memoized on the script's
//!   version, its props, the theme and a stamp of what it read last time ([`cx::Dep`]); an
//!   unchanged frame enters no VM.
//! - **Host-kept state** ([`bridge::shared`]): each plugin's stores, what the app knows that
//!   plugins ask about, and their images; processes ([`process`]) and background work ([`tasks`])
//!   off the app's thread; icons and strings interned for Rune's `&'static` data ([`icons`]).

pub(crate) mod bridge;
pub mod native;
pub(crate) mod runtime;
pub mod services;
pub(crate) mod ui;

pub use opencode_plugin_runtime::{catalogs, flyout, icons, images, machines, process, reveal, tasks};

pub use bridge::{commands, console, cx, project_request, requests, server_sources, servers, stores};
pub(crate) use bridge::contrib;
pub use runtime::{app_half, sources, trust};
#[cfg(not(target_arch = "wasm32"))]
pub use runtime::types;
pub(crate) use runtime::{dormant, memory, plugin, vm};
pub use ui::{panels, surfaces};
pub(crate) use ui::editors;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use opencode_plugin_spec::Manifest;
use rune::{KeyEvent, Tx};
use rune_luau::{Data, Script};

pub use catalogs::Catalogs;
pub use commands::{commands, key, run_command, run_slash, slash_commands};
pub use contrib::{
    code_block_actions, mcp_call, mcp_tools, mcp_tools_generation, menubar_items, open_link, run_menu_item,
    run_menubar, server_menu_items, settings_search,
};
pub use flyout::{Flyout, FlyoutEvent};
pub use machines::{Hot, HotEffect, Life, LifeEffect, LifeEvent, Trust, TrustEffect, TrustEvent};
pub use plugin::Half;
pub use reveal::reveal;
pub use runtime::boot::{doze_owed, first_frame, setup};
pub use runtime::lifecycle::{drive, fire_named, poll, reload, step_trust, toggle};
pub use servers::{
    ServerRec, all as contributed_servers, authenticating as server_authenticating, changed as servers_changed,
    connect as connect_server, cover as server_cover, row as server_row, stage as servers_stage,
};
pub use sources::{Origin, Tier};
pub use surfaces::{
    dialog_view, file_viewer, pill_views, popover_view, settings_page, settings_page_actions, settings_page_icon,
    settings_page_ids, settings_page_key, settings_page_title, settings_pages, sidebar_items, slot, slot_with,
    titlebar_items,
};
pub use trust::dialog as trust_dialog;
pub use ui::view::settings_section;

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
    Popover {
        plugin: Rc<str>,
        id: String,
        anchor: String,
        side: rune::Side,
        align: rune::Align,
        gap: f32,
        data: Data,
        flyout: Box<Flyout>,
        back: Option<String>,
    },
    PillMenu {
        plugin: Rc<str>,
        id: String,
        anchor: String,
        data: Data,
    },
}

impl Overlay {
    /// Whether plugin `plugin`'s popover `id` is open, on its own or as a row's list.
    pub fn popover_open(&self, plugin: &str, id: &str) -> bool {
        match self {
            Overlay::Popover { plugin: p, id: i, flyout, .. } if &**p == plugin => {
                i == id || flyout.list().is_some_and(|l| l.id == id)
            }
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

/// The pane's display scale while a panel renders (`PanelViewCtx.env_scale`), defaulting to `1.0`.
pub(crate) struct PanelScale(Cell<f32>);

impl Default for PanelScale {
    fn default() -> Self {
        Self(Cell::new(1.0))
    }
}

impl PanelScale {
    pub(crate) fn get(&self) -> f32 {
        self.0.get()
    }

    pub(crate) fn replace(&self, scale: f32) -> f32 {
        self.0.replace(scale)
    }
}

/// The window's plugin host (`WindowPluginHost`).
#[derive(Default)]
pub struct PluginHost {
    pub list: Vec<Plugin>,
    /// The plugins turned off in Settings › Plugins.
    disabled: BTreeSet<String>,
    /// Plugins from outside the app the user OKed: plugin id → the SHA-256 of their files.
    trusted: BTreeMap<String, String>,
    shared: bridge::shared::Shared,
    /// Contributed servers whose health check is on its way.
    checking: HashSet<String>,
    /// The window's page visits, for server covers' one sign-in offer per visit.
    pub cover_visit: servers::CoverVisit,
    /// Where booting this window's plugins stands (unstarted, or started with folder scan,
    /// deferred built-ins, and after-frame doze latch).
    boot: runtime::boot::Boot,
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
    pub asked: Cell<bool>,
    /// Actions plugins queued for this window during a VM entry, carried out by `requests::carry_out`.
    pub(crate) pending_requests: RefCell<Vec<(Rc<str>, cx::Request)>>,
    /// Per-window memo cache for reactive command flags (`cx::answer`).
    pub(crate) command_answers: RefCell<HashMap<String, cx::Answer>>,
    /// Per-window memo cache for panel listing callbacks (`panels::answer`).
    pub(crate) panel_answers: RefCell<HashMap<String, panels::AnswerEntry>>,
    /// Per-window memo cache for instance panel tab lists (`panels::listed`).
    pub(crate) panel_listed: RefCell<HashMap<(String, String, String), panels::Listed>>,
    /// Per-window cache of each session's last step with token usage (`services::session`).
    pub(crate) last_usage: RefCell<HashMap<String, services::CachedUsage>>,
    /// Display scale of the panel currently rendering (`ui.terminalSurface`, `ui.browserSurface`).
    pub(crate) panel_scale: PanelScale,
    /// In-flight MCP tool calls owned by this window's plugin VMs, with their deadlines.
    pub(crate) mcp_calls: RefCell<HashMap<u64, (opencode_kit::client_mcp::McpCall, rune::time::Instant)>>,
    /// Next MCP tool call id for this window.
    pub(crate) next_mcp_call: Cell<u64>,
    /// Server events plugins listen for, waiting for the next poll.
    events: RefCell<Vec<(String, serde_json::Value)>>,
    watcher: Option<runtime::hot::Watcher>,
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

/// Queues a server event for the plugins listening for its type, delivered on the next poll.
pub fn server_event(app: &mut App, ty: &str, data: &serde_json::Value) {
    if dormant::listens(app, ty) {
        dormant::buffer_event(app, ty, data);
    }
    let host = &app.plugins;
    let wanted = host
        .running()
        .any(|(_, p)| p.half.events.borrow().iter().any(|(_, prefix, _)| ty.starts_with(prefix.as_str())));
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
        services::fire(app, p, &format!("editor.{index}.KeyPressed"), |lua| {
            mlua::IntoLuaMulti::into_lua_multi(key.to_owned(), lua)
        });
    }
    requests::carry_out(app, tx);
}

/// Enter in a plugin's field: its editor's `Submitted` fires with the text.
pub fn submitted(app: &mut App, plugin: &str, index: usize, tx: &mut Tx) {
    let text = editors::text_in(app, plugin, index);
    if let Some(p) = app.plugins.list.iter().find(|p| p.manifest.id == plugin) {
        services::fire(app, p, &format!("editor.{index}.Submitted"), |lua| {
            mlua::IntoLuaMulti::into_lua_multi(text.clone(), lua)
        });
    }
    requests::carry_out(app, tx);
}

/// `--plugin-index`: each plugin, where it runs from, its state, and what it contributed.
pub fn print_index(app: &App) {
    for p in &app.plugins.list {
        eprintln!(
            "plugin index: plugin: {} {} (contract {}, {}, {}) from {}",
            p.manifest.id,
            p.manifest.version,
            p.manifest.contract,
            p.tier.label(),
            p.life.label(),
            p.origin.describe()
        );
        if let Some(e) = p.life.why() {
            eprintln!("plugin index: error: {}: {e}", p.manifest.id);
        }
        let c = p.half.contribs.borrow();
        for x in &c.panels {
            eprintln!(
                "plugin index: panel: {} {}{}",
                x.key,
                x.spec.title,
                if x.spec.launcher == Some(false) { ", no launcher" } else { "" }
            );
        }
        for x in &c.commands {
            eprintln!(
                "plugin index: command: {} {} [{}] section {:?}{}",
                x.key,
                x.spec.title,
                x.spec.bind.as_deref().unwrap_or_default(),
                x.spec.section,
                if x.spec.hidden == Some(true) { ", hidden" } else { "" }
            );
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
        eprintln!(
            "luau metrics: surfaces memo hits {hits} misses {misses} | since mark: hits {} misses {}",
            hits - mark.0,
            misses - mark.1
        );
    }
}

/// Notes the counters at the mark `--luau-metrics-from` names, once.
pub(super) fn mark(host: &PluginHost, now: f64) {
    if !host.baseline.borrow().is_empty() || host.mark_hits.get() != (0, 0) {
        return;
    }
    let Some(from) = crate::arg("--luau-metrics-from").and_then(|s| s.parse::<f64>().ok()) else { return };
    if now < from {
        return;
    }
    let base = host
        .list
        .iter()
        .filter_map(|p| Some((p.manifest.id.clone(), p.script().map(|s| (s.metrics().vm_calls, s.metrics().renders))?)))
        .collect();
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

/// Kernel feature registration for the Luau plugin host's overlays and idle memory/dormancy rest hook.
pub struct PluginHostFeature;

impl opencode_kernel::Feature<App> for PluginHostFeature {
    fn id(&self) -> &'static str {
        "feature.plugin-host"
    }

    fn register(&self, builder: &mut opencode_kernel::KernelBuilder<App>) {
        builder.register_overlay(
            opencode_kernel::OverlayRegistration::<App>::new("plugin.popover")
                .with_z(600)
                .with_key_priority(50)
                .with_render(|s, cx| {
                    let p = crate::palette::pal(s.dark(cx));
                    popover_view(s, p)
                })
                .with_key(popover_key),
        );
        builder.register_overlay(
            opencode_kernel::OverlayRegistration::<App>::new("plugin.dialog")
                .with_z(480)
                .with_key_priority(60)
                .with_render(|s, cx| {
                    let p = crate::palette::pal(s.dark(cx));
                    dialog_view(s, p, cx)
                })
                .with_key(dialog_key),
        );
        builder.register_overlay(
            opencode_kernel::OverlayRegistration::<App>::new("plugin.trust")
                .with_z(610)
                .with_render(|s, cx| {
                    let p = crate::palette::pal(s.dark(cx));
                    trust_dialog(s, p, cx)
                }),
        );
        builder.add_rest("plugin_host.rest", |s, _| {
            if s.tabs.is_empty() && matches!(&*s.route, crate::Route::Home) {
                dormant::sleep_unused(s);
            }
            memory::collect_garbage(s);
            #[cfg(feature = "alloc-sites")]
            memory::publish_vms(s);
        });
    }
}

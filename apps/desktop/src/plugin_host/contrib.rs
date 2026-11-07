//! The contribution registries the app's surfaces read: menu items and the app menu, link
//! handlers, code block actions, MCP tools, themes, server sources and settings search. Each
//! reads the active plugins' records; the ones read off the app's state (MCP tools, which the
//! MCP hub lists; code block actions, which markdown draws without the app) are mirrored by
//! `sync` whenever contributions change.

use std::cell::RefCell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use opencode_kit::client_mcp::{McpCall, McpResult, McpTool};
use opencode_plugin::abi::{EVENT_CODE_BLOCK, EVENT_MENU, contrib as kinds};
use rune::{El, Icon, Tx, icon, row};

use super::icons::resolve;
use super::instance::{ContribRec, ServerRec};
use super::run;
use super::{PluginHost, Slot};
use crate::App;
use crate::palette::Pal;

/// Active plugins' contributions of `kind`, with their slots.
pub(super) fn active(host: &PluginHost, kind: u32) -> impl Iterator<Item = (usize, &ContribRec)> {
    host.slots.iter().enumerate().filter_map(|(i, s)| s.active().map(|inst| (i, inst))).flat_map(move |(i, inst)| inst.of(kind).map(move |c| (i, c)))
}

fn find<'a>(host: &'a PluginHost, kind: u32, key: &str) -> Option<(usize, &'a ContribRec)> {
    active(host, kind).find(|(_, c)| c.key == key)
}

struct CodeAction {
    key: &'static str,
    label: String,
    icon: &'static Icon,
    languages: Vec<String>,
}

thread_local! {
    static CODE_ACTIONS: RefCell<Vec<CodeAction>> = const { RefCell::new(Vec::new()) };
}

static MCP_TOOLS: Mutex<Vec<(String, &'static str, McpTool)>> = Mutex::new(Vec::new());

/// Mirrors the registries read off the app's state.
pub(super) fn sync(host: &PluginHost) {
    let actions = active(host, kinds::CODE_BLOCK_ACTION)
        .map(|(i, c)| CodeAction {
            key: c.key,
            label: c.title.clone(),
            icon: resolve(&host.slots[i].manifest.id, if c.icon.is_empty() { "code" } else { &c.icon }),
            languages: c.target.split_whitespace().map(str::to_owned).collect(),
        })
        .collect();
    CODE_ACTIONS.with(|a| *a.borrow_mut() = actions);
    let tools = active(host, kinds::MCP_TOOL).map(|(i, c)| {
        let name = format!("{}_{}", host.slots[i].manifest.id, c.id).replace(['.', ' '], "_");
        let schema = serde_json::from_str(&c.target).unwrap_or_else(|_| serde_json::json!({ "type": "object" }));
        (name.clone(), c.key, McpTool::new(name, c.description.clone(), schema))
    });
    if let Ok(mut all) = MCP_TOOLS.lock() {
        let tools: Vec<_> = tools.collect();
        if *all != tools {
            *all = tools;
            MCP_TOOLS_GENERATION.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Bumped each time the plugins' MCP tools change.
static MCP_TOOLS_GENERATION: AtomicU64 = AtomicU64::new(0);

/// The plugins' tools on the app's client MCP (`rune_mcp::tools::catalog`), named
/// `<plugin>_<tool>`. A connection lists the tools there were when it opened, so the hub
/// reconnects when `mcp_tools_generation` moves.
pub fn mcp_tools() -> Vec<McpTool> {
    MCP_TOOLS.lock().map(|t| t.iter().map(|(_, _, tool)| tool.clone()).collect()).unwrap_or_default()
}

/// Which list of plugin MCP tools `mcp_tools` returns: it changes when a plugin adds,
/// removes or reloads one.
pub fn mcp_tools_generation() -> u64 {
    MCP_TOOLS_GENERATION.load(Ordering::Relaxed)
}

/// Runs a plugin's MCP tool, or hands `call` back when no plugin has one by its name.
pub fn mcp_call(app: &mut App, call: McpCall, tx: &mut Tx) -> Option<McpCall> {
    let key = MCP_TOOLS.lock().ok()?.iter().find(|(name, _, _)| *name == call.name).map(|(_, key, _)| *key);
    let Some(key) = key else { return Some(call) };
    let Some((i, scope, query)) = find(&app.plugins, kinds::MCP_TOOL, key).and_then(|(i, c)| Some((i, c.scope, c.query.as_ref()?.cb))) else {
        call.respond(McpResult::error("That plugin isn't running"));
        return None;
    };
    let args = call.arguments.to_string();
    let result = match run::query(app, tx, i, scope, query, &args) {
        Ok(Some(answer)) => serde_json::from_str(&answer).map_or_else(|_| McpResult::text(answer), McpResult::json),
        Ok(None) => McpResult::error("The plugin stopped"),
        Err(e) => McpResult::error(e),
    };
    call.respond(result);
    None
}

/// Follows `href` with the highest-priority plugin link handler that takes it; false when none
/// does.
pub fn open_link(app: &mut App, href: &str, tx: &mut Tx) -> bool {
    let mut handlers: Vec<_> = active(&app.plugins, kinds::LINK_HANDLER).filter_map(|(i, c)| Some((c.order, i, c.scope, c.query.as_ref()?.cb, c.run.as_ref()?.cb))).collect();
    // Highest priority first; ties keep registration order.
    handlers.sort_by_key(|h| std::cmp::Reverse(h.0));
    for (_, i, scope, matches, open) in handlers {
        if matches!(run::query(app, tx, i, scope, matches, href), Ok(Some(_))) {
            run::action(app, tx, i, scope, open, opencode_plugin::abi::EVENT_LINK, href, "");
            return true;
        }
    }
    false
}

/// Plugins' buttons for a code block's hover row, beside Copy (`TimelineHost::code_block_actions`).
pub fn code_block_actions(p: &Pal, language: Option<&str>, source: &str) -> Vec<El> {
    CODE_ACTIONS.with(|actions| {
        let actions = actions.borrow();
        let lang = language.unwrap_or_default();
        actions
            .iter()
            .filter(|a| a.languages.is_empty() || a.languages.iter().any(|l| l.eq_ignore_ascii_case(lang)))
            .map(|a| {
                let (key, code, lang) = (a.key, source.to_owned(), lang.to_owned());
                row()
                    .key(format!("code-action-{key}"))
                    .role("button")
                    .label(a.label.clone())
                    .help(a.label.clone())
                    .help_above()
                    .size(24.0, 24.0)
                    .center()
                    .radius(6.0)
                    .hover(p.hover)
                    .press(p.pressed)
                    .hover_motion(crate::view::INSTANT, crate::view::INSTANT)
                    .on_press_tx(move |s: &mut App, tx: &mut Tx| run_code_block_action(s, key, &code, &lang, tx))
                    .child(icon(a.icon, 16.0).color(p.icon_muted))
            })
            .collect()
    })
}

fn run_code_block_action(app: &mut App, key: &str, code: &str, language: &str, tx: &mut Tx) {
    let Some((i, scope, cb)) = find(&app.plugins, kinds::CODE_BLOCK_ACTION, key).and_then(|(i, c)| Some((i, c.scope, c.run.as_ref()?.cb))) else { return };
    tx.instant();
    run::action(app, tx, i, scope, cb, EVENT_CODE_BLOCK, code, language);
}

/// Runs menu item `key` (`<plugin>.<id>`) on `target` (a server's id for `server.row`).
pub fn run_menu_item(app: &mut App, key: &str, target: &str, tx: &mut Tx) {
    let Some((i, scope, cb)) = find(&app.plugins, kinds::MENU_ITEM, key).and_then(|(i, c)| Some((i, c.scope, c.run.as_ref()?.cb))) else { return };
    tx.instant();
    run::action(app, tx, i, scope, cb, EVENT_MENU, target, "");
}

/// The `server.add` or `server.row` menu items, for Settings › Servers once it has those menus:
/// `(key, title)`.
pub fn server_menu_items(app: &App, menu: &str) -> Vec<(&'static str, String)> {
    active(&app.plugins, kinds::MENU_ITEM).filter(|(_, c)| c.target == menu).map(|(_, c)| (c.key, c.title.clone())).collect()
}

/// The app menu's plugin items: `(submenu, key, title)`, the submenu one of `file`, `edit`,
/// `view`, `go`, `window`, `help` (`app` items join Help where there's no app menu).
pub fn menubar_items(app: &App) -> Vec<(&'static str, &'static str, String)> {
    let mut items: Vec<_> = active(&app.plugins, kinds::MENUBAR_ITEM).map(|(_, c)| (c.order, menu_name(&c.target), c.key, c.title.clone())).collect();
    items.sort_by_key(|i| i.0);
    items.into_iter().map(|(_, menu, key, title)| (menu, key, title)).collect()
}

fn menu_name(target: &str) -> &'static str {
    match target {
        "file" => "file",
        "edit" => "edit",
        "view" => "view",
        "go" => "go",
        "window" => "window",
        _ => "help",
    }
}

/// Runs app menu item `key`; false when no plugin has it.
pub fn run_menubar(app: &mut App, key: &str, tx: &mut Tx) -> bool {
    let Some((i, scope, cb)) = find(&app.plugins, kinds::MENUBAR_ITEM, key).and_then(|(i, c)| Some((i, c.scope, c.run.as_ref()?.cb))) else { return false };
    tx.instant();
    run::action(app, tx, i, scope, cb, EVENT_MENU, "", "");
    true
}

/// Every server the plugins' server sources list, for the server inventory once it reads
/// plugin sources (Settings › Servers, the server picker).
pub fn servers(app: &App) -> Vec<ServerRec> {
    app.plugins.slots.iter().filter_map(Slot::active).flat_map(|inst| inst.servers.iter().cloned()).collect()
}

/// One row settings search can find.
#[derive(Clone, Debug)]
pub struct SearchEntry {
    /// The plugin's name.
    pub owner: String,
    /// The page it's on: a host page (`general`, `appearance`, `plugins`) or the plugin's own.
    pub page: String,
    pub id: String,
    pub title: String,
    pub description: String,
    pub keywords: String,
}

/// The settings search index plugins add: every installed plugin's manifest entries (known
/// without loading it), and the running ones' settings rows and pages. Settings search reads it.
pub fn settings_search(app: &App) -> Vec<SearchEntry> {
    let host = &app.plugins;
    let mut out: Vec<SearchEntry> = Vec::new();
    for (slot, inst) in host.slots.iter().filter_map(|s| s.active().map(|inst| (s, inst))) {
        let owner = || slot.manifest.name.clone();
        out.extend(inst.contributions.settings.iter().map(|r| SearchEntry {
            owner: owner(),
            page: "plugins".to_owned(),
            id: r.id.clone(),
            title: r.title.clone(),
            description: r.description.clone(),
            keywords: r.keywords.clone(),
        }));
        for page in inst.contributions.of(kinds::SETTINGS_PAGE) {
            let page_id = if page.target.is_empty() { page.id.clone() } else { page.target.clone() };
            out.extend(page.keywords.lines().map(|line| {
                let mut f = line.split('\t');
                let mut next = || f.next().unwrap_or_default().to_owned();
                SearchEntry { owner: owner(), page: page_id.clone(), id: next(), title: next(), description: next(), keywords: next() }
            }));
        }
    }
    // A manifest's entries, unless the running plugin registered the same row itself.
    let manifest = host.slots.iter().flat_map(|s| {
        s.manifest.search.iter().map(|e| SearchEntry {
            owner: s.manifest.name.clone(),
            page: e.page.clone(),
            id: e.id.clone(),
            title: e.title.clone(),
            description: e.description.clone(),
            keywords: e.keywords.clone(),
        })
    });
    let manifest: Vec<SearchEntry> = manifest.filter(|e| !out.iter().any(|o| o.owner == e.owner && o.id == e.id)).collect();
    out.extend(manifest);
    out
}

/// `--plugin-index`: prints what plugins added to the registries that have no screen of their
/// own yet (settings search, server sources and their menus, MCP tools, themes), for checking a
/// plugin from the command line.
pub(super) fn print_index(app: &App) {
    for slot in &app.plugins.slots {
        let how = match &slot.source {
            super::Source::Library(path) => path.display().to_string(),
            super::Source::Static(_) => "linked in (static-builtins)".to_owned(),
        };
        eprintln!("plugin index: plugin: {} {} ({}, {}) from {how}", slot.manifest.id, slot.manifest.version, slot.tier.label(), slot.state.label());
        let Some(inst) = slot.active() else { continue };
        for p in &inst.contributions.panels {
            let launcher = if p.flags & opencode_plugin::abi::PANEL_NO_LAUNCHER != 0 { ", no launcher" } else { "" };
            let label = if p.label.is_some() { ", own tab label" } else { "" };
            eprintln!("plugin index: panel: {} {}{launcher}{label}", p.tab, p.title);
        }
        for c in &inst.contributions.commands {
            eprintln!("plugin index: command: {} {} [{}]", c.id, c.title, c.keybind.as_ref().map(|k| format!("{k:?}")).unwrap_or_default());
        }
        for c in inst.contributions.contribs.iter().filter(|c| matches!(c.kind, kinds::SLOT | kinds::POPOVER | kinds::DIALOG)) {
            let kind = match c.kind {
                kinds::SLOT => "slot",
                kinds::POPOVER => "popover",
                _ => "dialog",
            };
            eprintln!("plugin index: {kind}: {} {} (order {})", c.key, c.target, c.order);
        }
    }
    for e in settings_search(app) {
        eprintln!("plugin index: settings search: {} › {} › {} ({}): {} [{}]", e.owner, e.page, e.title, e.id, e.description, e.keywords);
    }
    for slot in &app.plugins.slots {
        for store in &slot.manifest.stores {
            eprintln!("plugin index: store: {}/{store}", slot.manifest.id);
        }
    }
    for s in servers(app) {
        eprintln!("plugin index: server: {}/{} {} state {} {}", s.source, s.id, s.name, s.state, s.url);
    }
    for menu in ["server.add", "server.row"] {
        for (key, title) in server_menu_items(app, menu) {
            eprintln!("plugin index: {menu} menu item: {key} {title}");
        }
    }
    for (_, c) in active(&app.plugins, kinds::SERVER) {
        eprintln!("plugin index: server source: {} {}", c.key, c.title);
    }
    for tool in mcp_tools() {
        eprintln!("plugin index: MCP tool: {} – {}", tool.name, tool.description);
    }
    for t in crate::themes::all().iter().filter(|t| t.id.contains(':')) {
        eprintln!("plugin index: theme: {} {}", t.id, t.name);
    }
}

/// Adds plugin `plugin`'s theme to Appearance as `<plugin>:<id>`; false for bad JSON.
pub(super) fn register_theme(plugin: &str, id: &str, name: &str, json: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return false };
    let mode = |m: &str| v[m].as_object().cloned().unwrap_or_default();
    crate::themes::register(&format!("{plugin}:{id}"), if name.is_empty() { id } else { name }, [mode("light"), mode("dark")]);
    true
}

//! The contribution registries the app's surfaces read: menu items and the app menu, link
//! handlers, code block actions, MCP tools and settings search. The ones read off the app's
//! state (MCP tools, which the MCP hub lists; code block actions, which markdown draws without
//! the app) are mirrored by [`sync`] whenever contributions change.

use std::cell::RefCell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use opencode_kit::client_mcp::{McpCall, McpTool};
use rune::{El, Icon, Tx, icon, row};

use crate::App;
use crate::palette::Pal;
use crate::plugin_host::icons::resolve;

struct CodeAction {
    key: &'static str,
    label: String,
    icon: &'static Icon,
    languages: Vec<String>,
}

thread_local! {
    /// The plugins' code block actions, mirrored by `sync`.
    static CODE_ACTIONS: RefCell<Vec<CodeAction>> = const { RefCell::new(Vec::new()) };
}

/// The plugins' MCP tools, mirrored by `sync`.
static MCP_TOOLS: Mutex<Vec<McpTool>> = Mutex::new(Vec::new());

/// Bumped each time the plugins' MCP tools change.
static MCP_TOOLS_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Mirrors the plugins' code block actions and MCP tools, read off the app's state.
pub fn sync(app: &App) {
    // A window whose plugins never started (`--no-plugins`) has none to list, and mustn't empty
    // the lists the process's other windows share.
    if !app.plugins.boot.is_started() {
        return;
    }
    let all: Vec<CodeAction> = app.plugins.listed().flat_map(|(_, p)| p.half.contribs.borrow().code_block_actions.iter().map(|c| CodeAction { key: c.key, label: c.spec.title.clone(), icon: resolve(&p.manifest.id, &c.spec.icon), languages: c.spec.languages.clone() }).collect::<Vec<_>>()).collect();
    CODE_ACTIONS.with(|a| *a.borrow_mut() = all);
    let tools: Vec<McpTool> = super::commands::mcp_tools(app).into_iter().map(|(name, description, schema)| McpTool::new(name, description, schema)).collect();
    if let Ok(mut t) = MCP_TOOLS.lock()
        && *t != tools
    {
        *t = tools;
        MCP_TOOLS_GENERATION.fetch_add(1, Ordering::Relaxed);
    }
}

/// The plugins' tools on the app's client MCP (`rune_mcp::tools::catalog`). Open connections
/// announce changes through `ClientMcp::set_tools` when `mcp_tools_generation` moves.
pub fn mcp_tools() -> Vec<McpTool> {
    MCP_TOOLS.lock().map(|t| t.clone()).unwrap_or_default()
}

/// Which list of plugin MCP tools `mcp_tools` returns: it changes when a plugin adds,
/// removes or reloads one.
pub fn mcp_tools_generation() -> u64 {
    MCP_TOOLS_GENERATION.load(Ordering::Relaxed)
}

/// Runs a plugin's MCP tool, or hands `call` back when no plugin has one by its name.
/// `session` is the session the call came from.
pub fn mcp_call(app: &mut App, session: &str, call: McpCall, tx: &mut Tx) -> Option<McpCall> {
    let directory = if call.directory.is_empty() { app.session(session).map(|s| s.directory.clone()).unwrap_or_default() } else { call.directory.clone() };
    super::commands::mcp_call(app, call, session, &directory, tx)
}

/// Follows `href` with the highest-priority plugin link handler that takes it; false when none
/// does.
pub fn open_link(app: &mut App, href: &str, tx: &mut Tx) -> bool {
    super::commands::open_link(app, href, tx)
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
                    .on_press_tx(move |s: &mut App, tx: &mut Tx| super::commands::run_code_block_action(s, key, &code, &lang, tx))
                    .child(icon(a.icon, 16.0).color(p.icon_muted))
            })
            .collect()
    })
}

/// Runs menu item `key` (`<plugin>.<id>`) on `target` (a server's id for `server.row`).
pub fn run_menu_item(app: &mut App, key: &str, target: &str, tx: &mut Tx) {
    super::commands::run_menu_item(app, key, target, tx);
}

/// The `server.add` or `server.row` menu items, for Settings › Servers: `(key, title)`.
pub fn server_menu_items(app: &App, menu: &str) -> Vec<(&'static str, String)> {
    super::commands::menu_items(app, menu).into_iter().map(|(key, title, ..)| (key, title)).collect()
}

/// The app menu's plugin items: `(submenu, key, title)`, the submenu one of `file`, `edit`,
/// `view`, `go`, `window`, `help` (`app` items join Help where there's no app menu).
pub fn menubar_items() -> Vec<(&'static str, &'static str, String)> {
    let mut items: Vec<_> = super::commands::menubar_items().into_iter().map(|(key, menu, label, order)| (order, menu_name(&menu), key, label)).collect();
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
        // The app menu is macOS's alone (`desktop-menu.ts`): the in-window menu lists none of it.
        "app" => "app",
        _ => "help",
    }
}

/// Runs app menu item `key`; false when no plugin has it.
pub fn run_menubar(app: &mut App, key: &str, tx: &mut Tx) -> bool {
    super::commands::run_menubar(app, key, tx)
}

/// One row settings search can find.
#[derive(Clone, Debug)]
pub struct SearchEntry {
    /// The plugin's name.
    pub owner: String,
    /// The page it's on: a host page (`general`, `plugins`, `servers`) or the plugin's own.
    pub page: String,
    pub id: String,
    pub title: String,
    pub description: String,
    pub keywords: String,
}

/// The settings search index plugins add: the running ones' settings rows, and every installed
/// plugin's manifest entries (known without running it) that no running plugin registered itself.
pub fn settings_search(app: &App) -> Vec<SearchEntry> {
    let mut out: Vec<SearchEntry> = Vec::new();
    for lp in &app.plugins.list {
        let owner = lp.manifest.name.clone();
        out.extend(lp.half.contribs.borrow().settings.iter().map(|c| SearchEntry { owner: owner.clone(), page: "plugins".to_owned(), id: c.spec.id.clone(), title: c.spec.title.clone(), description: c.spec.description.clone().unwrap_or_default(), keywords: c.spec.keywords.clone().unwrap_or_default() }));
        out.extend(lp.manifest.settings_search.iter().filter(|e| !out.iter().any(|o| o.id == e.id)).map(|e| SearchEntry { owner: owner.clone(), page: e.page.clone(), id: e.id.clone(), title: e.title.clone(), description: e.description.clone().unwrap_or_default(), keywords: e.keywords.clone().unwrap_or_default() }).collect::<Vec<_>>());
    }
    out
}

/// `--plugin-index`: prints each plugin and what it added, the registries without a screen of
/// their own (settings search, servers and their menus, MCP tools) too, for checking a plugin
/// from the command line.
pub(crate) fn print_index(app: &App) {
    crate::plugin_host::print_index(app);
    for e in settings_search(app) {
        eprintln!("plugin index: settings search: {} › {} › {} ({}): {} [{}]", e.owner, e.page, e.title, e.id, e.description, e.keywords);
    }
    for s in super::servers::all(app) {
        eprintln!("plugin index: server: {} {} {} {}", s.key, s.rec.name, s.rec.state.as_str(), s.rec.url);
    }
    for menu in ["server.add", "server.row"] {
        for (key, title) in server_menu_items(app, menu) {
            eprintln!("plugin index: {menu} menu item: {key} {title}");
        }
    }
    for tool in mcp_tools() {
        eprintln!("plugin index: MCP tool: {} – {}", tool.name, tool.description);
    }
}

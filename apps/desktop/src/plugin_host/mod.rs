//! `PluginHost`: plugins as `--!strict` Luau source and a `plugin.toml` against the contract
//! (`opencode-plugin-spec`), run on `rune-luau` (`luau`), bound to the app.
//!
//! - **Discovery and trust** (`luau::sources`, `luau::trust`): the built-ins are compiled into
//!   the binary; a plugin from outside the app waits for a one-time OK keyed by its files'
//!   SHA-256.
//! - **Contributions** reach the app's surfaces (`contrib.rs`, `surfaces.rs`, `view.rs`): panels
//!   with their own launchers, commands with keybinds and `/slash`, menus and the app menu,
//!   settings pages, link handlers, titlebar items, composer pills, code block actions, shell
//!   slots, file viewers, server sources (`servers.rs`) and MCP tools.
//! - **Host-kept state** (`shared.rs`): each plugin's stores, what the app knows that plugins ask
//!   about, and their images; processes (`process.rs`) and background work (`tasks.rs`) off the
//!   app's thread; icons and strings interned for Rune's `&'static` data (`icons.rs`).
//! - **Idle**: `poll` checks each script's inbox and deadline and returns; nothing at rest
//!   enters a VM or asks for a frame.

mod contrib;
pub(crate) mod icons;
mod images;
pub mod luau;
mod memory;
pub(crate) mod process;
mod reveal;
mod servers;
mod shared;
mod surfaces;
pub(crate) mod tasks;
pub(crate) mod view;
mod widgets;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;

use rune::{KeyEvent, Tx, Waker};

pub use contrib::{code_block_actions, mcp_call, mcp_tools, mcp_tools_generation, menubar_items, open_link, run_menu_item, run_menubar, server_menu_items, settings_search};
pub use luau::surfaces::{dialog_view, popover_view};
pub use luau::trust::dialog as trust_dialog;
pub use reveal::reveal;
pub use servers::{ServerRec, all as contributed_servers, authenticating as server_authenticating, changed as servers_changed, connect as connect_server, cover as server_cover, row as server_row, stage as servers_stage};
pub use surfaces::{file_viewer, pill_views, settings_page, settings_page_actions, settings_page_icon, settings_page_ids, settings_page_key, settings_page_title, settings_pages, sidebar_items, slot, slot_with, titlebar_items};
pub use view::settings_section;

use crate::App;

#[derive(Default)]
pub struct PluginHost {
    /// The plugins turned off in Settings › Plugins.
    disabled: BTreeSet<String>,
    /// Plugins from outside the app the user OKed: plugin id → the SHA-256 of their files.
    trusted: BTreeMap<String, String>,
    shared: shared::Shared,
    started: bool,
    /// Contributed servers whose health check is on its way.
    checking: HashSet<String>,
    /// The window's plugins (`luau::Host`, the window plugin host).
    pub luau: luau::Host,
    /// The window's page visits, for server covers' one sign-in offer per visit.
    pub cover_visit: servers::CoverVisit,
}

impl PluginHost {
    /// Whether a plugin dialog is open.
    pub fn dialog_open(&self) -> bool {
        self.luau.dialog.is_some()
    }
}

/// Discovers and starts the plugins once the window's waker exists (`--no-plugins` skips it).
pub fn setup(app: &mut App, waker: Waker) {
    let persists = app.persists();
    let keep_screen = app.settings.keep_screen_active;
    let host = &mut app.plugins;
    host.shared.images.borrow_mut().start(waker.clone());
    if host.started || crate::flag("--no-plugins") {
        return;
    }
    host.started = true;
    host.shared.stores.persist = persists;
    // Keep screen active was the app's own setting before the Pairing plugin took the page: it
    // moves into the plugin's store once.
    if keep_screen && persists {
        host.shared.stores.load("pairing");
        if host.shared.stores.text("pairing", "keepScreenActive", "").is_none() {
            host.shared.stores.set("pairing", "keepScreenActive", "", "true");
        }
    }
    (host.disabled, host.trusted) = load_state();
    // Plugins start processes and compare against the app's own server's version.
    process::warm();
    servers::read_own_version(app);
    // The servers added by address are in the registry from launch, their health checked.
    crate::servers::refresh(app);
    luau::setup(app, waker);
    if crate::flag("--plugin-index") {
        contrib::print_index(app);
    }
}

/// Runs what came due, finished or arrived for the plugins, and steps their servers' sign-ins.
/// At rest it enters no VM.
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    servers::poll_actions(app, tx);
    luau::poll(app, tx)
}

/// Queues a server event for the plugins listening for its type, delivered on the next poll.
pub fn server_event(app: &mut App, ty: &str, data: &serde_json::Value) {
    luau::server_event(app, ty, data);
}

/// Runs plugin command `id` (`<plugin>.<command>`); false when it declined or is gone.
pub fn run_command(app: &mut App, id: &str, tx: &mut Tx) -> bool {
    luau::commands::run(app, id, None, tx)
}

/// Runs plugin command `id` from the composer's `/name words` with `words` as its input.
pub fn run_slash(app: &mut App, id: &str, input: &str, tx: &mut Tx) -> bool {
    luau::commands::run(app, id, Some(input), tx)
}

/// One plugin command the composer's `/` offers: hidden ones too (`/btw`), while enabled.
pub struct SlashCommand {
    pub id: &'static str,
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// `/name ` waits for its words rather than running when picked.
    pub args: bool,
}

pub fn slash_commands(app: &App) -> Vec<SlashCommand> {
    luau::commands::all(app).into_iter().filter(|c| luau::commands::offered(app, c)).filter_map(|c| Some(SlashCommand { id: c.id, name: c.slash?, title: c.title, description: c.description, args: c.slash_args })).collect()
}

/// A plugin command's keybind. Chords with Ctrl, Alt or ⌘ fire anywhere; a plain key only
/// outside a focused control, unless the command is `editable`.
pub fn key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    luau::commands::key(app, ev, tx)
}

/// The plugins' commands: the ones offered now (`offered`), or every one, as Settings ›
/// Shortcuts lists them. Hidden ones come too, marked: the palette leaves them out.
pub fn commands(app: &App, offered: bool) -> Vec<crate::commands::Command> {
    luau::commands::all(app)
        .into_iter()
        .filter(|c| !offered || luau::commands::offered(app, c))
        .map(|c| crate::commands::Command {
            id: c.id,
            title: c.title,
            description: c.description,
            slash: c.slash,
            keys: None,
            chord: c.keybind.as_ref().and_then(|k| k.chords().first().copied()),
            section: c.section,
            hidden: c.hidden,
            run: crate::commands::Run::Plugin(c.id),
        })
        .collect()
}

/// Esc closes the open plugin popover.
pub fn popover_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key != rune::Key::Escape || !app.plugins.luau.overlay.is_open() {
        return false;
    }
    luau::requests::close_popover(app, tx);
    true
}

/// Esc closes the open plugin dialog, telling its plugin.
pub fn dialog_key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key != rune::Key::Escape || app.plugins.luau.dialog.is_none() {
        return false;
    }
    luau::requests::dismiss_dialog(app, tx);
    true
}

fn state_file() -> Option<PathBuf> {
    crate::dirs::data().map(|d| d.join("plugins-state.json"))
}

/// The plugins turned off, and the plugins from outside the app trusted.
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

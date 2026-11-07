//! The app's commands, one list that ⌘K, ⇧⌘P and the composer's `/` all read, as Desktop's
//! `command.register` feeds its palette and slash popover (`shell/commands/command.tsx`,
//! `session/commands/use-session-commands.tsx`, `composer/commands.tsx`).
//!
//! A feature adds its commands from its own file:
//! `rune_register::register!(crate::commands::COMMANDS: Cmd = [Cmd::new("id", "Title", run)…]);`
//! Panels with a `.command(…)` join the list on their own (`App::commands`).

use crate::pickers::PickerHost as _;
use rune::{KeyEvent, Tx};
use rune_register::{collect, slice};

use crate::keymap::Bind;
use crate::panels::PanelRegion;
use crate::pickers::Kind;
use crate::{App, Route};
use opencode_kit::client::Write;

slice!(pub COMMANDS: Cmd);

/// One registered command (`CommandOption`).
#[derive(Clone, Copy)]
pub struct Cmd {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// The `/name` that runs it from the composer.
    pub slash: Option<&'static str>,
    /// Its keymap command, whose chord the palette draws (`⇧⌘T`, `Ctrl+Shift+T`).
    pub keys: Option<Bind>,
    /// Listed only while this holds (Desktop's `disabled`).
    pub enabled: fn(&App) -> bool,
    pub run: fn(&mut App, &mut Tx),
}

fn always(_: &App) -> bool {
    true
}

impl Cmd {
    pub const fn new(id: &'static str, title: &'static str, run: fn(&mut App, &mut Tx)) -> Cmd {
        Cmd { id, title, description: "", slash: None, keys: None, enabled: always, run }
    }

    pub const fn describe(mut self, description: &'static str) -> Cmd {
        self.description = description;
        self
    }

    pub const fn slash(mut self, name: &'static str) -> Cmd {
        self.slash = Some(name);
        self
    }

    pub const fn keys(mut self, keys: Bind) -> Cmd {
        self.keys = Some(keys);
        self
    }

    pub const fn when(mut self, enabled: fn(&App) -> bool) -> Cmd {
        self.enabled = enabled;
        self
    }
}

/// How a listed command runs: its own function, a panel's launcher, or a plugin's command.
#[derive(Clone, Copy, Debug)]
pub enum Run {
    Fn(fn(&mut App, &mut Tx)),
    Panel(&'static str),
    Plugin(&'static str),
}

/// A command as the palette and the `/` popover list it.
#[derive(Clone, Debug)]
pub struct Command {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub slash: Option<&'static str>,
    pub keys: Option<Bind>,
    /// A plugin command's chord, drawn when it has no keymap entry.
    pub chord: Option<rune::Chord>,
    pub run: Run,
}

/// The palette's opening rows over a session, in this order (`palette.ts:57-59`): New session,
/// Previous and Next session, the featured Toggle terminal (`gui-plugins/src/terminal/renderer.tsx:89`)
/// and Toggle review. Desktop's `workspace.new` has no command here yet.
pub const COMMON: [&str; 5] = ["new", "previous-tab", "next-tab", "terminal", "side"];

/// The opening rows on Home (`HomeCommandPalette`: the first five commands Desktop registers
/// there, `settings/command.tsx` then `shell/titlebar/tab-strip.tsx`).
pub const HOME_COMMON: [&str; 5] = ["settings", "previous-tab", "next-tab", "previous-unread", "next-unread"];

/// Search commands' own entry: the commands-only palette doesn't list itself.
pub const SEARCH: &str = "commands";

impl App {
    /// Every enabled command: the registered ones, the panels', then the plugins'.
    pub fn commands(&self) -> Vec<Command> {
        let own = collect(&COMMANDS).into_iter().filter(|c| (c.enabled)(self)).map(|c| Command {
            id: c.id,
            title: c.title,
            description: c.description,
            slash: c.slash,
            keys: c.keys,
            chord: None,
            run: Run::Fn(c.run),
        });
        let panels = self.all_panels().into_iter().filter_map(|p| {
            Some(Command {
                id: p.plugin,
                title: p.command_title?,
                description: p.command_description,
                slash: p.slash,
                keys: p.shortcut,
                chord: None,
                run: Run::Panel(p.plugin),
            })
        });
        own.chain(panels).chain(crate::plugin_host::commands(self)).collect()
    }

    pub fn run_command(&mut self, id: &str, tx: &mut Tx) {
        let Some(c) = self.commands().into_iter().find(|c| c.id == id) else { return };
        match c.run {
            Run::Fn(f) => f(self, tx),
            Run::Plugin(id) => {
                crate::plugin_host::run_command(self, id, tx);
            }
            Run::Panel("file") => self.open_file_search(tx),
            Run::Panel(plugin) => {
                if let Some(p) = self.all_panels().into_iter().find(|p| p.plugin == plugin) {
                    if p.region == PanelRegion::Dock && p.open_launcher.is_none() {
                        self.toggle_dock(tx);
                    } else if let Some(open) = p.open_launcher {
                        open(self, p.default_pane, tx);
                    }
                }
            }
        }
    }

    /// `input.focus` (⌃L): the composer takes the keys from anywhere it shows.
    pub fn focus_composer(&mut self, tx: &mut Tx) -> bool {
        if !self.has_composer() {
            return false;
        }
        tx.instant();
        self.close_switcher(false);
        self.close_picker(false);
        self.search.focused = false;
        self.find.input.focused = false;
        self.composer.focused = true;
        true
    }

    fn copy_id(&mut self, id: Option<String>, tx: &mut Tx) {
        let Some(id) = id.filter(|i| !i.is_empty()) else { return };
        tx.copy(id);
        self.status = "Copied".into();
    }

    fn open_session(&self) -> Option<String> {
        match &self.route {
            Route::Session(id) => Some(id.clone()),
            _ => None,
        }
    }

    /// The open root session, which the session commands act on (a subagent's has no composer).
    fn open_root(&self) -> Option<String> {
        let id = self.open_session()?;
        self.session(&id).is_none_or(|i| i.parent.is_none()).then_some(id)
    }
}

/// ⌃L, as Desktop binds `input.focus`. A focused panel (the terminal's clear screen) takes it first.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    Bind::FocusInput.matches_tx(ev, tx) && s.focus_composer(tx)
}

fn in_session(s: &App) -> bool {
    s.open_root().is_some()
}

fn has_project(s: &App) -> bool {
    s.open_session().and_then(|id| s.session(&id)).is_some_and(|i| !i.project.is_empty())
}

fn can_background(s: &App) -> bool {
    s.open_session().is_some_and(|id| !crate::background::blocking(s, &id).is_empty())
}

fn agent_shown(s: &App) -> bool {
    s.has_composer() && s.picker_show_agent()
}

fn is_live(s: &App) -> bool {
    matches!(s.source, crate::Source::Live { .. })
}

/// `/reload` (`composer/commands.tsx:45-84`): the server rebuilds every location, and the app
/// says so while it does and once it has.
fn reload(s: &mut App, _: &mut Tx) {
    s.status = "Reloading configuration.".into();
    s.write(Write::Reload);
}

fn compact(s: &mut App, _: &mut Tx) {
    let Some(id) = s.open_root() else { return };
    s.write(Write::Compact(id));
}

fn export(s: &mut App, _: &mut Tx) {
    let Some(id) = s.open_root() else { return };
    s.write(Write::Export(id));
}

fn rename(s: &mut App, tx: &mut Tx) {
    let Some(id) = s.open_root() else { return };
    tx.instant();
    s.begin_rename(&crate::session_menu::Target::new(&id, crate::session_menu::Spot::Header));
}

fn toggle_sidebar(s: &mut App, tx: &mut Tx) {
    s.toggle_sidebar(tx);
}

/// Top tabs have no sidebar to toggle (`shell.tsx` registers the command for side tabs only).
fn has_sidebar(s: &App) -> bool {
    s.tab_layout().has_sidebar()
}

rune_register::register!(COMMANDS: Cmd = [
    // `session.new` has no keybind of its own; it runs `tab.new` (`use-session-commands.tsx:446-451`).
    Cmd::new("new", "New session", |s, tx| s.new_session(tx)).slash("new"),
    Cmd::new("home", "Home", |s, tx| s.go_home(tx)).keys(Bind::Home),
    Cmd::new("sidebar", "Toggle sidebar", toggle_sidebar).keys(Bind::Sidebar).when(has_sidebar),
    Cmd::new("settings", "Open settings", |s, tx| s.toggle_settings(tx)).keys(Bind::Settings),
    Cmd::new("side", "Toggle review", |s, tx| s.toggle_side_region(tx)).keys(Bind::SideToggle).when(in_session),
    Cmd::new("open-project", "Open project", |s, tx| s.open_project_dialog(tx)).keys(Bind::OpenProject),
    Cmd::new("select-project", "Search projects", |s, tx| s.toggle_project_switcher(tx)).keys(Bind::SelectProject),
    Cmd::new("sessions", "Search sessions", |s, tx| s.open_session_search(tx)).keys(Bind::SessionSearch),
    Cmd::new(SEARCH, "Search commands", |s, tx| s.open_command_search(tx)).keys(Bind::CommandSearch),
    Cmd::new("model", "Choose model", |s, _| s.open_picker(Kind::Model))
        .describe("Select a different model")
        .slash("model")
        .keys(Bind::Model)
        .when(App::has_composer),
    // Offered while the agent shows, as Desktop's `disabled: !local.agent.visible()` (`composer/commands.tsx:145-161`).
    Cmd::new("agent", "Cycle agent", |s, tx| s.cycle_picker(Kind::Agent, false, tx))
        .describe("Switch to the next agent")
        .slash("agent")
        .keys(Bind::Agent)
        .when(agent_shown),
    Cmd::new("agent-back", "Cycle agent backwards", |s, tx| s.cycle_picker(Kind::Agent, true, tx))
        .describe("Switch to the previous agent")
        .keys(Bind::AgentBack)
        .when(agent_shown),
    Cmd::new("reload", "Reload configuration", reload)
        .describe("Reload agents, commands, skills, MCP servers and plugins from your config")
        .slash("reload")
        .when(is_live),
    Cmd::new("variant", "Cycle model variant", |s, tx| s.cycle_picker(Kind::Effort, false, tx))
        .describe("Switch to the next model variant")
        .keys(Bind::Variant)
        .when(|s| s.has_composer() && s.has_effort()),
    Cmd::new("compact", "Compact session", compact)
        .describe("Summarize the session to reduce context size")
        .slash("compact")
        .when(in_session),
    Cmd::new("undo", "Undo", |s, tx| {
        s.undo_last(tx);
    })
    .describe("Undo the last message")
    .slash("undo")
    .when(App::can_undo_session),
    Cmd::new("redo", "Redo", |s, tx| {
        s.redo_revert(tx);
    })
    .describe("Redo the last undone message")
    .slash("redo")
    .when(App::can_redo_session),
    Cmd::new("fork", "Fork from message", |s, tx| {
        s.open_fork_dialog(tx);
    })
    .describe("Create a new session from a previous message")
    .slash("fork")
    .when(App::can_fork_session),
    // Neither has a `/name` in Desktop (`use-session-commands.tsx:476-482`, `session-identity-header.tsx`).
    Cmd::new("background", "Move to background", |s, tx| {
        s.move_to_background(tx);
    })
    .keys(Bind::Background)
    .when(can_background),
    Cmd::new("rename", "Rename session", rename).keys(Bind::Rename).when(in_session),
    Cmd::new("export", "Export session", export)
        .describe("Export the full session transcript as JSON")
        .slash("export")
        .when(in_session),
    Cmd::new("copy-session", "Copy Session ID", |s, tx| s.copy_id(s.open_session(), tx)).when(|s| s.open_session().is_some()),
    Cmd::new("copy-project", "Copy Project ID", |s, tx| {
        let id = s.open_session().and_then(|id| s.session(&id)).map(|i| i.project.clone());
        s.copy_id(id, tx);
    })
    .when(has_project),
    Cmd::new("find", "Find in session", |s, tx| s.open_find(tx)).keys(Bind::Find).when(|s| s.open_session().is_some()),
    Cmd::new("focus", "Focus input", |s, tx| {
        s.focus_composer(tx);
    })
    .keys(Bind::FocusInput)
    .when(App::has_composer),
    Cmd::new("next-tab", "Next session", |s, tx| s.select_adjacent(1, false, tx)).keys(Bind::SessionNext),
    Cmd::new("previous-tab", "Previous session", |s, tx| s.select_adjacent(-1, false, tx)).keys(Bind::SessionPrev),
    Cmd::new("previous-unread", "Previous unread session", |s, tx| s.select_adjacent(-1, true, tx)).keys(Bind::UnseenPrev),
    Cmd::new("next-unread", "Next unread session", |s, tx| s.select_adjacent(1, true, tx)).keys(Bind::UnseenNext),
    Cmd::new("close-tab", "Close tab", |s, tx| {
        if let Some(id) = s.current_tab() {
            s.close_tab(&id, tx);
        }
    })
    .keys(Bind::TabClose)
    .when(|s| s.current_tab().is_some()),
    Cmd::new("reopen-tab", "Reopen closed tab", |s, tx| s.reopen_tab(tx)).keys(Bind::TabReopen),
    Cmd::new("zoom-in", "Zoom in", |s, tx| s.zoom_by(1, tx)).keys(Bind::ZoomIn),
    Cmd::new("zoom-out", "Zoom out", |s, tx| s.zoom_by(-1, tx)).keys(Bind::ZoomOut),
    Cmd::new("zoom-reset", "Actual size", |s, tx| s.zoom_by(0, tx)).keys(Bind::ZoomReset),
]);

#[cfg(test)]
mod tests;

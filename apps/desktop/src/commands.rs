//! The app's commands, one list that ⌘K, ⇧⌘P and the composer's `/` all read, as Desktop's
//! `command.register` feeds its palette and slash popover (`shell/commands/command.tsx`,
//! `session/commands/use-session-commands.tsx`, `composer/commands.tsx`).
//!
//! A feature adds its commands from its own file:
//! `rune_register::register!(crate::commands::COMMANDS: Cmd = [Cmd::new("id", key!("title.key"), run)…]);`
//! Titles and descriptions are catalog keys, read in the current language as the list is made.
//! Panels with a `.command(…)` join the list on their own (`App::commands`).

use crate::pickers::PickerHost as _;
use rune::{KeyEvent, Tx};
use rune_register::{collect, slice};

use crate::keymap::{Bind, Section};
use crate::pickers::Kind;
use crate::{App, Route};
use opencode_kit::client::Write;

slice!(pub COMMANDS: Cmd);

/// One registered command (`CommandOption`).
#[derive(Clone, Copy)]
pub struct Cmd {
    pub id: &'static str,
    pub title: opencode_i18n::Key,
    pub description: Option<opencode_i18n::Key>,
    /// The `/name` that runs it from the composer.
    pub slash: Option<&'static str>,
    /// Its keymap command, whose chord the palette draws (`⇧⌘T`, `Ctrl+Shift+T`).
    pub keys: Option<Bind>,
    /// Listed only while this holds (Desktop's `disabled`).
    pub enabled: fn(&App) -> bool,
    /// Its group in Settings › Shortcuts when Desktop's id would place it elsewhere (`groupFor`).
    pub section: Option<Section>,
    /// Kept out of the palette, as Desktop's `hidden` commands are (`command.tsx:148`), while its
    /// keys and Settings › Shortcuts still have it.
    pub hidden: bool,
    /// Its title when it depends on the app (Desktop's `title` getters), over `title`.
    pub titled: Option<fn(&App) -> opencode_i18n::Key>,
    pub run: fn(&mut App, &mut Tx),
}

fn always(_: &App) -> bool {
    true
}

impl Cmd {
    pub const fn new(id: &'static str, title: opencode_i18n::Key, run: fn(&mut App, &mut Tx)) -> Cmd {
        Cmd { id, title, description: None, slash: None, keys: None, enabled: always, section: None, hidden: false, titled: None, run }
    }

    pub const fn hidden(mut self) -> Cmd {
        self.hidden = true;
        self
    }

    pub const fn section(mut self, section: Section) -> Cmd {
        self.section = Some(section);
        self
    }

    pub const fn describe(mut self, description: opencode_i18n::Key) -> Cmd {
        self.description = Some(description);
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

    pub const fn titled(mut self, titled: fn(&App) -> opencode_i18n::Key) -> Cmd {
        self.titled = Some(titled);
        self
    }

    pub const fn when(mut self, enabled: fn(&App) -> bool) -> Cmd {
        self.enabled = enabled;
        self
    }
}

/// How a listed command runs: its own function, or a plugin's command.
#[derive(Clone, Copy, Debug)]
pub enum Run {
    Fn(fn(&mut App, &mut Tx)),
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
    /// Its Settings › Shortcuts group, when it names one.
    pub section: Option<Section>,
    /// Kept out of the palette; its keys and Settings › Shortcuts still have it.
    pub hidden: bool,
    /// A plugin's `featured` command: among the palette's opening rows.
    pub featured: bool,
    pub run: Run,
}

/// The palette's opening rows over a session, in this order (`palette.ts:57-60,160-165`): New
/// session, Previous and Next session, then every `featured` command (the terminal plugin's
/// Toggle terminal, `gui-plugins/src/terminal/renderer.tsx:89`), then Toggle review. Desktop's
/// `workspace.new` has no command here yet.
pub const COMMON: [&str; 3] = ["new", "previous-tab", "next-tab"];
pub const COMMON_VIEW: [&str; 1] = ["side"];

/// The opening rows on Home (`HomeCommandPalette`: the first five commands Desktop registers
/// there, `settings/command.tsx` then `shell/titlebar/tab-strip.tsx`).
pub const HOME_COMMON: [&str; 5] = ["settings", "previous-tab", "next-tab", "previous-unread", "next-unread"];

/// Search commands' own entry: the commands-only palette doesn't list itself.
pub const SEARCH: &str = "commands";

impl App {
    /// Every enabled command: the registered ones, the panels', then the plugins'.
    pub fn commands(&self) -> Vec<Command> {
        self.listing(true)
    }

    /// What the palette lists: the enabled commands less the hidden ones.
    pub fn palette_commands(&self) -> Vec<Command> {
        self.commands().into_iter().filter(|c| !c.hidden).collect()
    }

    /// Every command, offered here or not, as Desktop's `command.catalog` remembers them:
    /// Settings › Shortcuts lists a session's commands even when it opens from Home.
    pub fn catalog(&self) -> Vec<Command> {
        self.listing(false)
    }

    fn listing(&self, offered: bool) -> Vec<Command> {
        let own = collect(&COMMANDS).into_iter().filter(|c| !offered || (c.enabled)(self)).map(|c| Command {
            id: c.id,
            title: opencode_i18n::I18N.text(c.titled.map_or(c.title, |f| f(self))),
            description: c.description.map_or("", |d| opencode_i18n::I18N.text(d)),
            slash: c.slash,
            keys: c.keys,
            chord: None,
            section: c.section,
            hidden: c.hidden,
            featured: false,
            run: Run::Fn(c.run),
        });
        own.chain(crate::plugin_host::commands(self, offered)).collect()
    }

    pub fn run_command(&mut self, id: &str, tx: &mut Tx) {
        let Some(c) = self.commands().into_iter().find(|c| c.id == id) else {
            crate::plugin_host::run_command(self, id, tx);
            return;
        };
        match c.run {
            Run::Fn(f) => f(self, tx),
            Run::Plugin(id) => {
                crate::plugin_host::run_command(self, id, tx);
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
        *self.status = opencode_i18n::t!("common.copied").into();
    }

    fn open_session(&self) -> Option<String> {
        match &*self.route {
            Route::Session { id, .. } => Some(id.clone()),
            _ => None,
        }
    }

    /// The open root session, which the session commands act on (a subagent's has no composer).
    fn open_root(&self) -> Option<String> {
        let id = self.open_session()?;
        self.session(&id).is_none_or(|i| i.parent.is_none()).then_some(id)
    }
}

/// Its Settings › Shortcuts group, when a registered command names one.
pub fn section_of(id: &str) -> Option<Section> {
    collect(&COMMANDS).into_iter().find(|c| c.id == id).and_then(|c| c.section)
}

/// ⌃L, as Desktop binds `input.focus` (a focused panel, the terminal's clear screen, takes it
/// first); ⇧⌘L adds the open file's selected lines; ⇧⌘X switches the composer's shell mode.
/// Each declines its key where Desktop's command is disabled.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if Bind::FocusInput.matches_tx(ev, tx) {
        return s.focus_composer(tx);
    }
    if Bind::AddSelection.matches_tx(ev, tx) && s.can_add_selection() {
        s.add_selection(tx);
        return true;
    }
    if Bind::PromptModeToggle.matches_tx(ev, tx) && s.has_composer() {
        toggle_shell(s, tx);
        return true;
    }
    if Bind::JumpToMessage.matches_tx(ev, tx) && s.open_session().is_some() {
        return s.open_jump_dialog(tx);
    }
    for (bind, offset) in [(Bind::MessagePrev, -1), (Bind::MessageNext, 1)] {
        if bind.matches_tx(ev, tx) && s.open_session().is_some() {
            s.navigate_message(offset, tx);
            return true;
        }
    }
    false
}

fn toggle_shell(s: &mut App, tx: &mut Tx) {
    tx.animated();
    if s.add.shell() { s.exit_shell() } else { s.enter_shell() }
}

/// Delete session (`message-timeline.tsx:268-277`): the open root session's confirmation.
fn delete(s: &mut App, tx: &mut Tx) {
    let Some(id) = s.open_root() else { return };
    tx.instant();
    s.ask_delete(&id);
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
    matches!(*s.source, crate::Source::Live { .. })
}

/// `/reload` (`composer/commands.tsx:45-84`): the server rebuilds every location, and the app
/// says so while it does and once it has.
fn reload(s: &mut App, _: &mut Tx) {
    *s.status = opencode_i18n::t!("command.config.reload.pending").into();
    s.write(Write::Reload);
}

fn compact(s: &mut App, _: &mut Tx) {
    let Some(id) = s.open_root() else { return };
    s.write(Write::Compact(id));
}

fn export(s: &mut App, _: &mut Tx) {
    let Some(id) = s.open_root() else { return };
    s.export_session_on(&crate::servers::runtime::current(), &id);
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
    s.shows_sidebar()
}

rune_register::register!(COMMANDS: Cmd = [
    // `session.new` has no keybind of its own; it runs `tab.new` (`use-session-commands.tsx:446-451`).
    Cmd::new("new", opencode_i18n::key!("command.session.new"), |s, tx| s.new_session(tx)).slash("new").section(Section::Session),
    // `titlebar.tsx:428-434`: hidden from the palette; its key and Settings keep it.
    Cmd::new("home", opencode_i18n::key!("home.title"), |s, tx| s.go_home(tx)).keys(Bind::Home).hidden(),
    // `titlebar.tsx:148-160`.
    Cmd::new("back", opencode_i18n::key!("common.goBack"), |s, tx| {
        s.navigate(false, tx);
    })
    .keys(Bind::GoBack),
    Cmd::new("forward", opencode_i18n::key!("common.goForward"), |s, tx| {
        s.navigate(true, tx);
    })
    .keys(Bind::GoForward),
    // `use-session-commands.tsx:711-722`: its title says what it will do.
    Cmd::new("auto-accept", opencode_i18n::key!("command.permissions.autoaccept.enable"), |s, tx| s.toggle_auto_accept(tx))
        .titled(|s| match (s.auto_accepting(), matches!(*s.route, Route::Session { .. }) && s.open_session().and_then(|id| s.session(&id).and_then(|i| s.project_for_session(i))).is_some()) {
            (true, _) => opencode_i18n::key!("command.permissions.autoaccept.disable"),
            (false, true) => opencode_i18n::key!("command.permissions.autoaccept.project"),
            (false, false) => opencode_i18n::key!("command.permissions.autoaccept.enable"),
        })
        .keys(Bind::AutoAccept)
        .when(|s| s.open_session().is_some()),
    Cmd::new("sidebar", opencode_i18n::key!("command.sidebar.toggle"), toggle_sidebar).keys(Bind::Sidebar).when(has_sidebar),
    Cmd::new("settings", opencode_i18n::key!("command.settings.open"), |s, tx| s.toggle_settings(tx)).keys(Bind::Settings),
    Cmd::new("open-project", opencode_i18n::key!("command.project.open"), |s, tx| s.open_project_dialog(tx)).keys(Bind::OpenProject),
    Cmd::new("select-project", opencode_i18n::key!("session.new.project.search"), |s, tx| s.toggle_project_switcher(tx)).keys(Bind::SelectProject),
    Cmd::new("sessions", opencode_i18n::key!("command.session.search"), |s, tx| s.open_session_search(tx)).keys(Bind::SessionSearch),
    Cmd::new(SEARCH, opencode_i18n::key!("command.commands.search"), |s, tx| s.open_command_search(tx)).keys(Bind::CommandSearch),
    Cmd::new("reload", opencode_i18n::key!("command.config.reload"), reload)
        .describe(opencode_i18n::key!("command.config.reload.description"))
        .slash("reload")
        .when(is_live),
    Cmd::new("compact", opencode_i18n::key!("command.session.compact"), compact)
        .describe(opencode_i18n::key!("command.session.compact.description"))
        .slash("compact")
        .section(Section::Session)
        .when(in_session),
    Cmd::new("undo", opencode_i18n::key!("command.session.undo"), |s, tx| {
        s.undo_last(tx);
    })
    .describe(opencode_i18n::key!("command.session.undo.description"))
    .slash("undo")
    .section(Section::Session)
    .when(App::can_undo_session),
    Cmd::new("redo", opencode_i18n::key!("command.session.redo"), |s, tx| {
        s.redo_revert(tx);
    })
    .describe(opencode_i18n::key!("command.session.redo.description"))
    .slash("redo")
    .section(Section::Session)
    .when(App::can_redo_session),
    Cmd::new("fork", opencode_i18n::key!("command.session.fork"), |s, tx| {
        s.open_fork_dialog(tx);
    })
    .describe(opencode_i18n::key!("command.session.fork.description"))
    .slash("fork")
    .section(Section::Session)
    .when(App::can_fork_session),
    // Neither has a `/name` in Desktop (`use-session-commands.tsx:476-482`, `session-identity-header.tsx`).
    Cmd::new("background", opencode_i18n::key!("command.session.background"), |s, tx| {
        s.move_to_background(tx);
    })
    .keys(Bind::Background)
    .when(can_background),
    Cmd::new("rename", opencode_i18n::key!("command.session.rename"), rename).keys(Bind::Rename).when(in_session),
    Cmd::new("export", opencode_i18n::key!("command.session.export"), export)
        .describe(opencode_i18n::key!("command.session.export.description"))
        .slash("export")
        .section(Section::Session)
        .when(|s| s.open_root().is_some_and(|id| !s.exporting(crate::servers::runtime::active(s), &id))),
    // Desktop's `file.open` (`use-session-commands.tsx:551-558`): Ctrl+P's search, as `/open`.
    Cmd::new("file.open", opencode_i18n::key!("command.file.open"), |s, tx| s.open_file_search(tx))
        .describe(opencode_i18n::key!("palette.search.placeholder"))
        .keys(Bind::FileOpen)
        .slash("open")
        .when(in_session),
    Cmd::new("copy-session", opencode_i18n::key!("command.session.copyID"), |s, tx| s.copy_id(s.open_session(), tx)).section(Section::Session).when(|s| s.open_session().is_some()),
    Cmd::new("session.delete", opencode_i18n::key!("command.session.delete"), delete).when(in_session),
    Cmd::new("copy-project", opencode_i18n::key!("command.project.copyID"), |s, tx| {
        let id = s.open_session().and_then(|id| s.session(&id)).map(|i| i.project.clone());
        s.copy_id(id, tx);
    })
    .when(has_project),
    // After Copy project ID and before the message keys, as Desktop registers it (`use-session-commands.tsx:610`).
    Cmd::new("side", opencode_i18n::key!("command.review.toggle"), |s, tx| s.toggle_side_region(tx)).keys(Bind::SideToggle).when(in_session),
    // `use-session-commands.tsx:674-681`, before the message keys as Desktop registers it.
    Cmd::new("session.jumpToMessage", opencode_i18n::key!("dialog.jump.title"), |s, tx| {
        s.open_jump_dialog(tx);
    })
    .describe(opencode_i18n::key!("command.session.jumpToMessage.description"))
    .keys(Bind::JumpToMessage)
    .when(|s| s.open_session().is_some()),
    // `use-session-commands.tsx:636-653`: offered over any open session.
    Cmd::new("message.previous", opencode_i18n::key!("command.message.previous"), |s, tx| s.navigate_message(-1, tx))
        .describe(opencode_i18n::key!("command.message.previous.description"))
        .keys(Bind::MessagePrev)
        .when(|s| s.open_session().is_some()),
    Cmd::new("message.next", opencode_i18n::key!("command.message.next"), |s, tx| s.navigate_message(1, tx))
        .describe(opencode_i18n::key!("command.message.next.description"))
        .keys(Bind::MessageNext)
        .when(|s| s.open_session().is_some()),
    // The composer's commands, after the session's, as Desktop registers them (`composer/commands.tsx:111-161`,
    // mounted after `use-session-commands.tsx`): a palette query lists them in that order.
    Cmd::new("model", opencode_i18n::key!("command.model.choose"), |s, _| s.open_picker(Kind::Model))
        .describe(opencode_i18n::key!("command.model.choose.description"))
        .slash("model")
        .keys(Bind::Model)
        .when(App::has_composer),
    Cmd::new("variant", opencode_i18n::key!("command.model.variant.cycle"), |s, tx| s.cycle_picker(Kind::Effort, false, tx))
        .describe(opencode_i18n::key!("command.model.variant.cycle.description"))
        .keys(Bind::Variant)
        .when(|s| s.has_composer() && s.has_effort()),
    // Offered while the agent shows, as Desktop's `disabled: !local.agent.visible()` (`composer/commands.tsx:145-161`).
    Cmd::new("agent", opencode_i18n::key!("command.agent.cycle"), |s, tx| s.cycle_picker(Kind::Agent, false, tx))
        .describe(opencode_i18n::key!("command.agent.cycle.description"))
        .slash("agent")
        .keys(Bind::Agent)
        .when(agent_shown),
    Cmd::new("agent-back", opencode_i18n::key!("command.agent.cycle.reverse"), |s, tx| s.cycle_picker(Kind::Agent, true, tx))
        .describe(opencode_i18n::key!("command.agent.cycle.reverse.description"))
        .keys(Bind::AgentBack)
        .when(agent_shown),
    Cmd::new("find", opencode_i18n::key!("rune.command.session.find"), |s, tx| s.open_find(tx)).keys(Bind::Find).when(|s| s.open_session().is_some()),
    Cmd::new("focus", opencode_i18n::key!("command.input.focus"), |s, tx| {
        s.focus_composer(tx);
    })
    .keys(Bind::FocusInput)
    .when(App::has_composer),
    Cmd::new("next-tab", opencode_i18n::key!("command.session.next"), |s, tx| s.select_adjacent(1, tx)).keys(Bind::SessionNext),
    Cmd::new("previous-tab", opencode_i18n::key!("command.session.previous"), |s, tx| s.select_adjacent(-1, tx)).keys(Bind::SessionPrev),
    Cmd::new("previous-unread", opencode_i18n::key!("command.session.previous.unseen"), |s, tx| s.select_unread(-1, tx)).keys(Bind::UnseenPrev),
    Cmd::new("next-unread", opencode_i18n::key!("command.session.next.unseen"), |s, tx| s.select_unread(1, tx)).keys(Bind::UnseenNext),
    Cmd::new("close-tab", opencode_i18n::key!("command.tab.close"), |s, tx| {
        if let Some(id) = s.current_tab() {
            s.close_tab(&id, tx);
        }
    })
    .keys(Bind::TabClose)
    .when(|s| s.current_tab().is_some())
    // `titlebar.tsx:453-458`.
    .hidden(),
    // `titlebar.tsx:480-486`: the same close, listed.
    Cmd::new("close-session-tab", opencode_i18n::key!("command.tab.closeSession"), |s, tx| {
        if let Some(id) = s.current_tab() {
            s.close_tab(&id, tx);
        }
    })
    .keys(Bind::TabCloseSession)
    .when(|s| s.current_tab().is_some()),
    Cmd::new("reopen-tab", opencode_i18n::key!("command.tab.reopenClosed"), |s, tx| s.reopen_tab(tx)).keys(Bind::TabReopen),
    // Beyond Desktop, which forgets a closed window: the newest one kept opens again.
    Cmd::new("reopen-window", opencode_i18n::key!("rune.command.window.reopenClosed"), |s, _| crate::windows::reopen::reopen_newest(s)),
    Cmd::new("zoom-in", opencode_i18n::key!("desktop.menu.zoomIn"), |s, tx| s.zoom_by(1, tx)).keys(Bind::ZoomIn),
    Cmd::new("zoom-out", opencode_i18n::key!("desktop.menu.zoomOut"), |s, tx| s.zoom_by(-1, tx)).keys(Bind::ZoomOut),
    Cmd::new("zoom-reset", opencode_i18n::key!("desktop.menu.actualSize"), |s, tx| s.zoom_by(0, tx)).keys(Bind::ZoomReset),
    // The composer's modes (`composer/model.ts:579-599`).
    Cmd::new("prompt.mode.toggle", opencode_i18n::key!("command.prompt.mode.toggle"), toggle_shell).keys(Bind::PromptModeToggle).when(App::has_composer),
    Cmd::new("prompt.mode.shell", opencode_i18n::key!("command.prompt.mode.shell"), |s, tx| {
        tx.instant();
        s.enter_shell();
    })
    .when(|s| s.has_composer() && !s.add.shell()),
    Cmd::new("prompt.mode.normal", opencode_i18n::key!("command.prompt.mode.normal"), |s, tx| {
        tx.instant();
        s.exit_shell();
    })
    .when(|s| s.has_composer() && s.add.shell()),
]);

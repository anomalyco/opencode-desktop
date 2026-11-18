//! The app's commands, one list that ⌘K, ⇧⌘P and the composer's `/` all read, as Desktop's
//! `command.register` feeds its palette and slash popover (`shell/commands/command.tsx`,
//! `session/commands/use-session-commands.tsx`, `composer/commands.tsx`).
//!
//! A feature registers its commands in `Feature::register` via
//! `builder.add_command(CommandEntry::...)` (`s.kernel.commands`); core shell/window commands live
//! below in `COMMANDS`. Titles and descriptions are catalog keys, read in the current language as
//! the list is made. Panels with a `.command(…)` join the list on their own (`App::commands`).

pub mod type_to_compose;

use crate::pickers::PickerHost as _;
use rune::{KeyEvent, Tx};
use rune_register::{collect, slice};

use crate::keymap::{Bind, Section};
use crate::pickers::Kind;
use crate::App;
use opencode_kit::client::Write;

slice!(pub COMMANDS: Cmd);

/// One registered core shell command (`CommandOption`).
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
    /// Ascending palette display order (shared with kernel feature commands).
    pub order: u16,
    pub run: fn(&mut App, &mut Tx),
}

fn always(_: &App) -> bool {
    true
}

impl Cmd {
    pub const fn new(id: &'static str, title: opencode_i18n::Key, run: fn(&mut App, &mut Tx)) -> Cmd {
        Cmd { id, title, description: None, slash: None, keys: None, enabled: always, section: None, hidden: false, order: u16::MAX, run }
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

    pub const fn order(mut self, order: u16) -> Cmd {
        self.order = order;
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

impl Command {
    /// Effective first chord shown in palettes and slash menus, applying Settings › Shortcuts overrides.
    pub fn effective_chord(&self) -> Option<rune::Chord> {
        if let Some(bind) = self.keys {
            return bind.chord();
        }
        crate::keymap::overrides::command(self.id)
            .map_or(self.chord, |k| k.chords().first().copied())
    }
}

/// Search commands' own entry: the commands-only palette doesn't list itself.
pub const SEARCH: &str = "commands";

fn parse_section(name: &str) -> Option<Section> {
    match name.to_ascii_lowercase().as_str() {
        "general" => Some(Section::General),
        "session" => Some(Section::Session),
        "navigation" => Some(Section::Navigation),
        "model" | "models" => Some(Section::Models),
        "terminal" => Some(Section::Terminal),
        "prompt" => Some(Section::Prompt),
        _ => None,
    }
}

impl App {
    /// Every enabled command: the registered ones, the kernel features', then the plugins'.
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
        let own = collect(&COMMANDS).into_iter().filter(|c| !offered || (c.enabled)(self)).map(|c| {
            (
                c.order,
                Command {
                    id: c.id,
                    title: opencode_i18n::I18N.text(c.title),
                    description: c.description.map_or("", |d| opencode_i18n::I18N.text(d)),
                    slash: c.slash,
                    keys: c.keys,
                    chord: None,
                    section: c.section,
                    hidden: c.hidden,
                    featured: false,
                    run: Run::Fn(c.run),
                },
            )
        });
        let kernel = self
            .kernel
            .commands
            .iter_with_chords()
            .filter(|(c, _)| !offered || c.is_enabled(self))
            .map(|(c, chord)| {
                let keys = c.bind_id.and_then(Bind::by_id);
                (
                    c.order,
                    Command {
                        id: c.id,
                        title: c.title_for(self),
                        description: c.description_text().unwrap_or(""),
                        slash: c.slash,
                        keys,
                        chord: if keys.is_some() { None } else { chord },
                        section: c.section.and_then(parse_section),
                        hidden: c.hidden,
                        featured: false,
                        run: Run::Fn(c.run),
                    },
                )
            });
        let mut combined: Vec<(u16, Command)> = own.chain(kernel).collect();
        combined.sort_by_key(|(order, _)| *order);
        combined.into_iter().map(|(_, c)| c).chain(crate::plugin_host::commands(self, offered)).collect()
    }

    pub fn run_command(&mut self, id: &str, tx: &mut Tx) {
        if let Some(c) = collect(&COMMANDS).into_iter().find(|c| c.id == id && (c.enabled)(self)) {
            (c.run)(self, tx);
            return;
        }
        if let Some(entry) = self.kernel.commands.get(id).copied().filter(|e| e.is_enabled(self)) {
            (entry.run)(self, tx);
            return;
        }
        crate::plugin_host::run_command(self, id, tx);
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

    pub(crate) fn copy_id(&mut self, id: Option<String>, tx: &mut Tx) {
        let Some(id) = id.filter(|i| !i.is_empty()) else { return };
        tx.copy(id);
        *self.status = opencode_i18n::t!("common.copied").into();
    }

    /// The open root session, which the session commands act on (a subagent's has no composer).
    pub(crate) fn open_root_session(&self) -> Option<String> {
        let id = self.active_session_id()?;
        self.session(id).is_none_or(|i| i.parent.is_none()).then(|| id.to_owned())
    }

    pub(crate) fn in_root_session(&self) -> bool {
        self.open_root_session().is_some()
    }
}

/// Runs the enabled command bound to `bind` (core `COMMANDS` or kernel `CommandRegistry`),
/// returning `true` if one ran.
pub fn run_bind(s: &mut App, bind: Bind, tx: &mut Tx) -> bool {
    if let Some(cmd) = collect(&COMMANDS).into_iter().find(|c| c.keys == Some(bind) && (c.enabled)(s)) {
        (cmd.run)(s, tx);
        return true;
    }
    if let Some(entry) = s.kernel.commands.find_bind(bind.id(), s) {
        (entry.run)(s, tx);
        return true;
    }
    false
}

/// Keymap-bound commands dispatched ahead of plugin shortcuts (`commands::key`).
const KEY_BINDS: [Bind; 6] = [
    Bind::FocusInput,
    Bind::AddSelection,
    Bind::PromptModeToggle,
    Bind::JumpToMessage,
    Bind::MessagePrev,
    Bind::MessageNext,
];

/// ⌃L, as Desktop binds `input.focus` (a focused panel, the terminal's clear screen, takes it
/// first); ⇧⌘L adds the open file's selected lines; ⇧⌘X switches the composer's shell mode;
/// message jump/navigation; plus any shortcut-bound kernel commands registered by features.
/// Each declines its key where the command is disabled.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    for bind in KEY_BINDS {
        if bind.matches_tx(ev, tx) && run_bind(s, bind, tx) {
            return true;
        }
    }
    if let Some(entry) = s
        .kernel
        .commands
        .find_key_with(s, ev, tx, |id| crate::keymap::overrides::command(id))
    {
        (entry.run)(s, tx);
        return true;
    }
    false
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
    let Some(id) = s.open_root_session() else { return };
    s.write(Write::Compact(id));
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
    Cmd::new("new", opencode_i18n::key!("command.session.new"), |s, tx| s.new_session(tx)).slash("new").section(Section::Session).order(10),
    // `titlebar.tsx:428-434`: hidden from the palette; its key and Settings keep it.
    Cmd::new("home", opencode_i18n::key!("home.title"), |s, tx| s.go_home(tx)).keys(Bind::Home).hidden().order(20),
    // `titlebar.tsx:148-160`.
    Cmd::new("back", opencode_i18n::key!("common.goBack"), |s, tx| {
        s.navigate(false, tx);
    })
    .keys(Bind::GoBack)
    .order(30),
    Cmd::new("forward", opencode_i18n::key!("common.goForward"), |s, tx| {
        s.navigate(true, tx);
    })
    .keys(Bind::GoForward)
    .order(40),
    Cmd::new("sidebar", opencode_i18n::key!("command.sidebar.toggle"), toggle_sidebar).keys(Bind::Sidebar).order(60).when(has_sidebar),
    Cmd::new("reload", opencode_i18n::key!("command.config.reload"), reload)
        .describe(opencode_i18n::key!("command.config.reload.description"))
        .slash("reload")
        .order(140)
        .when(is_live),
    Cmd::new("compact", opencode_i18n::key!("command.session.compact"), compact)
        .describe(opencode_i18n::key!("command.session.compact.description"))
        .slash("compact")
        .section(Section::Session)
        .order(150)
        .when(App::in_root_session),
    Cmd::new("copy-session", opencode_i18n::key!("command.session.copyID"), |s, tx| {
        let id = s.active_session_id().map(str::to_owned);
        s.copy_id(id, tx);
    })
    .section(Section::Session)
    .order(230)
    .when(|s| s.active_session_id().is_some()),
    // The composer's commands, after the session's, as Desktop registers them (`composer/commands.tsx:111-161`,
    // mounted after `use-session-commands.tsx`): a palette query lists them in that order.
    Cmd::new("model", opencode_i18n::key!("command.model.choose"), |s, _| s.open_picker(Kind::Model))
        .describe(opencode_i18n::key!("command.model.choose.description"))
        .slash("model")
        .keys(Bind::Model)
        .order(300)
        .when(App::has_composer),
    Cmd::new("variant", opencode_i18n::key!("command.model.variant.cycle"), |s, tx| s.cycle_picker(Kind::Effort, false, tx))
        .describe(opencode_i18n::key!("command.model.variant.cycle.description"))
        .keys(Bind::Variant)
        .order(310)
        .when(|s| s.has_composer() && s.has_effort()),
    // Offered while the agent shows, as Desktop's `disabled: !local.agent.visible()` (`composer/commands.tsx:145-161`).
    Cmd::new("agent", opencode_i18n::key!("command.agent.cycle"), |s, tx| s.cycle_picker(Kind::Agent, false, tx))
        .describe(opencode_i18n::key!("command.agent.cycle.description"))
        .slash("agent")
        .keys(Bind::Agent)
        .order(320)
        .when(agent_shown),
    Cmd::new("agent-back", opencode_i18n::key!("command.agent.cycle.reverse"), |s, tx| s.cycle_picker(Kind::Agent, true, tx))
        .describe(opencode_i18n::key!("command.agent.cycle.reverse.description"))
        .keys(Bind::AgentBack)
        .order(330)
        .when(agent_shown),
    Cmd::new("focus", opencode_i18n::key!("command.input.focus"), |s, tx| {
        s.focus_composer(tx);
    })
    .keys(Bind::FocusInput)
    .order(350)
    .when(App::has_composer),
    Cmd::new("next-tab", opencode_i18n::key!("command.session.next"), |s, tx| s.select_adjacent(1, tx)).keys(Bind::SessionNext).order(360),
    Cmd::new("previous-tab", opencode_i18n::key!("command.session.previous"), |s, tx| s.select_adjacent(-1, tx)).keys(Bind::SessionPrev).order(370),
    Cmd::new("previous-unread", opencode_i18n::key!("command.session.previous.unseen"), |s, tx| s.select_unread(-1, tx)).keys(Bind::UnseenPrev).order(380),
    Cmd::new("next-unread", opencode_i18n::key!("command.session.next.unseen"), |s, tx| s.select_unread(1, tx)).keys(Bind::UnseenNext).order(390),
    Cmd::new("close-tab", opencode_i18n::key!("command.tab.close"), |s, tx| {
        if let Some(id) = s.current_tab() {
            s.close_tab(&id, tx);
        }
    })
    .keys(Bind::TabClose)
    .when(|s| s.current_tab().is_some())
    // `titlebar.tsx:453-458`.
    .hidden()
    .order(400),
    // `titlebar.tsx:480-486`: the same close, listed.
    Cmd::new("close-session-tab", opencode_i18n::key!("command.tab.closeSession"), |s, tx| {
        if let Some(id) = s.current_tab() {
            s.close_tab(&id, tx);
        }
    })
    .keys(Bind::TabCloseSession)
    .order(410)
    .when(|s| s.current_tab().is_some()),
    Cmd::new("reopen-tab", opencode_i18n::key!("command.tab.reopenClosed"), |s, tx| s.reopen_tab(tx)).keys(Bind::TabReopen).order(420),
    // Beyond Desktop, which forgets a closed window: the newest one kept opens again.
    Cmd::new("reopen-window", opencode_i18n::key!("rune.command.window.reopenClosed"), |s, _| crate::windows::reopen::reopen_newest(s)).order(430),
    Cmd::new("zoom-in", opencode_i18n::key!("desktop.menu.zoomIn"), |s, tx| s.zoom_by(1, tx)).keys(Bind::ZoomIn).order(440),
    Cmd::new("zoom-out", opencode_i18n::key!("desktop.menu.zoomOut"), |s, tx| s.zoom_by(-1, tx)).keys(Bind::ZoomOut).order(450),
    Cmd::new("zoom-reset", opencode_i18n::key!("desktop.menu.actualSize"), |s, tx| s.zoom_by(0, tx)).keys(Bind::ZoomReset).order(460),
]);

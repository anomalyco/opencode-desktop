//! The app's commands, one registry (`s.kernel.commands`) that ⌘K, ⇧⌘P, Settings › Shortcuts,
//! and the composer's `/` all read, as Desktop's `command.register` feeds its palette and slash
//! popover (`shell/commands/command.tsx`, `session/commands/use-session-commands.tsx`,
//! `composer/commands.tsx`).
//!
//! Features register their commands in `Feature::register` via
//! `builder.add_command(CommandEntry::from_key(id, key, order::..., run)...)`; core shell and
//! window commands register in [`register_shell_commands`]. Titles and descriptions are catalog
//! keys, read in the current language as the list is made.

pub mod order;
pub mod type_to_compose;

use opencode_kernel::{CommandEntry, KernelBuilder};
use opencode_kit::client::Write;
use rune::{KeyEvent, Tx};

use crate::App;
use crate::keymap::{Bind, Section};
use crate::pickers::PickerHost as _;

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
    /// A command's default chord when it has no static keymap entry (for example a plugin or
    /// `.keybind(...)` feature command).
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
    /// Every enabled command: built-ins in canonical palette order, then plugins.
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
        let mut all: Vec<(i64, Command)> = self
            .kernel
            .commands
            .iter_with_chords()
            .filter(|(c, _)| !offered || c.is_enabled(self))
            .map(|(c, chord)| {
                let keys = c.bind_id.and_then(Bind::by_id);
                (
                    i64::from(c.order),
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
            })
            .chain(crate::plugin_host::commands(self, offered).into_iter().map(|(order, cmd)| (order.unwrap_or(i64::MAX), cmd)))
            .collect();
        all.sort_by_key(|(order, _)| *order);
        all.into_iter().map(|(_, cmd)| cmd).collect()
    }

    pub fn run_command(&mut self, id: &str, tx: &mut Tx) {
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
        self.kernel.slots.get_mut(&crate::find::FIND).input.focused = false;
        self.composer.focused = true;
        true
    }

    pub(crate) fn copy_id(&mut self, id: Option<String>, tx: &mut Tx) {
        let Some(id) = id.filter(|i| !i.is_empty()) else { return };
        tx.copy(id);
        *self.diag.status = opencode_i18n::t!("common.copied").into();
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

/// Runs the enabled command bound to `bind` in `s.kernel.commands`, returning `true` if one ran.
pub fn run_bind(s: &mut App, bind: Bind, tx: &mut Tx) -> bool {
    let Some(entry) = s.kernel.commands.find_bind(bind.id(), s) else {
        return false;
    };
    (entry.run)(s, tx);
    true
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

/// Dispatches the six keymap-bound commands ahead of plugin shortcuts (`KEY_BINDS`), plus any
/// kernel command with a default `.keybind(...)` that has no Settings › Shortcuts override.
///
/// Key dispatch rule for duplicate chords (matching pre-`6a8c7eb` behaviour):
/// - User overrides for non-table commands (`overrides::commands()`) are dispatched once, earlier
///   in `App::shortcut`, by `settings::pages::shortcuts::run_bound`. The first matching override
///   decides: if its command is disabled or unknown, `run_bound` declines the key and
///   `commands::key` never re-dispatches `overrides::command(id)` or fires a fallback default
///   `.keybind(...)` for that chord.
/// - Among `KEY_BINDS`, the first matching `Bind` decides (as in `shortcuts::run`); if its
///   command is disabled, no second `KEY_BINDS` entry fires for the same key.
/// - Among un-overridden default `.keybind(...)` commands, the first matching command decides; if
///   it is disabled, no second default command fires for the same key.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if let Some(bind) = KEY_BINDS.into_iter().find(|b| b.matches_tx(ev, tx)) {
        return run_bind(s, bind, tx);
    }
    if crate::keymap::overrides::commands().iter().any(|(_, kb)| kb.matches_tx(ev, tx)) {
        return false;
    }
    if let Some(entry) = s
        .kernel
        .commands
        .find_default_key(s, ev, tx, |id| crate::keymap::overrides::command(id).is_some())
    {
        (entry.run)(s, tx);
        return true;
    }
    false
}

fn is_live(s: &App) -> bool {
    matches!(*s.source, crate::Source::Live { .. })
}

/// `/reload` (`composer/commands.tsx:45-84`): the server rebuilds every location, and the app
/// says so while it does and once it has.
fn reload(s: &mut App, _: &mut Tx) {
    *s.diag.status = opencode_i18n::t!("command.config.reload.pending").into();
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

/// Registers core shell, navigation, tab, zoom, and window commands into `b`.
pub fn register_shell_commands(b: &mut KernelBuilder<App>) {
    // `session.new` has no keybind of its own; it runs `tab.new` (`use-session-commands.tsx:446-451`).
    b.add_command(
        CommandEntry::<App>::from_key("new", opencode_i18n::key!("command.session.new"), order::NEW, |s, tx| s.new_session(tx))
            .slash("new")
            .section("session"),
    );
    // `titlebar.tsx:428-434`: hidden from the palette; its key and Settings keep it.
    b.add_command(
        CommandEntry::<App>::from_key("home", opencode_i18n::key!("home.title"), order::HOME, |s, tx| s.go_home(tx))
            .bind_id(Bind::Home.id())
            .hidden(),
    );
    // `titlebar.tsx:148-160`.
    b.add_command(
        CommandEntry::<App>::from_key("back", opencode_i18n::key!("common.goBack"), order::BACK, |s, tx| {
            s.navigate(false, tx);
        })
        .bind_id(Bind::GoBack.id()),
    );
    b.add_command(
        CommandEntry::<App>::from_key("forward", opencode_i18n::key!("common.goForward"), order::FORWARD, |s, tx| {
            s.navigate(true, tx);
        })
        .bind_id(Bind::GoForward.id()),
    );
    b.add_command(
        CommandEntry::<App>::from_key("sidebar", opencode_i18n::key!("command.sidebar.toggle"), order::SIDEBAR, toggle_sidebar)
            .bind_id(Bind::Sidebar.id())
            .when(has_sidebar),
    );
    b.add_command(
        CommandEntry::<App>::from_key("reload", opencode_i18n::key!("command.config.reload"), order::RELOAD, reload)
            .describe(opencode_i18n::key!("command.config.reload.description"))
            .slash("reload")
            .when(is_live),
    );
    b.add_command(
        CommandEntry::<App>::from_key("compact", opencode_i18n::key!("command.session.compact"), order::COMPACT, compact)
            .describe(opencode_i18n::key!("command.session.compact.description"))
            .slash("compact")
            .section("session")
            .when(App::in_root_session),
    );
    b.add_command(
        CommandEntry::<App>::from_key("next-tab", opencode_i18n::key!("command.session.next"), order::NEXT_TAB, |s, tx| {
            s.select_adjacent(1, tx);
        })
        .bind_id(Bind::SessionNext.id()),
    );
    b.add_command(
        CommandEntry::<App>::from_key("previous-tab", opencode_i18n::key!("command.session.previous"), order::PREVIOUS_TAB, |s, tx| {
            s.select_adjacent(-1, tx);
        })
        .bind_id(Bind::SessionPrev.id()),
    );
    b.add_command(
        CommandEntry::<App>::from_key(
            "previous-unread",
            opencode_i18n::key!("command.session.previous.unseen"),
            order::PREVIOUS_UNREAD,
            |s, tx| s.select_unread(-1, tx),
        )
        .bind_id(Bind::UnseenPrev.id()),
    );
    b.add_command(
        CommandEntry::<App>::from_key(
            "next-unread",
            opencode_i18n::key!("command.session.next.unseen"),
            order::NEXT_UNREAD,
            |s, tx| s.select_unread(1, tx),
        )
        .bind_id(Bind::UnseenNext.id()),
    );
    b.add_command(
        CommandEntry::<App>::from_key("close-tab", opencode_i18n::key!("command.tab.close"), order::CLOSE_TAB, |s, tx| {
            if let Some(id) = s.current_tab() {
                s.close_tab(&id, tx);
            }
        })
        .bind_id(Bind::TabClose.id())
        .when(|s| s.current_tab().is_some())
        // `titlebar.tsx:453-458`.
        .hidden(),
    );
    // `titlebar.tsx:480-486`: the same close, listed.
    b.add_command(
        CommandEntry::<App>::from_key(
            "close-session-tab",
            opencode_i18n::key!("command.tab.closeSession"),
            order::CLOSE_SESSION_TAB,
            |s, tx| {
                if let Some(id) = s.current_tab() {
                    s.close_tab(&id, tx);
                }
            },
        )
        .bind_id(Bind::TabCloseSession.id())
        .when(|s| s.current_tab().is_some()),
    );
    b.add_command(
        CommandEntry::<App>::from_key("reopen-tab", opencode_i18n::key!("command.tab.reopenClosed"), order::REOPEN_TAB, |s, tx| {
            s.reopen_tab(tx);
        })
        .bind_id(Bind::TabReopen.id()),
    );
    // Beyond Desktop, which forgets a closed window: the newest one kept opens again.
    #[cfg(not(target_arch = "wasm32"))]
    b.add_command(CommandEntry::<App>::from_key(
        "reopen-window",
        opencode_i18n::key!("rune.command.window.reopenClosed"),
        order::REOPEN_WINDOW,
        |s, _| crate::windows::reopen::reopen_newest(s),
    ));
    b.add_command(
        CommandEntry::<App>::from_key("zoom-in", opencode_i18n::key!("desktop.menu.zoomIn"), order::ZOOM_IN, |s, tx| s.zoom_by(1, tx))
            .bind_id(Bind::ZoomIn.id()),
    );
    b.add_command(
        CommandEntry::<App>::from_key("zoom-out", opencode_i18n::key!("desktop.menu.zoomOut"), order::ZOOM_OUT, |s, tx| s.zoom_by(-1, tx))
            .bind_id(Bind::ZoomOut.id()),
    );
    b.add_command(
        CommandEntry::<App>::from_key("zoom-reset", opencode_i18n::key!("desktop.menu.actualSize"), order::ZOOM_RESET, |s, tx| s.zoom_by(0, tx))
            .bind_id(Bind::ZoomReset.id()),
    );
    // Desktop's `logs.export` (`shell/commands/desktop.tsx`), beside the app menu's Export Logs….
    #[cfg(not(target_arch = "wasm32"))]
    b.add_command(CommandEntry::<App>::from_key(
        "logs.export",
        opencode_i18n::key!("command.logs.export"),
        order::LOGS_EXPORT,
        |s, _| s.export_logs(),
    ));
}

//! Every shortcut the app binds, in one table: a command id, its title, and a `rune::Keybind`
//! with the chords each platform uses, as Desktop's command registry keeps them
//! (`shell/commands/command.tsx` with each feature's `keybind:`, and the GUI plugins' `bind:`).
//! Key handlers ask the table (`Bind::matches`), labels come from it (`Bind::label`,
//! `Bind::keycaps`), and Settings lists it. `mod` is ⌘ on macOS and Ctrl elsewhere.
//!
//! Off macOS `ctrl+x` and `mod+x` are the same keys, so where Desktop gives a platform its own
//! chord to keep two commands apart, so does this table (`Keybind::on`).

use std::sync::LazyLock;

use rune::{Key, KeyEvent, KeyPlatform, Keybind, Mods, Tx};

pub mod base;
pub mod overrides;
pub mod sequence;
pub mod view;

pub use base::{caps, label, sentence};
pub use rune::Chord;
pub use view::{LABEL, chips, keycaps};

use KeyPlatform::{Linux, Mac, Windows};

/// Host command catalog hook for resolving default chords of commands outside the static
/// [`Bind`] table (such as registered shell or plugin commands).
///
/// Object-safe so callers can pass `&dyn KeymapHost` (or `&App` once `App: KeymapHost`)
/// without monomorphizing keymap helpers per host crate.
pub trait KeymapHost {
    /// The default chord for published command `id` when no user override is installed
    /// (for example, a registered command's [`Bind::chord`] or a plugin-declared chord).
    fn default_command_chord(&self, id: &str) -> Option<Chord>;
}

/// One bound command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bind {
    Palette,
    CommandSearch,
    SessionSearch,
    Settings,
    Home,
    Sidebar,
    SideToggle,
    OpenProject,
    SelectProject,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    AutoAccept,
    FocusInput,
    TabNew,
    TabClose,
    TabCloseSession,
    TabReopen,
    TabPrev,
    TabNext,
    SessionPrev,
    SessionNext,
    UnseenPrev,
    UnseenNext,
    JumpToMessage,
    MessagePrev,
    MessageNext,
    Tab1,
    Tab2,
    Tab3,
    Tab4,
    Tab5,
    Tab6,
    Tab7,
    Tab8,
    Tab9,
    GoBack,
    GoForward,
    Find,
    FindNext,
    FindPrev,
    Background,
    Rename,
    ArchiveSession,
    Model,
    Agent,
    AgentBack,
    Variant,
    Attach,
    Dictate,
    SubmitAlternate,
    SubmitFirst,
    PastePlain,
    SubmitAnswer,
    Stop,
    FileOpen,
    BottomToggle,
    PaneTabNew,
    PaneTabPrev,
    PaneTabNext,
    TreeToggle,
    CopySelection,
    TerminalCopy,
    TerminalPaste,
    TerminalSelectAll,
    TerminalKillLine,
    Knobs,
    Rewind,
    DemoReplay,
    DemoPause,
    WindowNew,
    WindowClose,
    McpToggle,
    BranchChoose,
    LocationSelect,
    LocationCycle,
    AddSelection,
    QuestionBack,
    SettingsSearch,
    PromptModeToggle,
}

/// Settings > Shortcuts' groups (Desktop's `CommandSection`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    General,
    Session,
    Navigation,
    Models,
    Terminal,
    Prompt,
}

impl Section {
    pub const ALL: [Section; 6] = [Section::General, Section::Session, Section::Navigation, Section::Models, Section::Terminal, Section::Prompt];

    pub fn title(self) -> &'static str {
        match self {
            Self::General => opencode_i18n::t!("settings.shortcuts.group.general"),
            Self::Session => opencode_i18n::t!("settings.shortcuts.group.session"),
            Self::Navigation => opencode_i18n::t!("settings.shortcuts.group.navigation"),
            Self::Models => opencode_i18n::t!("settings.shortcuts.group.modelAndAgent"),
            Self::Terminal => opencode_i18n::t!("settings.shortcuts.group.terminal"),
            Self::Prompt => opencode_i18n::t!("settings.shortcuts.group.prompt"),
        }
    }
}

/// A row of the table.
pub struct Binding {
    pub bind: Bind,
    /// Desktop's command id, for a settings file.
    pub id: &'static str,
    /// Its title's key (Desktop's command title); see [`Binding::title`].
    pub title_key: opencode_i18n::Key,
    pub section: Section,
    pub keybind: Keybind,
    /// Settings › Shortcuts lists it, as Desktop's catalog does every command with a title.
    pub listed: bool,
    /// Acts only in its own surface (a pane, a dock, Settings) or is a developer tool: a pane
    /// that types keys (the terminal) keeps its chord.
    pub scoped: bool,
}

static TABLE: LazyLock<Vec<Binding>> = LazyLock::new(table);

fn table() -> Vec<Binding> {
    use Bind::*;
    use Section::{General, Models, Navigation, Prompt, Session, Terminal};
    use opencode_i18n::key;
    let on = |kb: &str| Keybind::new(kb);
    // The shared view shortcuts (`keymap::base`), listed here so Settings shows them.
    let view = |b: base::Bind| b.keybind().clone();
    let rows = vec![
        // `DEFAULT_PALETTE_KEYBIND` (`command.tsx:22`): ⇧⌘P opens the palette too, and Search
        // commands has no chord of its own (`shell.tsx:196-203`).
        row(Palette, "command.palette", key!("command.palette"), General, on("mod+k, mod+shift+p")),
        row(CommandSearch, "commands.search", key!("command.commands.search"), General, on("none")),
        row(SessionSearch, "sessions.search", key!("command.session.search"), General, on("mod+shift+f")),
        row(Settings, "settings.open", key!("command.settings.open"), General, on("mod+comma")),
        // `titlebar.tsx`: Windows keeps Ctrl+B for Move to background.
        row(Home, "home.toggle", key!("home.title"), General, on("mod+b").on(Windows, "alt+home")),
        row(Sidebar, "sidebar.toggle", key!("command.sidebar.toggle"), General, on("mod+shift+b")),
        // ⌥⌘B is Ctrl+Alt+B off macOS, the Browser's launcher; there only `review.toggle`'s chord.
        row(SideToggle, "review.toggle", key!("command.review.toggle"), Session, on("mod+alt+b, mod+shift+r").on(Windows, "mod+shift+r").on(Linux, "mod+shift+r")),
        row(OpenProject, "project.open", key!("command.project.open"), General, view(base::Bind::OpenProject)),
        row(SelectProject, "project.select", key!("session.new.project.search"), General, on("mod+shift+o")),
        // The app menu's View items: Desktop's Settings has no row for them.
        unlisted(ZoomIn, "view.zoomIn", key!("desktop.menu.zoomIn"), General, view(base::Bind::ZoomIn)),
        unlisted(ZoomOut, "view.zoomOut", key!("desktop.menu.zoomOut"), General, view(base::Bind::ZoomOut)),
        unlisted(ZoomReset, "view.resetZoom", key!("desktop.menu.actualSize"), General, view(base::Bind::ZoomReset)),
        // Desktop's title over a project's session, which its catalog keeps (`use-session-commands.tsx:666-677`).
        row(AutoAccept, "permissions.autoaccept", key!("command.permissions.autoaccept.project"), Session, on("mod+shift+a")),
        row(FocusInput, "input.focus", key!("command.input.focus"), General, on("ctrl+l")),
        row(TabNew, "tab.new", key!("command.session.new"), General, on("mod+t, mod+n")),
        row(TabClose, "tab.close", key!("command.tab.close"), General, on("mod+w")),
        // `titlebar.tsx:480-486`: the session tab, even while a pane's tab would take Ctrl+W.
        row(TabCloseSession, "tab.closeSession", key!("command.tab.closeSession"), General, on("mod+alt+w")),
        row(TabReopen, "tab.reopenClosed", key!("command.tab.reopenClosed"), General, on("mod+shift+t")),
        hidden(TabPrev, "tab.prev", key!("rune.command.tab.previous"), Navigation, on("mod+alt+left, ctrl+shift+tab")),
        hidden(TabNext, "tab.next", key!("rune.command.tab.next"), Navigation, on("mod+alt+right, ctrl+tab")),
        row(SessionPrev, "session.previous", key!("command.session.previous"), Session, on("alt+up")),
        row(SessionNext, "session.next", key!("command.session.next"), Session, on("alt+down")),
        row(UnseenPrev, "session.previous.unseen", key!("command.session.previous.unseen"), Session, on("shift+alt+up")),
        row(UnseenNext, "session.next.unseen", key!("command.session.next.unseen"), Session, on("shift+alt+down")),
        // `use-session-commands.tsx:674-681`.
        row(JumpToMessage, "session.jumpToMessage", key!("dialog.jump.title"), Session, on("mod+alt+g")),
        // `use-session-commands.tsx:636-653`.
        row(MessagePrev, "message.previous", key!("command.message.previous"), Session, on("mod+alt+[")),
        row(MessageNext, "message.next", key!("command.message.next"), Session, on("mod+alt+]")),
        hidden(Tab1, "tab.1", key!("rune.command.tab.goTo"), Navigation, on("mod+1")),
        hidden(Tab2, "tab.2", key!("rune.command.tab.goTo"), Navigation, on("mod+2")),
        hidden(Tab3, "tab.3", key!("rune.command.tab.goTo"), Navigation, on("mod+3")),
        hidden(Tab4, "tab.4", key!("rune.command.tab.goTo"), Navigation, on("mod+4")),
        hidden(Tab5, "tab.5", key!("rune.command.tab.goTo"), Navigation, on("mod+5")),
        hidden(Tab6, "tab.6", key!("rune.command.tab.goTo"), Navigation, on("mod+6")),
        hidden(Tab7, "tab.7", key!("rune.command.tab.goTo"), Navigation, on("mod+7")),
        hidden(Tab8, "tab.8", key!("rune.command.tab.goTo"), Navigation, on("mod+8")),
        hidden(Tab9, "tab.9", key!("rune.command.tab.goTo"), Navigation, on("mod+9")),
        row(GoBack, "common.goBack", key!("common.goBack"), General, on("mod+[")),
        row(GoForward, "common.goForward", key!("common.goForward"), General, on("mod+]")),
        // The timeline's search (`timeline/search-controller.ts:189-196`); its next and previous
        // match are the bar's buttons in Desktop, not commands.
        row(Find, "session.search", key!("session.search.placeholder"), Session, on("mod+f")),
        unlisted(FindNext, "session.find.next", key!("ui.fileSearch.nextMatch"), Session, on("mod+g")),
        unlisted(FindPrev, "session.find.previous", key!("ui.fileSearch.previousMatch"), Session, on("mod+shift+g")),
        scoped(Background, "session.background", key!("command.session.background"), Session, on("ctrl+b")),
        scoped(Rename, "session.rename", key!("command.session.rename"), Session, on("ctrl+r")),
        // `use-session-commands.tsx` `session.archive` (opencode#65428).
        row(ArchiveSession, "session.archive", key!("command.session.archive"), Session, on("mod+shift+s")),
        row(Model, "model.choose", key!("command.model.choose"), Models, view(base::Bind::Model)),
        row(Agent, "agent.cycle", key!("command.agent.cycle"), Models, view(base::Bind::Agent)),
        row(AgentBack, "agent.cycle.reverse", key!("command.agent.cycle.reverse"), Models, view(base::Bind::AgentBack)),
        row(Variant, "model.variant.cycle", key!("command.model.variant.cycle"), Models, view(base::Bind::Variant)),
        // The composer's `file.attach` (`composer/model.ts:570-578`).
        row(Attach, "file.attach", key!("prompt.action.attachFile"), Navigation, view(base::Bind::Attach)),
        // Dictation through Hex is this app's own; Desktop has no such command.
        row(Dictate, "prompt.dictate", key!("rune.command.prompt.dictate"), Prompt, on("mod+shift+m")),
        // The composer's own keys, which Desktop's Settings doesn't list.
        unlisted(SubmitAlternate, "prompt.submit.alternate", key!("rune.command.prompt.submitAlternate"), Prompt, on("mod+enter, mod+shift+enter")),
        unlisted(SubmitFirst, "prompt.submit.first", key!("session.queue.first"), Prompt, on("alt+enter")),
        // Chromium's paste as plain text, which Desktop's fields get from the browser; Rune leaves
        // Ctrl+Shift+V to the app. ⇧⌘V pasted here on macOS before, and still does.
        hidden(PastePlain, "edit.pastePlain", key!("rune.command.edit.pastePlain"), Prompt, on("mod+shift+v")),
        unlisted(SubmitAnswer, "question.submit", key!("rune.command.question.submit"), Prompt, view(base::Bind::QuestionAdvance)),
        // The composer's `prompt.stop` (`composer/model.ts`, opencode#64394): Esc and ⌃G while the
        // session runs, one keybind Settings can change. Saved as `session.stop` before (`RENAMES`).
        row(Stop, "prompt.stop", key!("command.prompt.stop"), Session, view(base::Bind::Stop)),
        row(FileOpen, "file.open", key!("command.file.open"), Navigation, on("mod+p")),
        // The browser's commands (`browser.open` and its pane's) are the browser plugin's.
        // Desktop's `terminal.toggle` (Ctrl+`) and `terminal.new` are the terminal plugin's
        // (`TERMINAL_TOGGLE`, `TERMINAL_NEW`).
        row(BottomToggle, "bottom.toggle", key!("command.bottom.toggle"), General, on("mod+j")),
        hidden(PaneTabNew, "file.tab.new", key!("rune.command.file.tab.new"), Navigation, on("mod+t")),
        hidden(PaneTabPrev, "file.tab.prev", key!("rune.command.file.tab.previous"), Navigation, on("mod+shift+[")),
        hidden(PaneTabNext, "file.tab.next", key!("rune.command.file.tab.next"), Navigation, on("mod+shift+]")),
        hidden(TreeToggle, "fileTree.toggle", key!("ui.sessionReviewV2.toggleSidebar"), Session, on("mod+\\")),
        hidden(CopySelection, "edit.copy", key!("ui.message.copy"), General, on("mod+c")),
        // The terminal's own keys (`terminal-key-event.ts`, `terminal.tsx`): ⌘ chords on macOS;
        // elsewhere Ctrl belongs to the shell, so copy and paste add Shift.
        hidden(TerminalCopy, "terminal.copy", key!("ui.message.copy"), Terminal, on("super+c, ctrl+shift+c")),
        hidden(TerminalPaste, "terminal.paste", key!("desktop.menu.paste"), Terminal, on("super+v").on(Windows, "ctrl+shift+v, super+v").on(Linux, "ctrl+shift+v, super+v")),
        hidden(TerminalSelectAll, "terminal.selectAll", key!("desktop.menu.selectAll"), Terminal, on("super+a")),
        hidden(TerminalKillLine, "terminal.killLine", key!("rune.command.terminal.killLine"), Terminal, on("super+backspace")),
        hidden(Knobs, "dev.knobs", key!("rune.command.dev.knobs"), General, on("mod+shift+k")),
        hidden(Rewind, "dev.rewind", key!("rune.command.dev.rewind"), General, on("mod+alt+shift+r")),
        // Ctrl+Alt+R and Ctrl+Alt+P are Review and Files off macOS.
        hidden(DemoReplay, "dev.demo.replay", key!("rune.command.dev.demoReplay"), General, on("mod+alt+r").on(Windows, "ctrl+alt+enter").on(Linux, "ctrl+alt+enter")),
        hidden(DemoPause, "dev.demo.pause", key!("rune.command.dev.demoPause"), General, on("mod+alt+p").on(Windows, "ctrl+alt+space").on(Linux, "ctrl+alt+space")),
        // The app menu's accelerators off macOS (`desktop-menu.ts`), which no command binds.
        hidden(WindowNew, "window.new", key!("desktop.menu.newWindow"), General, on("ctrl+shift+n").on(Mac, "none")),
        hidden(WindowClose, "window.close", key!("desktop.menu.closeWindow"), General, on("ctrl+shift+w").on(Mac, "none")),
        row(McpToggle, "mcp.toggle", key!("command.mcp.toggle"), Models, on("mod+semicolon")),
        // The new-session page's (`new-session/{workspace/selector,view,commands}.tsx`). Off macOS
        // Ctrl+Alt+B is also the Browser's launcher, which only a session has.
        row(BranchChoose, "session.branch.choose", key!("command.session.branch.choose"), Session, on("mod+alt+b")),
        row(LocationSelect, "session.location.select", key!("session.location.worktree"), Session, on("mod+alt+o")),
        row(LocationCycle, "session.location.cycle", key!("command.session.location.cycle"), Session, on("mod+alt+l")),
        // The open file's selected lines to the composer (`use-session-commands.tsx:597-606`).
        row(AddSelection, "context.addSelection", key!("command.context.addSelection"), General, on("mod+shift+l")),
        // The question dock's previous question and Settings' search, each while it shows.
        scoped(QuestionBack, "session.question.back", key!("ui.common.back"), Session, view(base::Bind::QuestionBack)),
        scoped(SettingsSearch, "settings.search.focus", key!("settings.search.placeholder"), General, on("mod+f")),
        // The composer's shell mode (`composer/model.ts:579-585`).
        row(PromptModeToggle, "prompt.mode.toggle", key!("command.prompt.mode.toggle"), Prompt, on("mod+shift+x")),
    ];
    assert!(rows.iter().enumerate().all(|(i, r)| r.bind as usize == i), "keymap rows follow `Bind`'s order");
    rows
}

fn row(bind: Bind, id: &'static str, title_key: opencode_i18n::Key, section: Section, keybind: Keybind) -> Binding {
    Binding { bind, id, title_key, section, keybind, listed: true, scoped: false }
}

/// Listed, but acting only in its own surface.
fn scoped(bind: Bind, id: &'static str, title_key: opencode_i18n::Key, section: Section, keybind: Keybind) -> Binding {
    Binding { scoped: true, ..row(bind, id, title_key, section, keybind) }
}

/// Window-wide, but not a command Desktop's Settings lists.
fn unlisted(bind: Bind, id: &'static str, title_key: opencode_i18n::Key, section: Section, keybind: Keybind) -> Binding {
    Binding { listed: false, ..row(bind, id, title_key, section, keybind) }
}

/// Scoped to a pane or a developer tool, and not listed.
fn hidden(bind: Bind, id: &'static str, title_key: opencode_i18n::Key, section: Section, keybind: Keybind) -> Binding {
    Binding { listed: false, scoped: true, ..row(bind, id, title_key, section, keybind) }
}

impl Binding {
    /// The command's title in the current language; the tab numbers name their tab.
    pub fn title(&self) -> String {
        match Bind::TABS.iter().position(|b| *b == self.bind) {
            Some(at) => opencode_i18n::I18N.format(self.title_key, &[("number", &(at + 1))]),
            None => opencode_i18n::I18N.text(self.title_key).to_owned(),
        }
    }
}

/// The whole table, in `Bind` order.
pub fn all() -> &'static [Binding] {
    &TABLE
}

/// The label of the first of `binds` bound here, or "".
pub fn first_label(binds: &[Bind]) -> String {
    binds.iter().map(|b| b.label()).find(|l| !l.is_empty()).unwrap_or_default()
}

/// Whether the app takes `ev` from a focused pane that would otherwise type it (the terminal): a
/// window-wide command's chord. Scoped ones stay with the pane: Rename, which Desktop leaves to
/// plugin panes, Move to background, which acts only while something blocks, and the dock's,
/// Settings' and the browser pane's own.
pub fn takes_from_pane(ev: &KeyEvent, tx: &Tx) -> bool {
    hits(ev, tx).any(|b| !b.binding().scoped)
}

/// Whether `m` holds the logo key: ⌘ on macOS, where it is the primary key; Win or Super elsewhere.
pub fn logo_held(m: Mods) -> bool {
    m.logo || (m.cmd && KeyPlatform::current() == Mac)
}

/// A modifier's cap alone, for a hint about holding it: "⌥" on macOS, "Alt" elsewhere.
pub fn modifier_label(m: rune::ChordMod) -> String {
    Chord::new(rune::ChordKey::Char('x')).with(m).keycaps_on(KeyPlatform::current()).swap_remove(0)
}

/// The bindings `ev` is, here.
pub fn hits<'a>(ev: &'a KeyEvent, tx: &'a Tx) -> impl Iterator<Item = Bind> + 'a {
    all().iter().filter(move |b| b.bind.matches_tx(ev, tx)).map(|b| b.bind)
}

impl Bind {
    const TABS: [Bind; 9] = [Bind::Tab1, Bind::Tab2, Bind::Tab3, Bind::Tab4, Bind::Tab5, Bind::Tab6, Bind::Tab7, Bind::Tab8, Bind::Tab9];

    pub fn binding(self) -> &'static Binding {
        &TABLE[self as usize]
    }

    /// This binding's canonical command id in the keymap table (for example `"settings.open"`).
    pub fn id(self) -> &'static str {
        self.binding().id
    }

    /// Looks up a keymap table entry by its canonical command id.
    pub fn by_id(id: &str) -> Option<Bind> {
        all().iter().find(|b| b.id == id).map(|b| b.bind)
    }

    /// The chords bound here: Settings › Shortcuts' when changed there, else the default.
    pub fn keybind(self) -> &'static Keybind {
        overrides::get(self).unwrap_or(&self.binding().keybind)
    }

    /// The chords before Settings › Shortcuts changes them.
    pub fn default_keybind(self) -> &'static Keybind {
        &self.binding().keybind
    }

    /// Whether `ev` is one of this command's chords here; see `Keybind::matches`. While a key
    /// chord's command is fired (`sequence::with_fired`), whether this is that command.
    pub fn matches(self, ev: &KeyEvent) -> bool {
        if let Some(fired) = sequence::fired_is(self.id()) {
            return fired;
        }
        self.keybind().matches(ev) || control_char(ev).is_some_and(|ev| self.keybind().matches(&ev))
    }

    /// `matches`, following the keyboard layout (`Keybind::matches_tx`); use it in key handlers.
    pub fn matches_tx(self, ev: &KeyEvent, tx: &Tx) -> bool {
        if let Some(fired) = sequence::fired_is(self.id()) {
            return fired;
        }
        self.keybind().matches_tx(ev, tx) || control_char(ev).is_some_and(|ev| self.keybind().matches(&ev))
    }

    /// The tab-number command `ev` is: 0 for the first tab, 8 for the last.
    pub fn tab_at(ev: &KeyEvent, tx: &Tx) -> Option<usize> {
        Self::TABS.iter().position(|b| b.matches_tx(ev, tx))
    }

    /// How the first chord reads here: "⇧⌘T" on macOS, "Ctrl+Shift+T" elsewhere, and a chord of
    /// several strokes with a space between them ("Ctrl+K Ctrl+C"). "" when unbound.
    pub fn label(self) -> String {
        match sequence::first(self.id()) {
            Some(strokes) => sequence::label(&strokes),
            None => self.chord().map_or_else(String::new, label),
        }
    }

    /// One string per keycap, modifiers first, as Desktop's keybind chips draw it; a chord of
    /// several strokes gives one per stroke.
    pub fn keycaps(self) -> Vec<String> {
        match sequence::first(self.id()) {
            Some(strokes) => sequence::caps(&strokes),
            None => self.chord().map_or_else(Vec::new, caps),
        }
    }

    /// The first chord here, if any.
    pub fn chord(self) -> Option<Chord> {
        self.keybind().chords().first().copied()
    }

    /// The first chord as a menu item's key equivalent.
    pub fn shortcut(self) -> Option<rune::Shortcut> {
        self.chord().and_then(|c| c.to_string().parse().ok())
    }

    /// `text` with this command's label after it in parentheses, for a tooltip: "Reload page (⌘R)".
    pub fn tip(self, text: &str) -> String {
        let keys = self.label();
        if keys.is_empty() { text.to_owned() } else { format!("{text} ({keys})") }
    }
}

/// A built-in plugin's command that the shell's own menus and key routing name: its id and the
/// chord its plugin binds it to, which Settings › Shortcuts can change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginBind {
    pub id: &'static str,
    pub default: &'static str,
}

/// The terminal plugin's `terminal.toggle` (`plugins/terminal`).
pub const TERMINAL_TOGGLE: PluginBind = PluginBind { id: "terminal.toggle", default: "ctrl+`" };
/// The terminal plugin's `terminal.new`.
pub const TERMINAL_NEW: PluginBind = PluginBind { id: "terminal.new", default: "ctrl+alt+t" };
/// The browser plugin's `browser.reloadPage`, the system browser's reload inside its pane.
pub const BROWSER_RELOAD_PAGE: PluginBind = PluginBind { id: "browser.reloadPage", default: "mod+r" };

impl PluginBind {
    fn keybind(self) -> Keybind {
        overrides::command(self.id).cloned().unwrap_or_else(|| Keybind::new(self.default))
    }

    /// The first chord here, if any.
    pub fn chord(self) -> Option<Chord> {
        self.keybind().chords().first().copied()
    }

    /// How the first chord reads here; "" when unbound.
    pub fn label(self) -> String {
        match sequence::first(self.id) {
            Some(strokes) => sequence::label(&strokes),
            None => self.chord().map_or_else(String::new, label),
        }
    }

    /// One string per keycap, modifiers first; one per stroke for a chord of several.
    pub fn keycaps(self) -> Vec<String> {
        match sequence::first(self.id) {
            Some(strokes) => sequence::caps(&strokes),
            None => self.chord().map_or_else(Vec::new, caps),
        }
    }

    /// The first chord as a menu item's key equivalent.
    pub fn shortcut(self) -> Option<rune::Shortcut> {
        self.chord().and_then(|c| c.to_string().parse().ok())
    }

    /// Whether `ev` is one of its chords here.
    pub fn matches(self, ev: &KeyEvent) -> bool {
        sequence::fired_is(self.id).unwrap_or_else(|| self.keybind().matches(ev))
    }
}

/// The chord published command `id` answers to here: the user's own override, else the host's
/// default from [`KeymapHost::default_command_chord`].
pub fn command_chord(host: &dyn KeymapHost, id: &str) -> Option<Chord> {
    overrides::command(id)
        .and_then(|k| k.chords().first().copied())
        .or_else(|| host.default_command_chord(id))
}

/// A Ctrl-letter that arrives as its control character (⌃R as U+0012), as the letter.
fn control_char(ev: &KeyEvent) -> Option<KeyEvent> {
    let Key::Text(t) = &ev.key else { return None };
    let mut chars = t.chars();
    let c = chars.next().filter(|c| ('\u{1}'..='\u{1a}').contains(c) && chars.next().is_none())?;
    let letter = char::from(b'a' + (c as u8) - 1);
    ev.mods.ctrl.then(|| KeyEvent { key: Key::Text(letter.to_string()), mods: ev.mods })
}

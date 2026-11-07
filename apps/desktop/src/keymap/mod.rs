//! Every shortcut the app binds, in one table: a command id, its title, and a `rune::Keybind`
//! with the chords each platform uses, as Desktop's command registry keeps them
//! (`shell/commands/command.tsx` with each feature's `keybind:`, and the GUI plugins' `bind:`).
//! Key handlers ask the table (`Bind::matches`), labels come from it (`Bind::label`,
//! `Bind::keycaps`), and Settings lists it. `mod` is ⌘ on macOS and Ctrl elsewhere.
//!
//! Off macOS `ctrl+x` and `mod+x` are the same keys, so where Desktop gives a platform its own
//! chord to keep two commands apart, so does this table (`Keybind::on`).

use std::sync::LazyLock;

use rune::{Chord, Key, KeyEvent, KeyPlatform, Keybind, Mods, Tx};

mod view;
pub use view::{keycaps, settings_section};

use KeyPlatform::{Linux, Mac, Windows};

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
    TabReopen,
    TabPrev,
    TabNext,
    SessionPrev,
    SessionNext,
    UnseenPrev,
    UnseenNext,
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
    Model,
    Agent,
    AgentBack,
    Variant,
    Attach,
    Dictate,
    SubmitAlternate,
    PastePlain,
    SubmitAnswer,
    Stop,
    FileOpen,
    FileBrowse,
    BrowserOpen,
    ReviewOpen,
    DockToggle,
    BottomToggle,
    TerminalNew,
    PaneTabNew,
    PaneTabPrev,
    PaneTabNext,
    TreeToggle,
    CopySelection,
    BrowserAddress,
    BrowserTab,
    BrowserReload,
    BrowserInspect,
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
            Self::General => "General",
            Self::Session => "Session",
            Self::Navigation => "Navigation",
            Self::Models => "Model and agent",
            Self::Terminal => "Terminal",
            Self::Prompt => "Prompt",
        }
    }
}

/// A row of the table.
pub struct Binding {
    pub bind: Bind,
    /// Desktop's command id, for a settings file.
    pub id: &'static str,
    pub title: &'static str,
    pub section: Section,
    pub keybind: Keybind,
    /// Scoped to a pane or a developer tool: works, but Settings doesn't list it.
    pub hidden: bool,
}

static TABLE: LazyLock<Vec<Binding>> = LazyLock::new(table);

fn table() -> Vec<Binding> {
    use Bind::*;
    use Section::{General, Models, Navigation, Prompt, Session, Terminal};
    let on = |kb: &str| Keybind::new(kb);
    // The session view's own (`session_view::keymap`), listed here so Settings shows them.
    let view = |b: session_view::keymap::Bind| b.keybind().clone();
    let rows = vec![
        row(Palette, "command.palette", "Search everything", General, on("mod+k")),
        row(CommandSearch, "commands.search", "Search commands", General, on("mod+shift+p")),
        row(SessionSearch, "sessions.search", "Search sessions", General, on("mod+shift+f")),
        row(Settings, "settings.open", "Settings", General, on("mod+comma")),
        // `titlebar.tsx`: Windows keeps Ctrl+B for Move to background.
        row(Home, "home.toggle", "Home", General, on("mod+b").on(Windows, "alt+home")),
        row(Sidebar, "sidebar.toggle", "Toggle sidebar", General, on("mod+shift+b")),
        // ⌥⌘B is Ctrl+Alt+B off macOS, the Browser's launcher; there only `review.toggle`'s chord.
        row(SideToggle, "review.toggle", "Toggle side panel", General, on("mod+alt+b, mod+shift+r").on(Windows, "mod+shift+r").on(Linux, "mod+shift+r")),
        row(OpenProject, "project.open", "Open project", General, view(session_view::keymap::Bind::OpenProject)),
        row(SelectProject, "project.select", "Search projects", General, on("mod+shift+o")),
        row(ZoomIn, "view.zoomIn", "Zoom in", General, view(session_view::keymap::Bind::ZoomIn)),
        row(ZoomOut, "view.zoomOut", "Zoom out", General, view(session_view::keymap::Bind::ZoomOut)),
        row(ZoomReset, "view.resetZoom", "Actual size", General, view(session_view::keymap::Bind::ZoomReset)),
        row(AutoAccept, "permissions.autoaccept", "Auto-accept permissions", General, on("mod+shift+a")),
        row(FocusInput, "input.focus", "Focus input", General, on("ctrl+l")),
        row(TabNew, "tab.new", "New session", Navigation, on("mod+t, mod+n")),
        row(TabClose, "tab.close", "Close tab", Navigation, on("mod+w")),
        row(TabReopen, "tab.reopenClosed", "Reopen closed tab", Navigation, on("mod+shift+t")),
        hidden(TabPrev, "tab.prev", "Previous tab", Navigation, on("mod+alt+left, ctrl+shift+tab")),
        hidden(TabNext, "tab.next", "Next tab", Navigation, on("mod+alt+right, ctrl+tab")),
        row(SessionPrev, "session.previous", "Previous session", Navigation, on("alt+up")),
        row(SessionNext, "session.next", "Next session", Navigation, on("alt+down")),
        row(UnseenPrev, "session.previous.unseen", "Previous unread session", Navigation, on("shift+alt+up")),
        row(UnseenNext, "session.next.unseen", "Next unread session", Navigation, on("shift+alt+down")),
        hidden(Tab1, "tab.1", "Go to tab 1", Navigation, on("mod+1")),
        hidden(Tab2, "tab.2", "Go to tab 2", Navigation, on("mod+2")),
        hidden(Tab3, "tab.3", "Go to tab 3", Navigation, on("mod+3")),
        hidden(Tab4, "tab.4", "Go to tab 4", Navigation, on("mod+4")),
        hidden(Tab5, "tab.5", "Go to tab 5", Navigation, on("mod+5")),
        hidden(Tab6, "tab.6", "Go to tab 6", Navigation, on("mod+6")),
        hidden(Tab7, "tab.7", "Go to tab 7", Navigation, on("mod+7")),
        hidden(Tab8, "tab.8", "Go to tab 8", Navigation, on("mod+8")),
        hidden(Tab9, "tab.9", "Go to last tab", Navigation, on("mod+9")),
        row(GoBack, "common.goBack", "Back", Navigation, on("mod+[")),
        row(GoForward, "common.goForward", "Forward", Navigation, on("mod+]")),
        row(Find, "session.find", "Find in session", Session, on("mod+f")),
        row(FindNext, "session.find.next", "Next match", Session, on("mod+g")),
        row(FindPrev, "session.find.previous", "Previous match", Session, on("mod+shift+g")),
        row(Background, "session.background", "Move to background", Session, on("ctrl+b")),
        row(Rename, "session.rename", "Rename session", Session, on("ctrl+r")),
        row(Model, "model.choose", "Choose model", Models, view(session_view::keymap::Bind::Model)),
        row(Agent, "agent.cycle", "Cycle agent", Models, view(session_view::keymap::Bind::Agent)),
        row(AgentBack, "agent.cycle.reverse", "Cycle agent backwards", Models, view(session_view::keymap::Bind::AgentBack)),
        row(Variant, "model.variant.cycle", "Cycle model variant", Models, view(session_view::keymap::Bind::Variant)),
        row(Attach, "prompt.attach", "Add images and files", Prompt, view(session_view::keymap::Bind::Attach)),
        row(Dictate, "prompt.dictate", "Dictate with Hex", Prompt, on("mod+shift+m")),
        row(SubmitAlternate, "prompt.submit.alternate", "Send the other way (steer or queue)", Prompt, on("mod+enter, mod+shift+enter")),
        // Chromium's paste as plain text, which Desktop's fields get from the browser; Rune leaves
        // Ctrl+Shift+V to the app. ⇧⌘V pasted here on macOS before, and still does.
        hidden(PastePlain, "edit.pastePlain", "Paste as plain text", Prompt, on("mod+shift+v")),
        row(SubmitAnswer, "question.submit", "Next question or submit", Prompt, view(session_view::keymap::Bind::QuestionAdvance)),
        row(Stop, "session.stop", "Stop the session", Prompt, view(session_view::keymap::Bind::Stop)),
        row(FileOpen, "file.open", "Open file", Session, on("mod+p")),
        // The GUI plugins' launchers (`gui-plugins/src/{browser,file,review}/renderer.tsx`):
        // ⌃⌘ on macOS, Ctrl+Alt elsewhere, so they keep clear of Ctrl+B, Ctrl+P and Ctrl+R.
        // On macOS Files' launcher stays on ⌘P (`file.open`), as it always has here.
        row(FileBrowse, "file.browse", "Browse files", Session, on("ctrl+alt+p").on(Mac, "none")),
        row(BrowserOpen, "browser.open", "Open browser", Session, on("ctrl+mod+b").on(Windows, "ctrl+alt+b").on(Linux, "ctrl+alt+b")),
        row(ReviewOpen, "review.open", "Review changes", Session, on("ctrl+alt+r").on(Mac, "ctrl+mod+r")),
        // Desktop's `terminal.toggle` (Ctrl+`) and this app's ⌘J dock toggle: the label reads ⌘J on
        // macOS, as it always has here, and Ctrl+` elsewhere, as Desktop's does.
        row(DockToggle, "terminal.toggle", "Toggle terminal", Terminal, on("mod+j, ctrl+`").on(Windows, "ctrl+`, mod+j").on(Linux, "ctrl+`, mod+j")),
        row(BottomToggle, "bottom.toggle", "Toggle bottom panel", Session, on("mod+j")),
        row(TerminalNew, "terminal.new", "New terminal", Terminal, on("ctrl+alt+t")),
        hidden(PaneTabNew, "file.tab.new", "New tab in the pane", Navigation, on("mod+t")),
        hidden(PaneTabPrev, "file.tab.prev", "Previous tab in the pane", Navigation, on("mod+shift+[")),
        hidden(PaneTabNext, "file.tab.next", "Next tab in the pane", Navigation, on("mod+shift+]")),
        hidden(TreeToggle, "fileTree.toggle", "Toggle file tree", Session, on("mod+\\")),
        hidden(CopySelection, "edit.copy", "Copy", General, on("mod+c")),
        hidden(BrowserAddress, "browser.address", "Focus the address bar", Session, on("mod+l")),
        hidden(BrowserTab, "browser.tab.new", "New browser tab", Session, on("mod+t")),
        hidden(BrowserReload, "browser.reload", "Reload page", Session, on("mod+r")),
        hidden(BrowserInspect, "browser.inspect", "Pick page element", Session, on("mod+shift+c")),
        // The terminal's own keys (`terminal-key-event.ts`, `terminal.tsx`): ⌘ chords on macOS;
        // elsewhere Ctrl belongs to the shell, so copy and paste add Shift.
        hidden(TerminalCopy, "terminal.copy", "Copy", Terminal, on("super+c, ctrl+shift+c")),
        hidden(TerminalPaste, "terminal.paste", "Paste", Terminal, on("super+v").on(Windows, "ctrl+shift+v, super+v").on(Linux, "ctrl+shift+v, super+v")),
        hidden(TerminalSelectAll, "terminal.selectAll", "Select all", Terminal, on("super+a")),
        hidden(TerminalKillLine, "terminal.killLine", "Delete line", Terminal, on("super+backspace")),
        hidden(Knobs, "dev.knobs", "Knobs", General, on("mod+shift+k")),
        hidden(Rewind, "dev.rewind", "Freeze the last minute", General, on("mod+alt+shift+r")),
        // Ctrl+Alt+R and Ctrl+Alt+P are Review and Files off macOS.
        hidden(DemoReplay, "dev.demo.replay", "Replay demo", General, on("mod+alt+r").on(Windows, "ctrl+alt+enter").on(Linux, "ctrl+alt+enter")),
        hidden(DemoPause, "dev.demo.pause", "Pause demo", General, on("mod+alt+p").on(Windows, "ctrl+alt+space").on(Linux, "ctrl+alt+space")),
        // The app menu's accelerators off macOS (`desktop-menu.ts`), which no command binds.
        hidden(WindowNew, "window.new", "New window", General, on("ctrl+shift+n").on(Mac, "none")),
        hidden(WindowClose, "window.close", "Close window", General, on("ctrl+shift+w").on(Mac, "none")),
        row(McpToggle, "mcp.toggle", "Toggle MCPs", Session, on("mod+semicolon")),
    ];
    assert!(rows.iter().enumerate().all(|(i, r)| r.bind as usize == i), "keymap rows follow `Bind`'s order");
    rows
}

fn row(bind: Bind, id: &'static str, title: &'static str, section: Section, keybind: Keybind) -> Binding {
    Binding { bind, id, title, section, keybind, hidden: false }
}

fn hidden(bind: Bind, id: &'static str, title: &'static str, section: Section, keybind: Keybind) -> Binding {
    Binding { hidden: true, ..row(bind, id, title, section, keybind) }
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
/// listed command's chord, except Rename, which Desktop leaves to plugin panes, and Move to
/// background, which acts only while something blocks.
pub fn takes_from_pane(ev: &KeyEvent, tx: &Tx) -> bool {
    hits(ev, tx).any(|b| !b.binding().hidden && !matches!(b, Bind::Rename | Bind::Background))
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

    pub fn keybind(self) -> &'static Keybind {
        &self.binding().keybind
    }

    /// Whether `ev` is one of this command's chords here; see `Keybind::matches`.
    pub fn matches(self, ev: &KeyEvent) -> bool {
        self.keybind().matches(ev) || control_char(ev).is_some_and(|ev| self.keybind().matches(&ev))
    }

    /// `matches`, following the keyboard layout (`Keybind::matches_tx`); use it in key handlers.
    pub fn matches_tx(self, ev: &KeyEvent, tx: &Tx) -> bool {
        self.keybind().matches_tx(ev, tx) || control_char(ev).is_some_and(|ev| self.keybind().matches(&ev))
    }

    /// The tab-number command `ev` is: 0 for the first tab, 8 for the last.
    pub fn tab_at(ev: &KeyEvent, tx: &Tx) -> Option<usize> {
        Self::TABS.iter().position(|b| b.matches_tx(ev, tx))
    }

    /// How the first chord reads here: "⇧⌘T" on macOS, "Ctrl+Shift+T" elsewhere. "" when unbound.
    pub fn label(self) -> String {
        self.chord().map_or_else(String::new, label)
    }

    /// One string per keycap, modifiers first, as Desktop's keybind chips draw it.
    pub fn keycaps(self) -> Vec<String> {
        self.chord().map_or_else(Vec::new, caps)
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

    /// `text` and this command's label, space-separated, as Desktop's tooltips put them: "Find in file ⌘F".
    pub fn hint(self, text: &str) -> String {
        let keys = self.label();
        if keys.is_empty() { text.to_owned() } else { format!("{text} {keys}") }
    }
}

pub use session_view::keymap::{caps, label};

/// A Ctrl-letter that arrives as its control character (⌃R as U+0012), as the letter.
fn control_char(ev: &KeyEvent) -> Option<KeyEvent> {
    let Key::Text(t) = &ev.key else { return None };
    let mut chars = t.chars();
    let c = chars.next().filter(|c| ('\u{1}'..='\u{1a}').contains(c) && chars.next().is_none())?;
    let letter = char::from(b'a' + (c as u8) - 1);
    ev.mods.ctrl.then(|| KeyEvent { key: Key::Text(letter.to_string()), mods: ev.mods })
}

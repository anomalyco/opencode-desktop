//! The panel providers of the session side region (`side`, `split`, and `bottom` panes).
//!
//! Every panel is a plugin's: Browser, Changes, Files and Terminal are built-in plugins', one
//! provider per plugin (`plugin_host::panels`). Tab metadata, launcher rows, shortcuts, and closed-tab restoration use
//! plain data descriptors (`PanelTabDesc`, `PanelLauncherDesc`, `ClosedPanelTab`).

pub mod closing;
pub mod dock;
pub mod drag;
pub mod keys;
pub mod menus;
pub mod persist;
pub mod resize;
pub mod state;
pub mod tabs;
pub mod view;
pub mod width;


use rune::{El, Icon, Tx};

use crate::App;
use crate::keymap::Bind;
use crate::palette::Pal;

pub use state::{SessionSideState, SideState};
pub use tabs::{ClosedSideTab, SessionTabState, SessionTabs, panel_key, split_panel_key};

/// The pane a new terminal opens in beside no other (`terminalPlacement` in Desktop).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DockPlacement {
    #[default]
    Side,
    Bottom,
}

impl DockPlacement {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Side => "side",
            Self::Bottom => "bottom",
        }
    }

    pub fn parse(s: Option<&str>) -> Self {
        match s {
            Some("bottom") => Self::Bottom,
            _ => Self::Side,
        }
    }
}

/// Physical pane inside a session where a tab currently lives (`session-tabs.ts`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SessionPane {
    /// Primary right-hand side pane.
    Side,
    /// Secondary pane split below `Side` inside the right-hand column.
    Split,
    /// Full-width dock below the session chat / side row (toggled with `⌘J`).
    Bottom,
}

impl SessionPane {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Side => "side",
            Self::Split => "split",
            Self::Bottom => "bottom",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "side" => Some(Self::Side),
            "split" => Some(Self::Split),
            "bottom" => Some(Self::Bottom),
            _ => None,
        }
    }
}

pub const SESSION_PANES: [SessionPane; 3] = [
    SessionPane::Side,
    SessionPane::Split,
    SessionPane::Bottom,
];

/// How a tab's close button renders in the strip (`PanelTab.closable`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseButtonMode {
    /// Always visible `×`.
    Standard,
    /// Revealed on tab hover or while selected.
    Hover,
    /// Unclosable tab (e.g. pinned).
    None,
}

/// Pure-data descriptor of one tab offered by a `Panel` in the session's strip.
#[derive(Clone, Debug)]
pub struct PanelTabDesc {
    /// Local id within the plugin; stored key in `SessionTabs` is `"{plugin}:{id}"`.
    pub id: String,
    /// Accessible title and visible label in the tab strip.
    pub title: String,
    /// Leading 14pt icon in the tab trigger.
    pub icon: Option<&'static Icon>,
    /// Optional short count or status badge after the title (e.g. changed file count).
    pub badge: Option<String>,
    /// Listed before stored tabs without being opened, never closed or dragged.
    pub pinned: bool,
    /// Stored at the front of the strip (`BuiltinPanelTab.first`).
    pub first: bool,
    /// Eligible for fallback selection when the active tab closes.
    pub fallback: bool,
    /// Launcher tab (e.g. "Open file"): next preview replaces it, not kept in closed tabs.
    pub transient: bool,
    /// Struck through / dimmed when its backing workspace file is missing.
    pub missing: bool,
    /// Hidden placeholder holding a restored selection until async inventory arrives.
    pub hidden: bool,
    /// Workspace file path shown by this tab, if any.
    pub file: Option<String>,
    /// Shared render group key (`"{plugin}/{group}"`) that keeps one subtree mounted across switches.
    pub group: Option<&'static str>,
    /// Close button style.
    pub closable: CloseButtonMode,
    /// Drags between panes (Desktop's `draggable`, true unless a launcher says not).
    pub draggable: bool,
}

impl PanelTabDesc {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            icon: None,
            badge: None,
            pinned: false,
            first: false,
            fallback: false,
            transient: false,
            missing: false,
            hidden: false,
            file: None,
            group: None,
            closable: CloseButtonMode::Standard,
            draggable: true,
        }
    }

    pub fn icon(mut self, icon: &'static Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn badge(mut self, badge: impl Into<String>) -> Self {
        self.badge = Some(badge.into());
        self
    }

    pub fn group(mut self, group: &'static str) -> Self {
        self.group = Some(group);
        self
    }

    pub fn file(mut self, path: impl Into<String>) -> Self {
        self.file = Some(path.into());
        self
    }

    pub fn first(mut self) -> Self {
        self.first = true;
        self
    }

    pub fn fallback(mut self) -> Self {
        self.fallback = true;
        self
    }

    pub fn pinned(mut self) -> Self {
        self.pinned = true;
        self.closable = CloseButtonMode::None;
        self
    }

    /// Stays where it is: a launcher tab, such as Open file, doesn't drag.
    pub fn undraggable(mut self) -> Self {
        self.draggable = false;
        self
    }

    pub fn transient(mut self) -> Self {
        self.transient = true;
        self
    }

    /// Listed but not drawn: holds a restored selection until the panel can show it.
    pub fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }

    pub fn missing(mut self, missing: bool) -> Self {
        self.missing = missing;
        self
    }

    pub fn closable(mut self, mode: CloseButtonMode) -> Self {
        self.closable = mode;
        self
    }

    /// Whether Close (button, menu, ⌘W) can close this tab.
    pub fn can_close(&self) -> bool {
        !self.pinned && self.closable != CloseButtonMode::None
    }
}

/// Entry in the side region's `+` menu and `"New tab"` launcher list (`SessionPanelMenuItem`).
#[derive(Clone, Debug)]
pub struct PanelLauncherDesc {
    pub id: &'static str,
    pub title: String,
    pub description: Option<String>,
    pub icon: &'static Icon,
    /// The commands whose chord the entry shows: the first one bound on this platform.
    pub shortcut: &'static [Bind],
    /// Ascending display order (Desktop: Review = 10, Browser = 15, Terminal = 20, Open file = 30).
    pub order: i32,
    /// Singleton tab is already open: omitted from `"New tab"` launcher cards, kept in `+` menu.
    pub opened: bool,
    /// A published command whose chord the entry shows when `shortcut` has none (a plugin's).
    pub command: Option<&'static str>,
}

impl PanelLauncherDesc {
    pub fn new(id: &'static str, title: impl Into<String>, icon: &'static Icon, order: i32) -> Self {
        Self { id, title: title.into(), description: None, icon, shortcut: &[], order, opened: false, command: None }
    }

    pub fn description(mut self, description: Option<String>) -> Self {
        self.description = description;
        self
    }

    pub fn shortcut(mut self, shortcut: &'static [Bind]) -> Self {
        self.shortcut = shortcut;
        self
    }

    pub fn command(mut self, command: Option<&'static str>) -> Self {
        self.command = command;
        self
    }

    pub fn opened(mut self, opened: bool) -> Self {
        self.opened = opened;
        self
    }

    /// The keycaps its row shows: its shortcut's, else its command's.
    pub fn keycaps(&self, s: &App) -> Vec<String> {
        if let Some(&bind) = self.shortcut.iter().find(|b| !b.keycaps().is_empty()) {
            return bind.keycaps();
        }
        self.command.and_then(|id| crate::keymap::command_chord(s, id)).map_or_else(Vec::new, crate::keymap::caps)
    }
}

/// How a launcher entry opens: its plugin's panel, or its `session.panel` menu item, by the
/// entry's id.
#[derive(Clone, Copy)]
pub struct Launch(pub &'static str);

impl Launch {
    pub fn run(self, s: &mut App, id: &str, pane: SessionPane, tx: &mut Tx) {
        crate::plugin_host::panels::open_entry(s, self.0, id, pane, tx);
    }

    /// The pointer came onto entry `id`'s row: its `session.panel` item's `hover` runs.
    pub fn hover(self, s: &mut App, id: &str, tx: &mut Tx) {
        crate::plugin_host::panels::hover_entry(s, self.0, id, tx);
    }
}

/// Metadata saved when a user closes a tab so `⌘⇧T` and `"Recently closed"` can bring it back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClosedPanelTab {
    pub title: Option<String>,
    pub icon_name: Option<String>,
    pub detail: Option<String>,
    pub data: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClosedTabInfo {
    /// Use the tab's own title/file with default metadata (unless transient).
    Default,
    /// Never add this tab to `Recently closed` (e.g. a blank `about:blank` browser tab).
    Deny,
    /// Record custom `ClosedPanelTab` metadata and `data` payload for `on_restore_tab`.
    Allow(ClosedPanelTab),
}

/// Read-only session context passed to `list` and `launcher`.
pub struct PanelSessionCtx<'a> {
    /// The provider's namespace ([`PanelRegistration::plugin`]).
    pub plugin: &'static str,
    pub session_id: &'a str,
    pub directory: &'a str,
    pub project_id: &'a str,
    /// This plugin's tab ids currently stored in the session's strip, in strip order.
    pub open_ids: &'a [String],
}

/// Read-only context passed to `PanelRegistration::render`.
pub struct PanelViewCtx<'a> {
    /// The provider's namespace ([`PanelRegistration::plugin`]).
    pub plugin: &'static str,
    pub session_id: &'a str,
    pub directory: &'a str,
    pub project_id: &'a str,
    pub pane: SessionPane,
    pub pane_size: (f32, f32),
    pub env_scale: f32,
    pub tab: &'a PanelTabDesc,
    pub visible: bool,
    pub focused: bool,
    /// It renders as a narrow window's view (`usePanel().placement() === "mobile"`).
    pub mobile: bool,
}

/// One plugin's panels: their tabs, views and launchers come from the plugin
/// (`plugin_host::panels`), keyed `<plugin>:<tab>` as Desktop keys a plugin's tabs.
#[derive(Clone, Copy)]
pub struct PanelRegistration {
    /// The plugin's id, e.g. `"browser"`, `"review"`, `"file"`, `"terminal"`. Stored tab keys are
    /// `"{plugin}:{tab.id}"`.
    pub plugin: &'static str,
    /// Display order tie-breaker.
    pub order: i32,
    /// The pane a new tab of this plugin opens in when the plugin doesn't say.
    pub default_pane: SessionPane,
    /// Stored tab keys leave the strip automatically once `list` stops returning them.
    pub transient: bool,
}

impl PanelRegistration {
    pub const fn new(plugin: &'static str, order: i32, default_pane: SessionPane, transient: bool) -> Self {
        Self { plugin, order, default_pane, transient }
    }

    /// Its tabs for the routed session.
    pub fn list(&self, s: &App, ctx: &PanelSessionCtx) -> Vec<PanelTabDesc> {
        crate::plugin_host::panels::list(s, ctx)
    }

    /// Draws `ctx.tab` inside its pane.
    pub fn render(&self, s: &App, p: &Pal, ctx: &PanelViewCtx) -> El {
        crate::plugin_host::panels::render(s, p, ctx)
    }

    /// Whether this panel asks for the wider session minimum now.
    pub fn is_wide(&self, s: &App) -> bool {
        crate::plugin_host::panels::wide(s, self.plugin)
    }

    /// Every `+` / New tab entry this plugin offers.
    pub fn launcher_entries(&self, s: &App, ctx: &PanelSessionCtx) -> Vec<(PanelLauncherDesc, Launch)> {
        crate::plugin_host::panels::launchers(s, ctx).into_iter().map(|d| (d, Launch(self.plugin))).collect()
    }

    /// Runs as `tab` of `session` becomes selected.
    pub fn focus(&self, s: &mut App, session: &str, tab: &str, restored: bool) {
        crate::plugin_host::panels::focused(s, self.plugin, session, tab, restored);
    }

    /// Runs as the side region opens onto `tab` of `session`.
    pub fn enter(&self, s: &mut App, session: &str, tab: &str) {
        crate::plugin_host::panels::focused(s, self.plugin, session, tab, false);
    }

    /// Runs after `tab` left `session`'s strip.
    pub fn close(&self, s: &mut App, session: &str, tab: &str, tx: &mut Tx) {
        crate::plugin_host::panels::closed(s, self.plugin, session, tab, tx);
    }

    /// What Recently closed keeps of `tab` as it closes.
    pub fn closed_info(&self, s: &App, session: &str, tab: &PanelTabDesc) -> ClosedTabInfo {
        crate::plugin_host::panels::closed_info(s, self.plugin, session, tab)
    }

    /// Restores a closed tab; false when the plugin leaves it to the default reopen.
    pub fn restore(&self, s: &mut App, session: &str, tab: &str, data: Option<&str>, tx: &mut Tx) -> bool {
        crate::plugin_host::panels::restore(s, self.plugin, session, tab, data, tx)
    }

    /// A tab's own strip label, if it draws one.
    pub fn lead(&self, s: &App, p: &Pal, tab: &PanelTabDesc, selected: bool) -> Option<El> {
        crate::plugin_host::panels::tab_lead(s, p, self.plugin, tab, selected)
    }

    pub fn lead_stamp(&self, s: &App, p: &Pal, tab: &PanelTabDesc, selected: bool) -> Option<u64> {
        crate::plugin_host::panels::tab_lead_stamp(s, p, self.plugin, tab, selected)
    }

    /// The pane a new tab of this panel opens in beside none of its others.
    pub fn home_pane(&self, s: &App) -> SessionPane {
        crate::plugin_host::panels::home_pane(s, self.plugin).unwrap_or(self.default_pane)
    }
}

/// Icon lookup by stored closed-tab icon name.
pub fn icon_by_name(name: Option<&str>) -> &'static Icon {
    use rune_theme_opencode::icons as ic;
    match name {
        Some("globe") => &ic::GLOBE,
        // Desktop's `terminal` icon: the prompt in its box, as the terminal tabs draw it.
        Some("terminal") => &crate::terminal::grid::TERMINAL_ICON,
        Some("console") => &ic::CONSOLE,
        Some("review") => &ic::REVIEW,
        Some("code") => &ic::CODE_LINES,
        Some("file-tree") => &ic::FILE_TREE,
        _ => &ic::FOLDER,
    }
}

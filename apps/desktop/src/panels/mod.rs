//! The panel providers of the session side region (`side`, `split`, and `bottom` panes).
//!
//! The built-in panels (Browser, Changes, Files, Terminal) are Luau plugins', one provider per
//! plugin (`plugin_host::luau::panels`); the native plugins' panels share one provider
//! (`plugin_host::view`). Tab metadata, launcher rows, shortcuts, and closed-tab restoration use
//! plain data descriptors (`PanelTabDesc`, `PanelLauncherDesc`, `ClosedPanelTab`).

pub mod closing;
pub mod dock;
pub mod drag;
pub mod keys;
pub mod menus;
pub mod persist;
pub mod state;
pub mod tabs;
#[cfg(test)]
mod tests;
pub mod view;
pub mod width;

use std::sync::OnceLock;

use rune::{El, Icon, Ime, KeyEvent, Tx};

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

/// How a launcher entry opens: its panel's one launcher, or a provider's shared opener given
/// the entry's id (a provider with several entries, such as native plugins' panels).
#[derive(Clone, Copy)]
pub enum Launch {
    Pane(fn(&mut App, SessionPane, &mut Tx)),
    Entry(fn(&mut App, &str, SessionPane, &mut Tx)),
    /// A Luau plugin's entry: its panel, or its `session.panel` menu item.
    Luau(&'static str),
}

impl Launch {
    pub fn run(self, s: &mut App, id: &str, pane: SessionPane, tx: &mut Tx) {
        match self {
            Self::Pane(open) => open(s, pane, tx),
            Self::Entry(open) => open(s, id, pane, tx),
            Self::Luau(plugin) => crate::plugin_host::luau::panels::open_entry(s, plugin, id, pane, tx),
        }
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

fn no_wide(_: &App) -> bool {
    false
}

/// One panel provider (`Copy`, and `const`-constructible).
#[derive(Clone, Copy)]
pub struct PanelRegistration {
    /// Unique plugin namespace, e.g. `"browser"`, `"review"`, `"file"`, `"terminal"`.
    /// Stored tab keys are `"{plugin}:{tab.id}"`.
    pub plugin: &'static str,
    /// Display order tie-breaker.
    pub order: i32,
    /// Default pane when opening a new tab of this plugin (`Side` by default; `Bottom` for a bottom-docked panel).
    pub default_pane: SessionPane,
    /// The default pane read from the app's settings, overriding `default_pane`.
    pub placed: Option<fn(&App) -> SessionPane>,
    /// Asks for a wider side region minimum (e.g. split diff).
    pub wide: fn(&App) -> bool,
    /// Stored tab keys leave the strip automatically once `list` stops returning them.
    pub transient: bool,
    /// Legacy stored keys mapped to `(old_key, new_tab_id)`.
    pub legacy: &'static [(&'static str, &'static str)],
    /// Optional global shortcut & `⌘K` palette title for launching/toggling this panel.
    pub shortcut: Option<Bind>,
    /// Its palette command's title and description, as catalog keys.
    pub command_title: Option<opencode_i18n::Key>,
    /// What the command runs when it isn't the launcher (a toggle, such as the terminal's).
    pub run: Option<fn(&mut App, &mut Tx)>,
    /// What the command's row says after its name in the palette and the `/` list.
    pub command_description: Option<opencode_i18n::Key>,
    /// The `/name` that runs the command from the composer.
    pub slash: Option<&'static str>,
    /// Additional session-scoped global shortcuts (e.g. `⌃\`` toggle and `⌃⌥T` new terminal).
    pub extra_shortcuts: &'static [(Bind, fn(&mut App, &mut Tx))],
    /// Optional `+` / `"New tab"` launcher entry.
    pub launcher: Option<fn(&App, &PanelSessionCtx) -> Option<PanelLauncherDesc>>,
    /// Invoked when the user triggers the launcher or global command.
    pub open_launcher: Option<fn(&mut App, SessionPane, &mut Tx)>,
    /// Several `+` / `"New tab"` entries, each opened by `open_entry` with its id (a provider
    /// whose tabs come and go at run time, such as native plugins' panels).
    pub launchers: Option<fn(&App, &PanelSessionCtx) -> Vec<PanelLauncherDesc>>,
    pub open_entry: Option<fn(&mut App, &str, SessionPane, &mut Tx)>,
    /// Lists this panel's tabs for the routed session.
    pub list: fn(&App, &PanelSessionCtx) -> Vec<PanelTabDesc>,
    /// Renders `ctx.tab` inside its pane.
    pub render: fn(&App, &Pal, &PanelViewCtx) -> El,
    /// Runs after `tab_id` is removed from `session_id`'s strip.
    pub on_close_tab: Option<fn(&mut App, &str, &str, &mut Tx)>,
    /// Returns metadata for `Recently closed` / `⌘⇧T` as `tab` closes.
    pub closed_tab: Option<fn(&App, &str, &PanelTabDesc) -> ClosedTabInfo>,
    /// Restores a closed tab from `id` and optional `data`; if `None`, reopens `"{plugin}:{id}"`.
    pub on_restore_tab: Option<fn(&mut App, &str, &str, Option<&str>, &mut Tx)>,
    /// Runs when `tab_id` becomes selected (`restored` is true on session mount/restore).
    pub on_focus_tab: Option<fn(&mut App, &str, &str, bool)>,
    /// Runs when the user opens the side region onto `tab_id`.
    pub on_enter: Option<fn(&mut App, &str, &str)>,
    /// Optional per-frame poll hook; returns `true` only when visible state changed.
    pub poll: Option<fn(&mut App, &mut Tx) -> bool>,
    /// Optional key hook for panel-scoped shortcuts when this panel is visible.
    pub on_key: Option<fn(&mut App, &KeyEvent, &mut Tx) -> bool>,
    /// Optional IME composition/commit hook when this panel is active.
    pub on_ime: Option<fn(&mut App, &Ime) -> bool>,
    /// A tab's own label in the strip in place of its icon and title (`PanelTab.label`), given
    /// whether the tab is selected; `None` keeps the icon and title.
    pub tab_lead: Option<fn(&App, &Pal, &PanelTabDesc, bool) -> Option<El>>,
    /// The panels of the Luau plugin `plugin`: the callbacks above stay unset, and the dispatch
    /// methods below ask the plugin (`plugin_host::luau::panels`).
    pub luau: bool,
}

impl PanelRegistration {
    pub const fn new(
        plugin: &'static str,
        list: fn(&App, &PanelSessionCtx) -> Vec<PanelTabDesc>,
        render: fn(&App, &Pal, &PanelViewCtx) -> El,
    ) -> Self {
        Self {
            plugin,
            order: 0,
            default_pane: SessionPane::Side,
            placed: None,
            wide: no_wide,
            transient: false,
            legacy: &[],
            shortcut: None,
            command_title: None,
            run: None,
            command_description: None,
            slash: None,
            extra_shortcuts: &[],
            launcher: None,
            open_launcher: None,
            launchers: None,
            open_entry: None,
            list,
            render,
            on_close_tab: None,
            closed_tab: None,
            on_restore_tab: None,
            on_focus_tab: None,
            on_enter: None,
            poll: None,
            on_key: None,
            on_ime: None,
            tab_lead: None,
            luau: false,
        }
    }

    /// Luau plugin `plugin`'s panels, under its own namespace (`<plugin>:<tab>`, as Desktop keys
    /// a plugin's tabs).
    pub fn luau(plugin: &'static str, order: i32, default_pane: SessionPane) -> Self {
        use crate::plugin_host::luau::panels as lp;
        Self { order, default_pane, luau: true, ..Self::new(plugin, lp::list, lp::render) }
    }

    /// Whether this panel asks for the wider session minimum now.
    pub fn is_wide(&self, s: &App) -> bool {
        if self.luau {
            return crate::plugin_host::luau::panels::wide(s, self.plugin);
        }
        (self.wide)(s)
    }

    /// Every `+` / New tab entry this provider offers, with how each opens.
    pub fn launcher_entries(&self, s: &App, ctx: &PanelSessionCtx) -> Vec<(PanelLauncherDesc, Launch)> {
        if self.luau {
            return crate::plugin_host::luau::panels::launchers(s, ctx).into_iter().map(|d| (d, Launch::Luau(self.plugin))).collect();
        }
        let one = self.launcher.zip(self.open_launcher).and_then(|(desc, open)| Some((desc(s, ctx)?, Launch::Pane(open))));
        let many = self.launchers.zip(self.open_entry).map(|(list, open)| list(s, ctx).into_iter().map(move |d| (d, Launch::Entry(open))));
        one.into_iter().chain(many.into_iter().flatten()).collect()
    }

    /// Runs as `tab` of `session` becomes selected.
    pub fn focus(&self, s: &mut App, session: &str, tab: &str, restored: bool) {
        if self.luau {
            return crate::plugin_host::luau::panels::focused(s, self.plugin, session, tab, restored);
        }
        if let Some(f) = self.on_focus_tab {
            f(s, session, tab, restored);
        }
    }

    /// Runs as the side region opens onto `tab` of `session`.
    pub fn enter(&self, s: &mut App, session: &str, tab: &str) {
        if self.luau {
            return crate::plugin_host::luau::panels::focused(s, self.plugin, session, tab, false);
        }
        if let Some(f) = self.on_enter {
            f(s, session, tab);
        }
    }

    /// Runs after `tab` left `session`'s strip.
    pub fn close(&self, s: &mut App, session: &str, tab: &str, tx: &mut Tx) {
        if self.luau {
            return crate::plugin_host::luau::panels::closed(s, self.plugin, session, tab, tx);
        }
        if let Some(f) = self.on_close_tab {
            f(s, session, tab, tx);
        }
    }

    /// What Recently closed keeps of `tab` as it closes.
    pub fn closed_info(&self, s: &App, session: &str, tab: &PanelTabDesc) -> ClosedTabInfo {
        if self.luau {
            return crate::plugin_host::luau::panels::closed_info(s, self.plugin, session, tab);
        }
        self.closed_tab.map_or(ClosedTabInfo::Default, |f| f(s, session, tab))
    }

    /// Restores a closed tab; false when the provider leaves it to the default reopen.
    pub fn restore(&self, s: &mut App, session: &str, tab: &str, data: Option<&str>, tx: &mut Tx) -> bool {
        if self.luau {
            return crate::plugin_host::luau::panels::restore(s, self.plugin, session, tab, data, tx);
        }
        match self.on_restore_tab {
            Some(f) => {
                f(s, session, tab, data, tx);
                true
            }
            None => false,
        }
    }

    /// A tab's own strip label, if it draws one.
    pub fn lead(&self, s: &App, p: &Pal, tab: &PanelTabDesc, selected: bool) -> Option<El> {
        if self.luau {
            return crate::plugin_host::luau::panels::tab_lead(s, p, self.plugin, tab, selected);
        }
        self.tab_lead.and_then(|lead| lead(s, p, tab, selected))
    }

    pub const fn tab_lead(mut self, f: fn(&App, &Pal, &PanelTabDesc, bool) -> Option<El>) -> Self {
        self.tab_lead = Some(f);
        self
    }

    pub const fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    pub const fn default_pane(mut self, pane: SessionPane) -> Self {
        self.default_pane = pane;
        self
    }

    pub const fn placed(mut self, placed: fn(&App) -> SessionPane) -> Self {
        self.placed = Some(placed);
        self
    }

    /// The pane a new tab of this panel opens in beside none of its others.
    pub fn home_pane(&self, s: &App) -> SessionPane {
        if self.luau {
            return crate::plugin_host::luau::panels::home_pane(s, self.plugin).unwrap_or(self.default_pane);
        }
        self.placed.map_or(self.default_pane, |f| f(s))
    }

    pub const fn wide(mut self, wide: fn(&App) -> bool) -> Self {
        self.wide = wide;
        self
    }

    pub const fn transient(mut self, transient: bool) -> Self {
        self.transient = transient;
        self
    }

    pub const fn legacy(mut self, legacy: &'static [(&'static str, &'static str)]) -> Self {
        self.legacy = legacy;
        self
    }

    pub const fn command(mut self, title: opencode_i18n::Key, shortcut: Bind) -> Self {
        self.command_title = Some(title);
        self.shortcut = Some(shortcut);
        self
    }

    pub const fn runs(mut self, run: fn(&mut App, &mut Tx)) -> Self {
        self.run = Some(run);
        self
    }

    pub const fn slash(mut self, name: &'static str) -> Self {
        self.slash = Some(name);
        self
    }

    pub const fn describe(mut self, description: opencode_i18n::Key) -> Self {
        self.command_description = Some(description);
        self
    }

    pub const fn shortcuts(
        mut self,
        shortcuts: &'static [(Bind, fn(&mut App, &mut Tx))],
    ) -> Self {
        self.extra_shortcuts = shortcuts;
        self
    }

    pub const fn launcher(
        mut self,
        launcher: fn(&App, &PanelSessionCtx) -> Option<PanelLauncherDesc>,
        open: fn(&mut App, SessionPane, &mut Tx),
    ) -> Self {
        self.launcher = Some(launcher);
        self.open_launcher = Some(open);
        self
    }

    pub const fn launchers(
        mut self,
        list: fn(&App, &PanelSessionCtx) -> Vec<PanelLauncherDesc>,
        open: fn(&mut App, &str, SessionPane, &mut Tx),
    ) -> Self {
        self.launchers = Some(list);
        self.open_entry = Some(open);
        self
    }

    pub const fn on_close(mut self, f: fn(&mut App, &str, &str, &mut Tx)) -> Self {
        self.on_close_tab = Some(f);
        self
    }

    pub const fn closed(mut self, f: fn(&App, &str, &PanelTabDesc) -> ClosedTabInfo) -> Self {
        self.closed_tab = Some(f);
        self
    }

    pub const fn on_restore(
        mut self,
        f: fn(&mut App, &str, &str, Option<&str>, &mut Tx),
    ) -> Self {
        self.on_restore_tab = Some(f);
        self
    }

    pub const fn on_focus(mut self, f: fn(&mut App, &str, &str, bool)) -> Self {
        self.on_focus_tab = Some(f);
        self
    }

    pub const fn on_enter(mut self, f: fn(&mut App, &str, &str)) -> Self {
        self.on_enter = Some(f);
        self
    }

    pub const fn poll(mut self, f: fn(&mut App, &mut Tx) -> bool) -> Self {
        self.poll = Some(f);
        self
    }

    pub const fn on_key(mut self, f: fn(&mut App, &KeyEvent, &mut Tx) -> bool) -> Self {
        self.on_key = Some(f);
        self
    }

    pub const fn on_ime(mut self, f: fn(&mut App, &Ime) -> bool) -> Self {
        self.on_ime = Some(f);
        self
    }
}

/// The app's own panel providers: the native plugins' panels. The built-in panels are Luau
/// plugins' (`plugin_host::luau::panels::providers`).
pub fn registered_panels() -> &'static [PanelRegistration] {
    static ALL: OnceLock<Vec<PanelRegistration>> = OnceLock::new();
    ALL.get_or_init(|| vec![crate::plugin_host::view::provider()])
}

/// Finds a registered provider by its `plugin` id.
pub fn find_panel(plugin: &str) -> Option<&'static PanelRegistration> {
    registered_panels().iter().find(|p| p.plugin == plugin)
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

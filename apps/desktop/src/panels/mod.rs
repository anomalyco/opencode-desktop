//! Two-lane Panel registry for the session side region (`side`, `split`, and `bottom` panes).
//!
//! Built-in panels (Browser, Changes/Review, Files, Terminal) register in-process from their own
//! modules via `rune_register::register!(crate::panels::PANELS: ...)`. Tab metadata, launcher rows,
//! shortcuts, and closed-tab restoration use plain data descriptors (`PanelTabDesc`,
//! `PanelLauncherDesc`, `ClosedPanelTab`) so a future sandboxed WASM `Surface` lane can feed the
//! exact same region state without changing built-ins or `view.rs`.

pub mod dock;
pub mod keys;
pub mod menus;
pub mod persist;
pub mod state;
pub mod tabs;
#[cfg(test)]
mod tests;
pub mod view;

use std::sync::OnceLock;

use rune::{El, Icon, Ime, Key, KeyEvent, Tx};
use rune_register::{collect, slice};

use crate::App;
use crate::palette::Pal;

pub use state::{SessionSideState, SideState};
pub use tabs::{ClosedSideTab, SessionTabState, SessionTabs, panel_key, split_panel_key};

// Distributed slice of all compiled-in panel providers.
slice!(pub PANELS: PanelRegistration);

/// Logical host region a panel contributes to (`Side` tab strip or `Dock` terminal region).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PanelRegion {
    /// Tabbed right-hand side region (Browser, Review/Changes, Files, etc.).
    Side,
    /// Dedicated dock region (Terminal, `⌘J`), stacked below `Side` in the right column
    /// (`DockPlacement::Side`, default) or across the bottom of the session screen (`DockPlacement::Bottom`).
    Dock,
}

/// User preference for where `PanelRegion::Dock` renders (`terminalPlacement` in Desktop).
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

/// Declarative keyboard shortcut chord for a panel launcher or scoped action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShortcutChord {
    pub key: char,
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub hint: &'static str,
}

impl ShortcutChord {
    pub const fn ctrl_cmd(key: char, hint: &'static str) -> Self {
        Self { key, cmd: true, ctrl: true, alt: false, shift: false, hint }
    }

    pub const fn cmd_j() -> Self {
        Self { key: 'j', cmd: true, ctrl: false, alt: false, shift: false, hint: "⌘J" }
    }

    pub const fn cmd_p() -> Self {
        Self { key: 'p', cmd: true, ctrl: false, alt: false, shift: false, hint: "⌘P" }
    }

    pub const fn ctrl_backtick() -> Self {
        Self { key: '`', cmd: false, ctrl: true, alt: false, shift: false, hint: "⌃`" }
    }

    pub const fn ctrl_alt(key: char, hint: &'static str) -> Self {
        Self { key, cmd: false, ctrl: true, alt: true, shift: false, hint }
    }

    pub fn matches(&self, ev: &KeyEvent) -> bool {
        let m = ev.mods;
        if m.cmd != self.cmd || m.ctrl != self.ctrl || m.alt != self.alt || m.shift != self.shift {
            return false;
        }
        match &ev.key {
            Key::Command(c) => c.eq_ignore_ascii_case(&self.key),
            Key::Text(s) => {
                let mut chars = s.chars();
                chars.next().is_some_and(|c| c.eq_ignore_ascii_case(&self.key))
                    && chars.next().is_none()
            }
            _ => false,
        }
    }

    pub fn matches_tx(&self, ev: &KeyEvent, tx: &Tx) -> bool {
        if self.matches(ev) {
            return true;
        }
        let m = ev.mods;
        if m.cmd != self.cmd || m.ctrl != self.ctrl || m.alt != self.alt || m.shift != self.shift {
            return false;
        }
        tx.key_base().is_some_and(|b| b.eq_ignore_ascii_case(&self.key))
    }
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
            closable: CloseButtonMode::Hover,
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

    pub fn transient(mut self) -> Self {
        self.transient = true;
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
}

/// Entry in the side region's `+` menu and `"New tab"` launcher list (`SessionPanelMenuItem`).
#[derive(Clone, Debug)]
pub struct PanelLauncherDesc {
    pub id: &'static str,
    pub title: String,
    pub description: Option<String>,
    pub icon: &'static Icon,
    pub shortcut_hint: &'static str,
    /// Ascending display order (Desktop: Review = 10, Browser = 15, Terminal = 20, Open file = 30).
    pub order: i32,
    /// Singleton tab is already open: omitted from `"New tab"` launcher cards, kept in `+` menu.
    pub opened: bool,
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
    pub session_id: &'a str,
    pub directory: &'a str,
    pub project_id: &'a str,
    /// This plugin's tab ids currently stored in the session's strip, in strip order.
    pub open_ids: &'a [String],
}

/// Read-only context passed to `PanelRegistration::render`.
pub struct PanelViewCtx<'a> {
    pub session_id: &'a str,
    pub directory: &'a str,
    pub project_id: &'a str,
    pub region: PanelRegion,
    pub pane: SessionPane,
    pub pane_size: (f32, f32),
    pub env_scale: f32,
    pub tab: &'a PanelTabDesc,
    pub visible: bool,
    pub focused: bool,
    /// True when this panel sits at the top-right of the right column with no outer tab bar above it
    /// (e.g. `PanelRegion::Dock` filling the right column while `Side` tabs are closed), so its
    /// header should leave ~30 pt on the trailing end for the floating `side-toggle` button.
    pub reserve_top_right: bool,
}

fn no_wide(_: &App) -> bool {
    false
}

/// One registered panel provider (`Copy` + `const`-constructible for `rune_register::slice!`).
#[derive(Clone, Copy)]
pub struct PanelRegistration {
    /// Unique plugin namespace, e.g. `"browser"`, `"review"`, `"file"`, `"terminal"`.
    /// Stored tab keys are `"{plugin}:{tab.id}"`.
    pub plugin: &'static str,
    /// Display order tie-breaker.
    pub order: i32,
    /// Target region (`PanelRegion::Side`).
    pub region: PanelRegion,
    /// Default pane when opening a new tab of this plugin (`Side` by default; `Bottom` for a bottom-docked panel).
    pub default_pane: SessionPane,
    /// Asks for a wider side region minimum (e.g. split diff).
    pub wide: fn(&App) -> bool,
    /// Stored tab keys leave the strip automatically once `list` stops returning them.
    pub transient: bool,
    /// Legacy stored keys mapped to `(old_key, new_tab_id)`.
    pub legacy: &'static [(&'static str, &'static str)],
    /// Optional global shortcut & `⌘K` palette title for launching/toggling this panel.
    pub shortcut: Option<ShortcutChord>,
    pub command_title: Option<&'static str>,
    /// The `/name` that runs the command from the composer.
    pub slash: Option<&'static str>,
    /// Additional session-scoped global shortcuts (e.g. `⌃\`` toggle and `⌃⌥T` new terminal).
    pub extra_shortcuts: &'static [(ShortcutChord, fn(&mut App, &mut Tx))],
    /// Optional `+` / `"New tab"` launcher entry.
    pub launcher: Option<fn(&App, &PanelSessionCtx) -> Option<PanelLauncherDesc>>,
    /// Invoked when the user triggers the launcher or global command.
    pub open_launcher: Option<fn(&mut App, SessionPane, &mut Tx)>,
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
            region: PanelRegion::Side,
            default_pane: SessionPane::Side,
            wide: no_wide,
            transient: false,
            legacy: &[],
            shortcut: None,
            command_title: None,
            slash: None,
            extra_shortcuts: &[],
            launcher: None,
            open_launcher: None,
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
        }
    }

    pub const fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    pub const fn region(mut self, region: PanelRegion) -> Self {
        self.region = region;
        self
    }

    pub const fn dock(
        plugin: &'static str,
        list: fn(&App, &PanelSessionCtx) -> Vec<PanelTabDesc>,
        render: fn(&App, &Pal, &PanelViewCtx) -> El,
    ) -> Self {
        Self::new(plugin, list, render).region(PanelRegion::Dock)
    }

    pub const fn default_pane(mut self, pane: SessionPane) -> Self {
        self.default_pane = pane;
        self
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

    pub const fn command(mut self, title: &'static str, shortcut: ShortcutChord) -> Self {
        self.command_title = Some(title);
        self.shortcut = Some(shortcut);
        self
    }

    pub const fn slash(mut self, name: &'static str) -> Self {
        self.slash = Some(name);
        self
    }

    pub const fn shortcuts(
        mut self,
        shortcuts: &'static [(ShortcutChord, fn(&mut App, &mut Tx))],
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

/// Every registered `PanelRegistration`, deterministically ordered by `(order, plugin)`.
pub fn registered_panels() -> &'static [PanelRegistration] {
    static ALL: OnceLock<Vec<PanelRegistration>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut v = collect(&PANELS);
        v.sort_by_key(|p| (p.order, p.plugin));
        v
    })
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
        Some("terminal") | Some("console") => &ic::CONSOLE,
        Some("review") | Some("code") => &ic::CODE_LINES,
        Some("file-tree") => &ic::FILE_TREE,
        _ => &ic::FOLDER,
    }
}

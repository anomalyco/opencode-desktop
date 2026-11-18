//! Side region per-session state, resolved strip projection, and query helpers.

mod actions;
mod resolve;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::closing;
use crate::drag::PaneDrag;
use crate::host::PanelsHost;
use crate::resize::Resize;
use crate::tabs::{ClosedSideTab, SessionTabState, SessionTabs, session_pane_tab, session_tab_pane};
use crate::{Launch, PanelLauncherDesc, PanelRegistration, PanelTabDesc, SessionPane};

pub use actions::{
    SavedKeyboardFocus, begin_keeping_keyboard, close_side_tab, dismiss_side_new_tab, end_keeping_keyboard, finish_show_pane_tab,
    hide_side_tab, keeping_keyboard, move_side_tab, open_side_new_tab, open_side_tab, open_side_tab_for_session,
    promote_preview_tab, reopen_closed_side_tab, select_side_tab, show_pane_tab, show_side_tab, sync_empty_side,
    toggle_bottom_region, toggle_side_region,
};
pub use resolve::resolve_side_region;

/// `PANE_HEIGHT_MIN` (`side-region.tsx`): each pane of a split side region keeps this much.
pub const SPLIT_MIN_H: f32 = 120.0;
/// The side pane's share of a split side region until the divider is dragged.
pub const SPLIT_DEFAULT_RATIO: f32 = 0.5;
/// `DEFAULT_DOCK_HEIGHT` (`shell/state/layout.tsx`): the bottom pane's height until dragged.
pub const BOTTOM_DEFAULT_H: f32 = 280.0;
/// `BOTTOM_PANE_HEIGHT_MIN` (`session/screen.tsx`).
pub const BOTTOM_MIN_H: f32 = 120.0;

/// One session's side region, as Desktop keeps it per shell tab (`TabRegions`): whether its
/// panes show, its strip, and the sizes the user dragged (none until they do).
#[derive(Clone, Debug, PartialEq)]
pub struct SessionSideState {
    pub visibility: Visibility,
    pub bottom_open: bool,
    pub tab_state: SessionTabState,
    pub closed: Vec<ClosedSideTab>,
    /// The chat panel's width beside the side region (`sessionWidth`).
    pub session_width: Option<f32>,
    /// The side pane's share of the side region's height while a pane splits below it.
    pub split_ratio: Option<f32>,
    /// The bottom pane's height.
    pub bottom_height: Option<f32>,
}

impl Default for SessionSideState {
    fn default() -> Self {
        Self {
            visibility: Visibility::Unseen,
            bottom_open: true,
            tab_state: SessionTabState::default(),
            closed: Vec::new(),
            session_width: None,
            split_ratio: None,
            bottom_height: None,
        }
    }
}

/// A shell tab's `contentMounted` latch (`side-region.tsx`). Another session opening a pane
/// cannot make this one lay out a hidden plugin tree. Only the open preference is persisted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Unseen,
    Shown,
    Shut,
}

impl Visibility {
    pub fn open(self) -> bool {
        self == Self::Shown
    }

    pub fn mounted(self) -> bool {
        self != Self::Unseen
    }

    pub fn step(&mut self, open: bool) {
        *self = match (*self, open) {
            (_, true) => Self::Shown,
            (Self::Unseen, false) => Self::Unseen,
            (_, false) => Self::Shut,
        };
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewTabEntry {
    pub id: String,
    pub pane: SessionPane,
    pub session_id: String,
    pub over: Option<String>,
}

/// Whether the routed session's side region is open with an empty strip (`new-tab-machine.ts`
/// `openedEmpty`). Entering such a span opens one New tab; leaving it lets the next one open
/// another.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum EmptySpan {
    #[default]
    Outside,
    Within { session: String },
}

impl EmptySpan {
    pub fn within(&self, session: &str) -> bool {
        matches!(self, Self::Within { session: s } if s == session)
    }

    /// Whether step would change the span: polls write it only then.
    pub fn moves(&self, session: &str, open_empty: bool) -> bool {
        if open_empty { !self.within(session) } else { !matches!(self, Self::Outside) }
    }

    /// Steps on the routed `session`'s region as it is now; true when it enters a span.
    pub fn step(&mut self, session: &str, open_empty: bool) -> bool {
        if !open_empty {
            *self = Self::Outside;
            return false;
        }
        if self.within(session) {
            return false;
        }
        *self = Self::Within { session: session.to_owned() };
        true
    }
}

pub(crate) type ResolvedCache = (u64, rune::Reads, Option<Rc<ResolvedRegion>>);

pub struct SideState {
    /// The chat/side divider's gesture, over the chat panel's width.
    pub chat_resize: Resize,
    /// The split divider's gesture, over the lower pane's height.
    pub split_resize: Resize,
    /// The bottom dock's top edge's gesture, over its height.
    pub bottom_resize: Resize,
    /// The open, empty side region that already opened its New tab.
    pub empty_span: EmptySpan,
    /// The window's width at the last layout, for sizes estimated outside the view.
    pub laid_w: Cell<f32>,
    /// Until when (`PanelsHost::clock`) the left sidebar's flight moves the side region's edge,
    /// so its contents glide with the edge rather than land at once.
    pub edge_flight: Option<f64>,
    /// A tab dragged between panes.
    pub pane_drag: PaneDrag,
    /// Keyboard focus belongs to the side/bottom region rather than the session chat.
    pub focused: bool,
    /// A shortcut pressed in the composer moved the keyboard here (Open file, New tab, Reopen
    /// closed tab): the selected tab's label rests muted until the next key or press inside, as
    /// Desktop's side focus reads the shortcut's own key, whose target was the composer
    /// (`closed-tabs.ts:146-153`). A terminal is the exception there too: its focus comes later.
    pub quiet: bool,
    /// The composer had the keyboard at the last poll, and whether a press landed in the region
    /// since: the keyboard leaving the composer for here with no press is a shortcut's doing.
    pub was_composing: bool,
    pub pressed: bool,
    pub active_pane_focus: SessionPane,
    /// A tab is opening for someone other than the user ([`keeping_keyboard`]).
    pub background_open: bool,
    /// Blocks an extra `⌘W` right after closing the side region's last tab from closing the session.
    pub emptied_guard: bool,
    /// The side region's last tab, still drawn while closing it slides the region shut.
    pub closing: closing::State,
    /// Right-click context menu on a side tab: `(tab_key, x, y)`.
    pub tab_menu: Option<(String, f32, f32)>,
    /// `+` launcher dropdown menu anchor `(pane, x, y)`.
    pub add_menu: Option<(SessionPane, f32, f32)>,
    /// Each pane's tab strip scroll (by `SessionPane as usize`): what its handlers read, which a
    /// build never does.
    pub strips: Rc<[StripScroll; 3]>,
    pub sessions: HashMap<String, SessionSideState>,
    pub adding: Vec<NewTabEntry>,
    pub chosen_new_tab: HashMap<SessionPane, String>,
    pub(crate) next_new_serial: u64,
    /// The last [`resolve_side_region`]: the plugins' surface versions and the `Tracked`
    /// state it read, and what it resolved.
    pub(crate) resolved: RefCell<Option<ResolvedCache>>,
}

impl Default for SideState {
    fn default() -> Self {
        Self {
            chat_resize: Default::default(),
            split_resize: Default::default(),
            bottom_resize: Default::default(),
            empty_span: EmptySpan::Outside,
            laid_w: Cell::new(1280.0),
            edge_flight: None,
            pane_drag: Default::default(),
            focused: false,
            quiet: false,
            was_composing: false,
            pressed: false,
            active_pane_focus: SessionPane::Side,
            background_open: false,
            emptied_guard: false,
            closing: Default::default(),
            tab_menu: None,
            add_menu: None,
            strips: Rc::default(),
            resolved: Default::default(),
            sessions: HashMap::new(),
            adding: Vec::new(),
            chosen_new_tab: HashMap::new(),
            next_new_serial: 1,
        }
    }
}

/// A tab strip's scroll, as `tab-strip-scroll.ts` keeps it: the offset a wheel notch moves on
/// from, and the session, width and lead tab it last laid out with, so a strip that grows glides
/// to its new tab.
#[derive(Default)]
pub struct StripScroll {
    pub x: Cell<f32>,
    pub laid: RefCell<Option<(String, f32, bool)>>,
}

#[derive(Clone)]
pub struct ResolvedEntry {
    pub key: String,
    pub plugin: &'static str,
    pub tab: PanelTabDesc,
    pub provider: PanelRegistration,
    pub pane: SessionPane,
}

pub struct ResolvedRegion {
    pub session_id: String,
    pub directory: String,
    pub project_id: String,
    pub entries: Vec<ResolvedEntry>,
    pub strip: Vec<ResolvedEntry>,
    pub active: Option<String>,
    pub active_pane: SessionPane,
    pub wide: bool,
    pub launchers: Vec<(&'static str, PanelLauncherDesc, Launch)>,
}

impl ResolvedRegion {
    pub fn pane_entries(&self, pane: SessionPane) -> Vec<&ResolvedEntry> {
        self.strip.iter().filter(|e| e.pane == pane && !e.tab.hidden).collect()
    }

    pub fn pane_active(&self, pane: SessionPane, stored: &SessionTabs) -> Option<String> {
        let members: Vec<&ResolvedEntry> = self.strip.iter().filter(|e| e.pane == pane).collect();
        if let Some(act) = self.active.as_deref()
            && members.iter().any(|e| e.key == act)
        {
            return Some(act.to_owned());
        }
        if let Some(shown) = stored.selected.get(&pane)
            && members.iter().any(|e| e.key == *shown)
        {
            return Some(shown.clone());
        }
        if pane != SessionPane::Side {
            return members.first().map(|e| e.key.clone());
        }
        let eligible: Vec<&&ResolvedEntry> = members.iter().filter(|e| e.tab.fallback).collect();
        eligible
            .iter()
            .find(|e| !e.tab.pinned && !e.tab.first)
            .or_else(|| eligible.iter().find(|e| !e.tab.pinned && e.tab.first))
            .or_else(|| eligible.iter().find(|e| e.tab.pinned))
            .map(|e| e.key.clone())
            .or_else(|| members.first().map(|e| e.key.clone()))
    }

    pub fn entry(&self, key: &str) -> Option<&ResolvedEntry> {
        self.entries.iter().find(|e| e.key == key)
    }
}

/// The panel providers: each running plugin that adds panels, sorted by `(order, plugin)`.
pub fn all_panels(host: &dyn PanelsHost) -> Vec<PanelRegistration> {
    let mut out = host.panel_providers();
    out.sort_by_key(|p| (p.order, p.plugin));
    out
}

/// The provider of namespace `plugin`.
pub fn find_panel(host: &dyn PanelsHost, plugin: &str) -> Option<PanelRegistration> {
    all_panels(host).into_iter().find(|p| p.plugin == plugin)
}

/// The side region shows: open for the routed session, in a wide window (a narrow one folds
/// it into its views and keeps it open for when the window widens, as Desktop's `sideVisible`).
pub fn side_open(host: &dyn PanelsHost) -> bool {
    side_stored_open(host) && !host.narrow()
}

/// The routed session's side region is open, shown or folded away by a narrow window.
pub fn side_stored_open(host: &dyn PanelsHost) -> bool {
    host.active_session_id()
        .and_then(|id| host.side().sessions.get(id))
        .is_some_and(|st| st.visibility.open())
}

pub fn side_mounted(host: &dyn PanelsHost) -> bool {
    host.active_session_id()
        .and_then(|id| host.side().sessions.get(id))
        .is_some_and(|st| st.visibility.mounted())
}

/// The bottom pane shows: open for the routed session, in a wide window (`screen.tsx:545`).
pub fn bottom_open(host: &dyn PanelsHost) -> bool {
    bottom_stored_open(host) && !host.narrow()
}

pub fn bottom_stored_open(host: &dyn PanelsHost) -> bool {
    host.active_session_id()
        .and_then(|id| host.side().sessions.get(id))
        .is_some_and(|st| st.bottom_open)
}

/// Whether the side region and the bottom pane are on screen. The bottom pane can be stored
/// open with no tab in it, which draws nothing: its first tab opens it.
pub fn panes_shown(host: &dyn PanelsHost) -> (bool, bool) {
    let bottom = bottom_stored_open(host)
        && resolve_side_region(host).is_some_and(|r| !r.pane_entries(SessionPane::Bottom).is_empty());
    (side_stored_open(host), bottom)
}

pub fn session_side<'a>(host: &'a dyn PanelsHost, session_id: &str) -> Option<&'a SessionSideState> {
    host.side().sessions.get(session_id)
}

pub fn session_side_mut<'a>(host: &'a mut dyn PanelsHost, session_id: &str) -> &'a mut SessionSideState {
    host.side_mut().sessions.entry(session_id.to_owned()).or_default()
}

pub fn shown_new_tab<'a>(
    host: &'a dyn PanelsHost,
    session_id: &str,
    pane: SessionPane,
    pane_active: Option<&str>,
) -> Option<&'a NewTabEntry> {
    let side = host.side();
    let live: Vec<&NewTabEntry> = side
        .adding
        .iter()
        .filter(|n| n.session_id == session_id && n.pane == pane && n.over.as_deref() == pane_active)
        .collect();
    if let Some(chosen) = side.chosen_new_tab.get(&pane)
        && let Some(found) = live.iter().copied().find(|n| n.id == *chosen)
    {
        return Some(found);
    }
    live.last().copied()
}

pub fn live_new_tabs<'a>(
    host: &'a dyn PanelsHost,
    session_id: &str,
    pane: SessionPane,
    pane_active: Option<&str>,
) -> Vec<&'a NewTabEntry> {
    host.side()
        .adding
        .iter()
        .filter(|n| n.session_id == session_id && n.pane == pane && n.over.as_deref() == pane_active)
        .collect()
}

/// Where tab `key` is in `session`: 0 not in the strip, 1 listed, 2 its pane's shown tab
/// with the pane hidden, 3 on screen (Desktop's `layout.state`).
pub fn tab_state(host: &dyn PanelsHost, session: &str, key: &str) -> u64 {
    let Some(st) = host.side().sessions.get(session) else { return 0 };
    let tabs = &st.tab_state.tabs;
    if !tabs.all.iter().any(|k| k == key) {
        return 0;
    }
    let pane = session_tab_pane(tabs, key);
    if session_pane_tab(tabs, pane) != Some(key) {
        return 1;
    }
    let shown = match pane {
        SessionPane::Bottom => st.bottom_open,
        SessionPane::Side | SessionPane::Split => st.visibility.open(),
    };
    if shown { 3 } else { 2 }
}

/// The tab pane `pane` of the routed session shows, if any.
pub fn pane_shown_tab(host: &dyn PanelsHost, pane: SessionPane) -> Option<String> {
    let reg = resolve_side_region(host)?;
    let empty = SessionTabs::default();
    let stored = host.side().sessions.get(&reg.session_id).map_or(&empty, |s| &s.tab_state.tabs);
    reg.pane_active(pane, stored)
}

/// Whether `key` is its pane's shown tab and that pane is on screen (Desktop's `"visible"`).
pub fn side_tab_visible(host: &dyn PanelsHost, key: &str) -> bool {
    let Some(sid) = host.active_session_id() else { return false };
    let Some(st) = host.side().sessions.get(sid) else { return false };
    let pane = session_tab_pane(&st.tab_state.tabs, key);
    let open = if pane == SessionPane::Bottom { st.bottom_open } else { st.visibility.open() };
    open && pane_shown_tab(host, pane).as_deref() == Some(key)
}

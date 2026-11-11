//! The shell's layouts and their motion state: Desktop's Interface layout setting
//! (`appearance.tabLayout`), a switch between layouts while its flights run, and the collapsed
//! sidebar's hover peek (`sidebar-peek-machine.ts`). One tab store (`App::tabs`, `App::route`)
//! backs every layout; these say only how it shows. Both machines run on `App::clock`.
// Until the shell draws top tabs and the inbox, parts of this go unread.
#![allow(dead_code)]

use std::time::Duration;

use rune::Tx;

use crate::App;

/// Where the tabs live (`settings.json`'s `"tabLayout"`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TabLayout {
    /// "Top tabs": a strip in the titlebar. Desktop's default.
    #[default]
    Horizontal,
    /// "Side tabs": a list in the sidebar.
    Vertical,
    /// "Inbox": the sidebar sorts sessions by what needs you.
    Inbox,
}

impl TabLayout {
    pub const ALL: [TabLayout; 3] = [TabLayout::Horizontal, TabLayout::Vertical, TabLayout::Inbox];

    pub fn name(self) -> &'static str {
        match self {
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
            Self::Inbox => "inbox",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Horizontal => opencode_i18n::t!("settings.general.row.interfaceMode.top"),
            Self::Vertical => opencode_i18n::t!("settings.general.row.interfaceMode.side"),
            Self::Inbox => opencode_i18n::t!("settings.general.row.interfaceMode.inbox"),
        }
    }

    pub fn parse(s: Option<&str>) -> TabLayout {
        Self::ALL.into_iter().find(|l| Some(l.name()) == s).unwrap_or_default()
    }

    /// Side tabs and Inbox have a sidebar; top tabs have none, so ⇧⌘B does nothing there.
    pub fn has_sidebar(self) -> bool {
        matches!(self, Self::Vertical | Self::Inbox)
    }
}

/// How long a layout switch's flights run: the longest of them, `GLIDE`.
pub const SWITCH_SECS: f64 = 0.32;

/// A layout at rest, or a switch whose flights are still running. A switch begun mid-flight
/// retargets from the layout it was heading for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayoutMotionState {
    AtRest(TabLayout),
    Switching { from: TabLayout, to: TabLayout, started_at: f64 },
}

impl Default for LayoutMotionState {
    fn default() -> Self {
        Self::AtRest(TabLayout::default())
    }
}

impl LayoutMotionState {
    /// The layout shown, or being switched to.
    pub fn target(self) -> TabLayout {
        match self {
            Self::AtRest(l) | Self::Switching { to: l, .. } => l,
        }
    }

    fn switch(&mut self, to: TabLayout, now: f64) -> bool {
        let from = self.target();
        if from == to {
            return false;
        }
        *self = Self::Switching { from, to, started_at: now };
        true
    }

    fn due(self) -> Option<f64> {
        match self {
            Self::Switching { started_at, .. } => Some(started_at + SWITCH_SECS),
            Self::AtRest(_) => None,
        }
    }

    fn tick(&mut self, now: f64) -> bool {
        match self.due() {
            Some(at) if now + 1e-9 >= at => {
                *self = Self::AtRest(self.target());
                true
            }
            _ => false,
        }
    }
}

/// How long the pointer may stray past the sidebar and toggle before the peek closes.
pub const PEEK_GRACE: f64 = 0.1;
/// The slide out's fallback, when its spring never reports landing.
pub const PEEK_LEAVE_TIMEOUT: f64 = 0.8;
/// A dock from a peek's fallback, when the docked sidebar's flight never reports landing.
pub const PEEK_HANDOFF_TIMEOUT: f64 = 1.0;

/// The collapsed side-tabs or inbox sidebar's hover peek (`sidebar-peek-machine.ts`). Only
/// resting at the window's start edge brings it: the toggle docks the sidebar and never peeks
/// it. Each timed phase holds its deadline on `App::clock`, so a late timer from an earlier
/// phase finds nothing to do.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum SidebarPeekState {
    #[default]
    Idle,
    /// The docked sidebar closed with the pointer at the edge: no peek until it leaves the edge,
    /// or closing it there would bring it straight back.
    Suppressed,
    Dwelling { due_at: f64 },
    Peeking,
    Grace { due_at: f64 },
    Leaving { restore_focus: bool, due_at: f64 },
    HandingOff { due_at: f64 },
}

/// What the peek hears from the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeekEvent {
    /// The pointer entered or left the start edge's zone. `can_peek`: the sidebar is closed and
    /// nothing holds the pointer elsewhere. `held`: a menu or a rename holds an open peek.
    Edge { inside: bool, can_peek: bool, held: bool },
    /// The pointer entered or left the corridor over the sidebar and its toggle.
    Corridor { inside: bool, held: bool },
    /// What the machine hears of the pointer, which `Shell::peek_event` makes from the two zones:
    /// `can_start_edge`, it rests at the edge and a peek may start; `keep_open`, it's within
    /// the corridor, or the peek is held.
    Pointer { can_start_edge: bool, keep_open: bool },
    /// A press, drag or resize: cancels a dwell, leaves an open peek alone.
    Interrupt,
    Escape,
    Blur,
    /// Keyboard focus left the sidebar, its toggle and its menus.
    FocusLeft,
    DockOpened,
    DockClosed,
    /// The docked sidebar's flight landed, after a hand-off.
    DockSettled,
    /// The slide out landed.
    LeaveSettled,
    /// The sidebar is gone, as when tabs move to the titlebar.
    Disabled,
}

/// Where the pointer last was, by the peek's two zones.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct PeekPointer {
    at_edge: bool,
    can_peek: bool,
    in_corridor: bool,
    held: bool,
}

impl PeekPointer {
    fn heard(&mut self, event: PeekEvent) -> PeekEvent {
        match event {
            PeekEvent::Edge { inside, can_peek, held } => (self.at_edge, self.can_peek, self.held) = (inside, can_peek, held),
            PeekEvent::Corridor { inside, held } => (self.in_corridor, self.held) = (inside, held),
            _ => return event,
        }
        PeekEvent::Pointer { can_start_edge: self.can_start_edge(), keep_open: self.keep_open() }
    }

    fn can_start_edge(self) -> bool {
        self.at_edge && self.can_peek
    }

    fn keep_open(self) -> bool {
        self.in_corridor || self.held
    }
}

/// What a transition reads beside its event: the edge's dwell (`sidebar::PEEK_DELAY`), and
/// whether the pointer rests at the edge.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PeekCx {
    pub dwell: f64,
    pub at_edge: bool,
}

impl SidebarPeekState {
    /// Whether the sidebar shows as a peek: sliding in, open, sliding out, or handing off to
    /// the dock.
    pub fn shown(self) -> bool {
        matches!(self, Self::Peeking | Self::Grace { .. } | Self::Leaving { .. } | Self::HandingOff { .. })
    }

    /// Whether the peek is out over the panel, not sliding away or docking.
    pub fn open(self) -> bool {
        matches!(self, Self::Peeking | Self::Grace { .. })
    }

    pub fn handed_off(self) -> bool {
        matches!(self, Self::HandingOff { .. })
    }

    /// The next deadline, for a wake.
    pub fn due(self) -> Option<f64> {
        match self {
            Self::Dwelling { due_at } | Self::Grace { due_at } | Self::Leaving { due_at, .. } | Self::HandingOff { due_at } => Some(due_at),
            Self::Idle | Self::Suppressed | Self::Peeking => None,
        }
    }

    /// Applies `event` at `now`: Desktop's `transitionSidebarPeek`, without its toggle trigger.
    pub fn on(&mut self, event: PeekEvent, now: f64, cx: PeekCx) {
        use PeekEvent as E;
        let leaving = |restore_focus| Self::Leaving { restore_focus, due_at: now + PEEK_LEAVE_TIMEOUT };
        let handing_off = Self::HandingOff { due_at: now + PEEK_HANDOFF_TIMEOUT };
        let next = match (*self, event) {
            (_, E::Disabled) => Self::Idle,
            (Self::Idle | Self::Dwelling { .. }, E::DockClosed) if cx.at_edge => Self::Suppressed,
            (Self::Idle, E::Pointer { can_start_edge: true, .. }) => Self::Dwelling { due_at: now + cx.dwell },
            (Self::Suppressed, E::Pointer { .. }) if !cx.at_edge => Self::Idle,
            (Self::Suppressed, E::DockOpened) => Self::Idle,
            (Self::Dwelling { .. }, E::Pointer { can_start_edge: false, .. }) => Self::Idle,
            (Self::Dwelling { .. }, E::Interrupt | E::Escape | E::Blur | E::DockOpened | E::DockClosed) => Self::Idle,
            (Self::Peeking, E::Pointer { keep_open: false, .. }) => Self::Grace { due_at: now + PEEK_GRACE },
            (Self::Grace { .. }, E::Pointer { keep_open: true, .. }) => Self::Peeking,
            (Self::Peeking | Self::Grace { .. }, E::Escape) => leaving(true),
            (Self::Peeking | Self::Grace { .. }, E::Blur | E::FocusLeft | E::DockClosed) => leaving(false),
            (Self::Peeking | Self::Grace { .. }, E::DockOpened) => handing_off,
            // Back at the edge while it slides out: it turns around at once.
            (Self::Leaving { .. }, E::Pointer { can_start_edge: true, .. }) => Self::Peeking,
            (Self::Leaving { .. }, E::DockOpened) => handing_off,
            (Self::Leaving { .. }, E::LeaveSettled) => Self::Idle,
            (Self::HandingOff { .. }, E::DockClosed) => leaving(false),
            (Self::HandingOff { .. }, E::DockSettled) => Self::Idle,
            (state, _) => state,
        };
        *self = next;
    }

    /// Runs out a deadline that has passed. `ready`: the pointer still rests at the edge;
    /// `keep_open`: it's back within the corridor when the grace ends. True on a change.
    fn tick(&mut self, now: f64, ready: bool, keep_open: bool) -> bool {
        if self.due().is_none_or(|at| now + 1e-9 < at) {
            return false;
        }
        *self = match *self {
            Self::Dwelling { .. } if ready => Self::Peeking,
            Self::Grace { .. } if keep_open => Self::Peeking,
            Self::Grace { .. } => Self::Leaving { restore_focus: false, due_at: now + PEEK_LEAVE_TIMEOUT },
            _ => Self::Idle,
        };
        true
    }
}

/// The two machines, kept on `App::layout`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shell {
    pub motion: LayoutMotionState,
    pub peek: SidebarPeekState,
    /// The pointer's last sample, for transitions and deadlines that ask where it is.
    pointer: PeekPointer,
    /// The edge's dwell, read from its knob on each poll.
    dwell: f64,
    /// Events heard between polls, applied on the next one at its clock.
    queued: Vec<PeekEvent>,
    /// The narrow window's layout, views and drawers (`narrow`).
    pub narrow: crate::narrow::Narrow,
}

impl Shell {
    pub fn at_rest(layout: TabLayout) -> Shell {
        Shell { motion: LayoutMotionState::AtRest(layout), ..Shell::default() }
    }

    /// The peek hears `event` at `now`; the edge and the corridor update the pointer's sample
    /// and reach the machine as a `Pointer`.
    pub fn peek_event(&mut self, event: PeekEvent, now: f64) {
        let event = self.pointer.heard(event);
        let cx = PeekCx { dwell: self.dwell, at_edge: self.pointer.at_edge };
        self.peek.on(event, now, cx);
    }
}

impl App {
    /// The layout the shell shows, or is switching to.
    pub fn tab_layout(&self) -> TabLayout {
        self.layout.motion.target()
    }

    /// Settings' Interface layout: saves the choice and starts the switch. It ends a tab drag,
    /// a rename and the peek first, so nothing is mid-gesture while the tabs fly.
    pub fn set_tab_layout(&mut self, layout: TabLayout, tx: &mut Tx) {
        if !self.layout.motion.switch(layout, self.clock) {
            return;
        }
        self.change_settings(|s| s.tab_layout = layout);
        self.tab_drag.cancel();
        self.finish_rename(true);
        self.layout.peek_event(PeekEvent::Disabled, self.clock);
        // The Inbox shows its cards in the switch's own frame when it can, so the active
        // session's tab has a card to fly into.
        if layout == TabLayout::Inbox {
            crate::inbox_model::settle_now(self, tx);
        }
        tx.animated();
        // The tabs and Home, Search and New session fly between the titlebar and the sidebar,
        // which clip them at either end; they fly above both.
        tx.unclipped_flights();
        tx.wake_after(Duration::from_secs_f64(SWITCH_SECS));
    }

    /// Whether a layout switch's flights are still running.
    pub fn switching_layout(&self) -> bool {
        matches!(self.layout.motion, LayoutMotionState::Switching { .. })
    }

    /// The spring a layout switch flies the active session's tab or card on: Side tabs and
    /// the Inbox swap in place on `SETTLE` (`layouts-design.md` 4: B ↔ C), any switch with Top
    /// tabs glides on `GLIDE`. None at rest.
    pub fn switch_spring(&self) -> Option<rune::Spring> {
        let LayoutMotionState::Switching { from, to, .. } = self.layout.motion else { return None };
        let in_sidebar = from.has_sidebar() && to.has_sidebar();
        Some(if in_sidebar { rune::tok::motion::SETTLE } else { rune::tok::motion::GLIDE })
    }

    /// Hands the peek `event`, applied on the next poll at its clock: handlers don't see the
    /// clock, and a dwell must count from now.
    pub fn queue_peek(&mut self, event: PeekEvent, tx: &mut Tx) {
        if !self.shows_sidebar() {
            return;
        }
        self.layout.queued.push(event);
        tx.wake_after(Duration::ZERO);
    }
}

/// Runs out the machines' deadlines on the poll clock, and wakes for the next one.
pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let dwell = f64::from(s.knobs.get(&crate::view::PEEK_DELAY));
    let shell = &mut s.layout;
    shell.dwell = dwell;
    let was = shell.peek;
    for event in std::mem::take(&mut shell.queued) {
        shell.peek_event(event, now);
    }
    let heard = shell.peek != was;
    let (ready, keep_open) = (shell.pointer.can_start_edge(), shell.pointer.keep_open());
    let changed = heard | shell.motion.tick(now) | shell.peek.tick(now, ready, keep_open);
    if let Some(at) = [shell.motion.due(), shell.peek.due()].into_iter().flatten().reduce(f64::min) {
        tx.wake_after(Duration::from_secs_f64((at - now).max(0.0)));
    }
    changed
}

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
            Self::Horizontal => "Top tabs",
            Self::Vertical => "Side tabs",
            Self::Inbox => "Inbox",
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

/// What starts a peek: resting at the window's start edge, or on the collapsed toggle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeekTrigger {
    Edge,
    Toggle,
}

impl PeekTrigger {
    /// How long the pointer rests before the sidebar peeks (`SIDEBAR_PEEK_DWELL_MS`): the edge
    /// is quick, the toggle waits, as the header row is crossed far more often.
    pub fn dwell(self) -> f64 {
        match self {
            Self::Edge => 0.04,
            Self::Toggle => 0.5,
        }
    }
}

/// How long the pointer may stray past the sidebar and toggle before the peek closes.
pub const PEEK_GRACE: f64 = 0.1;
/// The slide out's fallback, when its spring never reports landing.
pub const PEEK_LEAVE_TIMEOUT: f64 = 0.8;
/// A dock from a peek's fallback, when the docked sidebar's flight never reports landing.
pub const PEEK_HANDOFF_TIMEOUT: f64 = 1.0;

/// The collapsed side-tabs or inbox sidebar's hover peek. Each timed phase holds its deadline
/// on `App::clock`, so a late timer from an earlier phase finds nothing to do.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum SidebarPeekState {
    #[default]
    Idle,
    Dwelling { from: PeekTrigger, due_at: f64 },
    Peeking,
    Grace { due_at: f64 },
    Leaving { restore_focus: bool, due_at: f64 },
    HandingOff { due_at: f64 },
}

/// What the peek hears from the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeekEvent {
    /// A pointer move, scroll or window exit. `can_start_edge`: it rests in the start edge's
    /// zone and a peek may start. `keep_open`: it's within the toggle-to-sidebar corridor, or
    /// the peek is held (typing, a menu, a drag).
    Pointer { can_start_edge: bool, keep_open: bool },
    /// The pointer entered the collapsed toggle; `can_peek` when nothing blocks a peek.
    ToggleEnter { can_peek: bool },
    ToggleLeave,
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

impl SidebarPeekState {
    /// Whether the sidebar shows as a peek: sliding in, open, or sliding out.
    pub fn shown(self) -> bool {
        matches!(self, Self::Peeking | Self::Grace { .. } | Self::Leaving { .. } | Self::HandingOff { .. })
    }

    pub fn handed_off(self) -> bool {
        matches!(self, Self::HandingOff { .. })
    }

    /// The next deadline, for a wake.
    pub fn due(self) -> Option<f64> {
        match self {
            Self::Dwelling { due_at, .. } | Self::Grace { due_at } | Self::Leaving { due_at, .. } | Self::HandingOff { due_at } => {
                Some(due_at)
            }
            Self::Idle | Self::Peeking => None,
        }
    }

    /// Applies `event` at `now`: Desktop's `transitionSidebarPeek`.
    pub fn on(&mut self, event: PeekEvent, now: f64) {
        use PeekEvent as E;
        let leaving = |restore_focus| Self::Leaving { restore_focus, due_at: now + PEEK_LEAVE_TIMEOUT };
        let handing_off = Self::HandingOff { due_at: now + PEEK_HANDOFF_TIMEOUT };
        let dwelling = |from: PeekTrigger| Self::Dwelling { from, due_at: now + from.dwell() };
        let next = match (*self, event) {
            (_, E::Disabled) => Self::Idle,
            (Self::Idle, E::Pointer { can_start_edge: true, .. }) => dwelling(PeekTrigger::Edge),
            (Self::Idle, E::ToggleEnter { can_peek: true }) => dwelling(PeekTrigger::Toggle),
            (Self::Dwelling { from, .. }, E::Pointer { can_start_edge, .. }) if from == PeekTrigger::Edge && !can_start_edge => Self::Idle,
            (Self::Dwelling { .. }, E::ToggleEnter { can_peek: false }) => Self::Idle,
            (Self::Dwelling { from: PeekTrigger::Edge, .. }, E::ToggleEnter { can_peek: true }) => dwelling(PeekTrigger::Toggle),
            (Self::Dwelling { from: PeekTrigger::Toggle, .. }, E::ToggleLeave) => Self::Idle,
            (Self::Dwelling { .. }, E::Interrupt | E::Escape | E::Blur | E::DockOpened | E::DockClosed) => Self::Idle,
            (Self::Peeking, E::Pointer { keep_open: false, .. }) => Self::Grace { due_at: now + PEEK_GRACE },
            (Self::Grace { .. }, E::Pointer { keep_open: true, .. } | E::ToggleEnter { can_peek: true }) => Self::Peeking,
            (Self::Peeking | Self::Grace { .. }, E::Escape) => leaving(true),
            (Self::Peeking | Self::Grace { .. }, E::Blur | E::FocusLeft | E::DockClosed) => leaving(false),
            (Self::Peeking | Self::Grace { .. }, E::DockOpened) => handing_off,
            // Back at the edge or on the toggle while it slides out: it turns around at once.
            (Self::Leaving { .. }, E::Pointer { can_start_edge: true, .. } | E::ToggleEnter { can_peek: true }) => Self::Peeking,
            (Self::Leaving { .. }, E::DockOpened) => handing_off,
            (Self::Leaving { .. }, E::LeaveSettled) => Self::Idle,
            (Self::HandingOff { .. }, E::DockClosed) => leaving(false),
            (Self::HandingOff { .. }, E::DockSettled) => Self::Idle,
            (state, _) => state,
        };
        *self = next;
    }

    /// Runs out a deadline that has passed. `ready`: the pointer still rests where its dwell
    /// began; `keep_open`: it's back within the corridor when the grace ends. True on a change.
    pub fn tick(&mut self, now: f64, ready: bool, keep_open: bool) -> bool {
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
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shell {
    pub motion: LayoutMotionState,
    pub peek: SidebarPeekState,
    /// The pointer's last sample, for deadlines that ask whether it still qualifies.
    pointer: (bool, bool),
}

impl Shell {
    pub fn at_rest(layout: TabLayout) -> Shell {
        Shell { motion: LayoutMotionState::AtRest(layout), ..Shell::default() }
    }

    /// The peek hears `event` at `now`, keeping a pointer sample for its deadlines.
    pub fn peek_event(&mut self, event: PeekEvent, now: f64) {
        if let PeekEvent::Pointer { can_start_edge, keep_open } = event {
            self.pointer = (can_start_edge, keep_open);
        }
        self.peek.on(event, now);
    }
}

impl App {
    /// The layout the shell shows, or is switching to.
    pub fn tab_layout(&self) -> TabLayout {
        self.layout.motion.target()
    }

    /// Settings' Interface layout: saves the choice and starts the switch, which cancels any
    /// peek when the new layout has no sidebar.
    pub fn set_tab_layout(&mut self, layout: TabLayout, tx: &mut Tx) {
        if !self.layout.motion.switch(layout, self.clock) {
            return;
        }
        self.change_settings(|s| s.tab_layout = layout);
        if !layout.has_sidebar() {
            self.layout.peek_event(PeekEvent::Disabled, self.clock);
        }
        tx.wake_after(Duration::from_secs_f64(SWITCH_SECS));
    }
}

/// Runs out the machines' deadlines on the poll clock, and wakes for the next one.
pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let shell = &mut s.layout;
    let (edge, keep_open) = shell.pointer;
    let ready = match shell.peek {
        SidebarPeekState::Dwelling { from: PeekTrigger::Edge, .. } => edge,
        _ => true,
    };
    let changed = shell.motion.tick(now) | shell.peek.tick(now, ready, keep_open);
    if let Some(at) = [shell.motion.due(), shell.peek.due()].into_iter().flatten().reduce(f64::min) {
        tx.wake_after(Duration::from_secs_f64((at - now).max(0.0)));
    }
    changed
}

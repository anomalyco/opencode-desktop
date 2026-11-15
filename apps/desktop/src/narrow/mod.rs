//! Desktop's narrow window (below 768 CSS px): the session's conversation goes edge to edge
//! under a Session / Changes / More switcher, the side panel and the bottom pane fold into it,
//! the side-tabs and inbox sidebar hide, and the titlebar's tabs become one trigger that opens
//! a drawer of them (`shell.tsx`, `titlebar.tsx`, `session/screen.tsx`, `runtime/plugin/mobile.tsx`).
//! The machines are in `machine`; this holds them for the window and feeds them.

pub mod machine;
pub mod session;
pub mod sheet;
pub mod tabs;

use std::cell::{Cell, RefCell};
use std::time::Duration;

use rune::{Key, KeyEvent, Tx};

use crate::{App, Route};
use machine::{
    Mode, ModeEffect, ModeEvent, MoreDrawer, MoreEvent, Presence, PresenceEffect, PresenceEvent, TabsDrawer, TabsEffect, TabsEvent, View,
    ViewEffect, ViewEvent,
};

/// The window's narrow layout: its mode and the views and drawers it shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Narrow {
    /// Stepped by the view, where the shell's width is measured (the media query's input).
    mode: Cell<Mode>,
    /// The shell's width at the last layout, in CSS px.
    width: Cell<f32>,
    /// Crossings the view stepped, for the next poll or layout to hand the drawers.
    crossed: RefCell<Vec<ModeEffect>>,
    /// A real wide-to-narrow crossing replaces the conversation's scroll viewport. Its first
    /// layout consumes the row and its old window y. Never persisted as a session restore.
    pub(crate) reading: RefCell<Option<(String, String, f32)>>,
    pub view: View,
    /// The routed session the view machine last heard of.
    routed: RefCell<Option<String>>,
    pub tabs: TabsDrawer,
    /// The tabs drawer's close timers so far, naming the latest.
    tab_attempts: u32,
    pub tabs_presence: Presence,
    pub more: MoreDrawer,
    pub more_presence: Presence,
    pub(super) pull: sheet::Pull,
}

impl Narrow {
    /// Hears the shell's width (window points over zoom) from the view, as Desktop's media
    /// query does, and says whether the window is narrow.
    pub fn measure(&self, width: f32) -> bool {
        self.width.set(width);
        let (mode, effect) = self.mode.get().step(ModeEvent::Measured { width });
        self.mode.set(mode);
        self.crossed.borrow_mut().extend(effect);
        mode == Mode::Narrow
    }

    pub fn is_narrow(&self) -> bool {
        self.mode.get() == Mode::Narrow
    }

    /// The shell's width at the last layout.
    pub fn width(&self) -> f32 {
        self.width.get()
    }

    /// The window is at least 640 wide: Tailwind's `sm:` (`min-width: 640px`); below it the
    /// `max-width: 639px` rules apply. Before the first layout it counts as wide.
    pub fn sm(&self) -> bool {
        let w = self.width.get();
        w <= 0.0 || w >= 640.0
    }
}

impl App {
    /// The window is below Desktop's 768 px breakpoint.
    pub fn narrow(&self) -> bool {
        self.layout.narrow.is_narrow() && self.stage.is_none()
    }

    /// The side-tabs or inbox sidebar shows: its layout has one, and the window is wide.
    pub fn shows_sidebar(&self) -> bool {
        self.tab_layout().has_sidebar() && !self.narrow()
    }

    /// The view the routed session shows in a narrow window: "session" or a panel's plugin.
    pub fn narrow_view(&self) -> &str {
        let routed = match &*self.route {
            Route::Session { id, .. } if !self.settings.open => Some(id.as_str()),
            _ => None,
        };
        self.layout.narrow.view.shown(routed)
    }

    /// Picks the narrow view `view` for `session` (Session, Changes, a More row, or a panel an
    /// open asked for).
    pub fn select_narrow_view(&mut self, session: &str, view: &str, tx: &mut Tx) {
        tx.instant();
        let routed = self.active_session_id().map(str::to_owned);
        let event = ViewEvent::Select { session: session.to_owned(), view: view.to_owned(), routed, now: *self.clock };
        let (next, effect) = std::mem::take(&mut self.layout.narrow.view).step(event);
        self.layout.narrow.view = next;
        if let Some(ViewEffect::WakeAt(at)) = effect {
            tx.wake_after(Duration::from_secs_f64((at - *self.clock).max(0.0)));
        }
    }

    /// An open of tab `key` in `session` while the window is narrow (`host-apis.tsx` `place`):
    /// it picks its panel's narrow view rather than opening the side region or bottom pane,
    /// which keep what they were (`was`: side, bottom) for when the window widens.
    pub fn place_narrow(&mut self, session: &str, key: &str, was: (crate::panels::state::Visibility, bool)) {
        if !self.narrow() {
            return;
        }
        let st = self.session_side_mut(session);
        (st.visibility, st.bottom_open) = was;
        let Some(view) = session::view_for_tab(self, key) else { return };
        let routed = self.active_session_id().map(str::to_owned);
        let event = ViewEvent::Select { session: session.to_owned(), view, routed, now: *self.clock };
        self.layout.narrow.view = std::mem::take(&mut self.layout.narrow.view).step(event).0;
    }

    /// The tabs drawer hears `event`.
    pub fn tabs_drawer(&mut self, event: TabsEvent, tx: &mut Tx) {
        let (next, effect) = self.layout.narrow.tabs.step(event, &mut self.layout.narrow.tab_attempts);
        self.layout.narrow.tabs = next;
        match effect {
            Some(TabsEffect::WakeAt(at)) => tx.wake_after(Duration::from_secs_f64((at - *self.clock).max(0.0))),
            Some(TabsEffect::OpenSettings) => self.open_settings(),
            None => {}
        }
        let narrow = &mut self.layout.narrow;
        if !narrow.tabs.is_open() { narrow.pull.clear(tabs::SHEET); }
        present(&mut narrow.tabs_presence, narrow.tabs.is_open(), *self.clock, tx);
    }

    /// The More drawer hears `event`.
    pub fn more_drawer(&mut self, event: MoreEvent, tx: &mut Tx) {
        self.layout.narrow.more = std::mem::take(&mut self.layout.narrow.more).step(event);
        let narrow = &mut self.layout.narrow;
        if !narrow.more.is_open() { narrow.pull.clear(session::MORE_SHEET); }
        present(&mut narrow.more_presence, narrow.more.is_open(), *self.clock, tx);
    }
}

/// A drawer's presence follows its machine's open state.
fn present(presence: &mut Presence, open: bool, now: f64, tx: &mut Tx) {
    let (next, effect) = presence.step(PresenceEvent::Open { open, now });
    if next != *presence {
        tx.animated();
    }
    *presence = next;
    if let Some(PresenceEffect::WakeAt(at)) = effect {
        tx.wake_after(Duration::from_secs_f64((at - now).max(0.0)));
    }
}

/// Hands the drawers the crossings the view measured. True when anything changed.
pub fn settle(s: &mut App, tx: &mut Tx) -> bool {
    let crossed: Vec<ModeEffect> = s.layout.narrow.crossed.borrow_mut().drain(..).collect();
    for ModeEffect::Crossed { narrow } in &crossed {
        s.tabs_drawer(TabsEvent::ViewportChanged { narrow: *narrow }, tx);
        if !narrow {
            s.more_drawer(MoreEvent::Close, tx);
        }
    }
    !crossed.is_empty()
}

/// Deliver a route change with the navigation that caused it. Remembering the route is
/// bookkeeping, not a change to what the shell draws.
pub(crate) fn follow_route(s: &mut App, tx: &mut Tx) -> bool {
    let routed = match &*s.route {
        Route::Session { id, .. } => Some(id.as_str()),
        _ => None,
    };
    if routed == s.layout.narrow.routed.borrow().as_deref() { return false; }
    s.layout.narrow.routed.replace(routed.map(str::to_owned));
    let view = !matches!(s.layout.narrow.view, View::Conversation);
    let tabs = s.layout.narrow.tabs != TabsDrawer::Closed;
    let more = s.layout.narrow.more.is_open();
    if view {
        let routed = routed.map(str::to_owned);
        s.layout.narrow.view = std::mem::take(&mut s.layout.narrow.view).step(ViewEvent::RouteChanged { routed }).0;
    }
    if tabs { s.tabs_drawer(TabsEvent::RouteChanged, tx); }
    if more { s.more_drawer(MoreEvent::Close, tx); }
    s.narrow() && (view || tabs || more)
}

/// Runs the machines' clocks and catches routes set outside session navigation.
pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let mut changed = settle(s, tx);
    let wrap = s.settings.mobile_diff_wrap;
    if s.review.sessions.values().any(|sr| sr.narrow.wraps_otherwise(wrap)) {
        for sr in s.review.sessions.values_mut() {
            sr.narrow.set_wrap(wrap);
        }
    }
    changed |= follow_route(s, tx);
    // The clock moves only a pending view and a drawer sliding out: polls borrow the shell to
    // write only then, so regions that read it stay reused.
    if matches!(s.layout.narrow.view, View::Pending { due_at, .. } if now + 1e-9 >= due_at) {
        let view = std::mem::take(&mut s.layout.narrow.view);
        let (view, _) = view.step(ViewEvent::Tick { now });
        s.layout.narrow.view = view;
    }
    if let TabsDrawer::ClosingToSettings { attempt, due_at } = s.layout.narrow.tabs
        && now + 1e-9 >= due_at
    {
        s.tabs_drawer(TabsEvent::CloseTimedOut { attempt }, tx);
        changed = true;
    }
    if matches!(s.layout.narrow.more_presence, Presence::Leaving { .. }) {
        let (more, _) = s.layout.narrow.more_presence.step(PresenceEvent::Tick { now });
        changed |= more != s.layout.narrow.more_presence;
        s.layout.narrow.more_presence = more;
    }
    if matches!(s.layout.narrow.tabs_presence, Presence::Leaving { .. }) {
        let (tabs, effect) = s.layout.narrow.tabs_presence.step(PresenceEvent::Tick { now });
        changed |= tabs != s.layout.narrow.tabs_presence;
        s.layout.narrow.tabs_presence = tabs;
        if effect == Some(PresenceEffect::Gone) {
            s.tabs_drawer(TabsEvent::ContentPresent { present: false }, tx);
        }
    }
    changed
}

/// Escape closes an open drawer, as corvu's does.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key != Key::Escape || !s.narrow() {
        return false;
    }
    if s.layout.narrow.tabs.is_open() {
        s.tabs_drawer(TabsEvent::Changed { open: false }, tx);
        return true;
    }
    if s.layout.narrow.more.is_open() {
        s.more_drawer(MoreEvent::Close, tx);
        return true;
    }
    false
}

/// The timeline's column in a narrow window: the panel edge to edge, 1 px in from each side,
/// less the rows' 16 px padding (`px-4`, no `md:max-w-session`).
pub fn content_width(s: &App) -> Option<f32> {
    s.narrow().then(|| (s.layout.narrow.width() - 2.0 - 32.0).max(0.0))
}

//! Desktop's narrow window (below 768 CSS px): the session's conversation goes edge to edge
//! under a Session / Changes / More switcher, the side panel and the bottom pane fold into it,
//! the side-tabs and inbox sidebar hide, and the titlebar's tabs become one trigger that opens
//! a drawer of them (`shell.tsx`, `titlebar.tsx`, `session/screen.tsx`, `runtime/plugin/mobile.tsx`).
//!
//! The machines live in [`machine`]; every handler operates on non-generic `&dyn NarrowHost` /
//! `&mut dyn NarrowHost`, keeping thin `H: NarrowHost + 'static` wrappers strictly on `rune::El`
//! view callbacks in [`sheet`], [`session`], and [`tabs`].

#![forbid(unsafe_code)]

pub mod host;
pub mod machine;
pub mod session;
pub mod sheet;
pub mod tabs;

use std::cell::{Cell, RefCell};
use std::time::Duration;

use rune::{Key, KeyEvent, Tx};

pub use host::{NarrowChrome, NarrowHost, NarrowTab, NarrowTabStatus};
pub use machine::{
    BREAKPOINT, CONVERSATION, Mode, ModeEffect, ModeEvent, MoreDrawer, MoreEvent, Nested, PENDING_TTL, Presence,
    PresenceEffect, PresenceEvent, SLIDE_OUT, TABS_CLOSE_TIMEOUT, TabsDrawer, TabsEffect, TabsEvent, View, ViewEffect,
    ViewEvent,
};
pub use session::{Kind, MORE_SHEET, NarrowView};
pub use tabs::SHEET as TABS_SHEET;

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
    pub reading: RefCell<Option<(String, String, f32)>>,
    pub view: View,
    /// The routed session the view machine last heard of.
    routed: RefCell<Option<String>>,
    pub tabs: TabsDrawer,
    /// The tabs drawer's close timers so far, naming the latest.
    tab_attempts: u32,
    pub tabs_presence: Presence,
    pub more: MoreDrawer,
    pub more_presence: Presence,
    pub(crate) pull: sheet::Pull,
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

/// The window is below Desktop's 768 px breakpoint (and no Storybook stage covers it).
pub fn narrow(host: &dyn NarrowHost) -> bool {
    host.narrow_state().is_narrow() && !host.stage_active()
}

/// The side-tabs or inbox sidebar shows: its layout has one, and the window is wide.
pub fn shows_sidebar(host: &dyn NarrowHost) -> bool {
    host.layout_has_sidebar() && !narrow(host)
}

/// The view the routed session shows in a narrow window: `"session"` or a panel's plugin.
pub fn narrow_view(host: &dyn NarrowHost) -> &str {
    let routed = if host.settings_open() { None } else { host.active_session_id() };
    host.narrow_state().view.shown(routed)
}

/// Picks the narrow view `view` for `session` (Session, Changes, a More row, or a panel an
/// open asked for).
pub fn select_narrow_view(host: &mut dyn NarrowHost, session: &str, view: &str, tx: &mut Tx) {
    tx.instant();
    let routed = host.active_session_id().map(str::to_owned);
    let now = host.clock();
    let event = ViewEvent::Select { session: session.to_owned(), view: view.to_owned(), routed, now };
    let narrow = host.narrow_state_mut();
    let (next, effect) = std::mem::take(&mut narrow.view).step(event);
    narrow.view = next;
    if let Some(ViewEffect::WakeAt(at)) = effect {
        tx.wake_after(Duration::from_secs_f64((at - now).max(0.0)));
    }
}

/// An open of tab `key` in `session` while the window is narrow (`host-apis.tsx` `place`):
/// picks its panel's narrow view instead of opening the side region or bottom pane.
/// Returns `true` when the window is narrow (so the caller restores the side/bottom pane
/// visibility from before opening `key`).
pub fn place_narrow(host: &mut dyn NarrowHost, session: &str, key: &str) -> bool {
    if !narrow(host) {
        return false;
    }
    if let Some(view) = session::view_for_tab(host, key) {
        let routed = host.active_session_id().map(str::to_owned);
        let now = host.clock();
        let event = ViewEvent::Select { session: session.to_owned(), view, routed, now };
        host.narrow_state_mut().view = std::mem::take(&mut host.narrow_state_mut().view).step(event).0;
    }
    true
}

/// The tabs drawer hears `event`.
pub fn tabs_drawer(host: &mut dyn NarrowHost, event: TabsEvent, tx: &mut Tx) {
    let effect = {
        let narrow = host.narrow_state_mut();
        let (next, effect) = narrow.tabs.step(event, &mut narrow.tab_attempts);
        narrow.tabs = next;
        effect
    };
    let now = host.clock();
    match effect {
        Some(TabsEffect::WakeAt(at)) => tx.wake_after(Duration::from_secs_f64((at - now).max(0.0))),
        Some(TabsEffect::OpenSettings) => host.open_settings(),
        None => {}
    }
    let narrow = host.narrow_state_mut();
    let open = narrow.tabs.is_open();
    if !open {
        narrow.pull.clear(tabs::SHEET);
    }
    present(&mut narrow.tabs_presence, open, now, tx);
}

/// The More drawer hears `event`.
pub fn more_drawer(host: &mut dyn NarrowHost, event: MoreEvent, tx: &mut Tx) {
    let now = host.clock();
    let narrow = host.narrow_state_mut();
    narrow.more = std::mem::take(&mut narrow.more).step(event);
    let open = narrow.more.is_open();
    if !open {
        narrow.pull.clear(session::MORE_SHEET);
    }
    present(&mut narrow.more_presence, open, now, tx);
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
pub fn settle(host: &mut dyn NarrowHost, tx: &mut Tx) -> bool {
    let crossed: Vec<ModeEffect> = host.narrow_state().crossed.borrow_mut().drain(..).collect();
    for ModeEffect::Crossed { narrow } in &crossed {
        tabs_drawer(host, TabsEvent::ViewportChanged { narrow: *narrow }, tx);
        if !narrow {
            more_drawer(host, MoreEvent::Close, tx);
        }
    }
    !crossed.is_empty()
}

/// Deliver a route change with the navigation that caused it. Remembering the route is
/// bookkeeping, not a change to what the shell draws.
pub fn follow_route(host: &mut dyn NarrowHost, tx: &mut Tx) -> bool {
    let routed = host.active_session_id().map(str::to_owned);
    if routed.as_deref() == host.narrow_state().routed.borrow().as_deref() {
        return false;
    }
    host.narrow_state().routed.replace(routed.clone());
    let view = !matches!(host.narrow_state().view, View::Conversation);
    let tabs = host.narrow_state().tabs != TabsDrawer::Closed;
    let more = host.narrow_state().more.is_open();
    if view {
        let narrow = host.narrow_state_mut();
        narrow.view = std::mem::take(&mut narrow.view).step(ViewEvent::RouteChanged { routed }).0;
    }
    if tabs {
        tabs_drawer(host, TabsEvent::RouteChanged, tx);
    }
    if more {
        more_drawer(host, MoreEvent::Close, tx);
    }
    narrow(host) && (view || tabs || more)
}

/// Runs the machines' clocks and catches routes set outside session navigation.
pub fn poll(host: &mut dyn NarrowHost, now: f64, tx: &mut Tx) -> bool {
    let mut changed = settle(host, tx);
    host.sync_narrow_review_wrap();
    changed |= follow_route(host, tx);
    // The clock moves only a pending view and a drawer sliding out: polls borrow the shell to
    // write only then, so regions that read it stay reused.
    let pending_due = matches!(host.narrow_state().view, View::Pending { due_at, .. } if now + 1e-9 >= due_at);
    if pending_due {
        let narrow = host.narrow_state_mut();
        let view = std::mem::take(&mut narrow.view);
        let (view, _) = view.step(ViewEvent::Tick { now });
        narrow.view = view;
    }
    let cur_tabs_drawer = host.narrow_state().tabs;
    if let TabsDrawer::ClosingToSettings { attempt, due_at } = cur_tabs_drawer
        && now + 1e-9 >= due_at
    {
        tabs_drawer(host, TabsEvent::CloseTimedOut { attempt }, tx);
        changed = true;
    }
    let cur_more = host.narrow_state().more_presence;
    if matches!(cur_more, Presence::Leaving { .. }) {
        let (more, _) = cur_more.step(PresenceEvent::Tick { now });
        if more != cur_more {
            host.narrow_state_mut().more_presence = more;
            changed = true;
        }
    }
    let cur_tabs = host.narrow_state().tabs_presence;
    if matches!(cur_tabs, Presence::Leaving { .. }) {
        let (tabs, effect) = cur_tabs.step(PresenceEvent::Tick { now });
        if tabs != cur_tabs {
            host.narrow_state_mut().tabs_presence = tabs;
            changed = true;
        }
        if effect == Some(PresenceEffect::Gone) {
            tabs_drawer(host, TabsEvent::ContentPresent { present: false }, tx);
        }
    }
    changed
}

/// Escape closes an open drawer, as corvu's does.
pub fn key(host: &mut dyn NarrowHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key != Key::Escape || !narrow(host) {
        return false;
    }
    if host.narrow_state().tabs.is_open() {
        tabs_drawer(host, TabsEvent::Changed { open: false }, tx);
        return true;
    }
    if host.narrow_state().more.is_open() {
        more_drawer(host, MoreEvent::Close, tx);
        return true;
    }
    false
}

/// The timeline's column in a narrow window: the panel edge to edge, 1 px in from each side,
/// less the rows' 16 px padding (`px-4`, no `md:max-w-session`).
pub fn content_width(host: &dyn NarrowHost) -> Option<f32> {
    narrow(host).then(|| (host.narrow_state().width() - 2.0 - 32.0).max(0.0))
}

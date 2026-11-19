//! The narrow window's machines, each one `enum` and a `step` that returns its effects:
//!
//! - [`Mode`]: wide or narrow, from the shell's width against Desktop's 768 px breakpoint
//!   (`(max-width: 767px)`, `shell.tsx:44`, `session/model.ts:28`). Desktop has no machine of its
//!   own here, only the media query; leaving narrow closes both drawers, as unmounting them does.
//! - [`View`]: which view the routed session shows while narrow: the conversation or a panel's
//!   (`host-layout-machine.ts` `MobileViewState`, `transitionMobileView`).
//! - [`TabsDrawer`]: the titlebar's tabs drawer (`mobile-tabs-machine.ts`).
//! - [`MoreDrawer`]: the session's More drawer (`mobile.tsx` `MobileViewTabs`' store).
//! - [`Presence`]: a drawer sliding in and out (corvu's `data-transitioning`, 240 ms in, 200 out),
//!   whose end is the tabs drawer's `content.present`.

/// Desktop's breakpoint: narrow below 768 CSS px.
pub const BREAKPOINT: f32 = 768.0;

/// How long a view picked for a session that isn't routed waits for its visit
/// (`MOBILE_VIEW_PENDING_TTL_MS`).
pub const PENDING_TTL: f64 = 5.0;

/// The tabs drawer's slide out takes 200 ms; Settings opens anyway if it never reports its end
/// (`MOBILE_TABS_CLOSE_TIMEOUT_MS`).
pub const TABS_CLOSE_TIMEOUT: f64 = 0.5;

/// A drawer's slide out (`mobile-drawer.css`: `[data-closing] { transition-duration: 200ms }`).
pub const SLIDE_OUT: f64 = opencode_ui::sheet::SLIDE_OUT as f64;

// ---------------------------------------------------------------------------------------------
// Mode

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Wide,
    Narrow,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModeEvent {
    /// The shell measured `width` CSS px (window points over the app's zoom).
    Measured { width: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeEffect {
    /// The window became narrow or wide: the drawers hear it (`viewport.changed`).
    Crossed { narrow: bool },
}

impl Mode {
    pub fn step(self, event: ModeEvent) -> (Mode, Option<ModeEffect>) {
        let ModeEvent::Measured { width } = event;
        // No fractional width is both narrow and wide (`host-apis.tsx:135`).
        let next = if width < BREAKPOINT { Mode::Narrow } else { Mode::Wide };
        if next == self {
            return (self, None);
        }
        (next, Some(ModeEffect::Crossed { narrow: next == Mode::Narrow }))
    }
}

// ---------------------------------------------------------------------------------------------
// View

/// The session's view in a narrow window. `Pending` is a view picked for a session that isn't
/// routed (a link that opens Files on another session): it belongs to the next visit only if
/// that visit routes its session before `due_at`.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum View {
    #[default]
    Conversation,
    Active { session: String, view: String },
    Pending { session: String, view: String, due_at: f64 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ViewEvent {
    /// `view` picked for `session` ("session" is the conversation), with `routed` routed now.
    Select { session: String, view: String, routed: Option<String>, now: f64 },
    RouteChanged { routed: Option<String> },
    /// The clock passed a pending view's deadline.
    Tick { now: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewEffect {
    /// Wake at this clock time to expire a pending view.
    WakeAt(f64),
}

/// The conversation's key.
pub const CONVERSATION: &str = "session";

impl View {
    pub fn step(self, event: ViewEvent) -> (View, Option<ViewEffect>) {
        match event {
            ViewEvent::Select { view, .. } if view == CONVERSATION => (View::Conversation, None),
            ViewEvent::Select { session, view, routed, .. } if routed.as_deref() == Some(session.as_str()) => (View::Active { session, view }, None),
            ViewEvent::Select { session, view, now, .. } => {
                let due_at = now + PENDING_TTL;
                (View::Pending { session, view, due_at }, Some(ViewEffect::WakeAt(due_at)))
            }
            ViewEvent::Tick { now } => match self {
                View::Pending { due_at, .. } if now + 1e-9 >= due_at => (View::Conversation, None),
                other => (other, None),
            },
            ViewEvent::RouteChanged { routed } => match self {
                View::Active { ref session, .. } if routed.as_deref() == Some(session.as_str()) => (self, None),
                // The navigation that follows a pending view starts its visit; any other visit
                // reads as the conversation.
                View::Pending { session, view, .. } if routed.as_deref() == Some(session.as_str()) => (View::Active { session, view }, None),
                _ => (View::Conversation, None),
            },
        }
    }

    /// The view the routed session shows (`selectMobileView`).
    pub fn shown(&self, routed: Option<&str>) -> &str {
        match self {
            View::Active { session, view } if Some(session.as_str()) == routed => view,
            _ => CONVERSATION,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Tabs drawer (`mobile-tabs-machine.ts`)

/// `closing-to-settings` lets the drawer slide out before Settings opens, so the two don't
/// fight over focus. `attempt` names the latest close timer, whose expiry (`due_at`) alone
/// applies.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum TabsDrawer {
    #[default]
    Closed,
    Open,
    ClosingToSettings { attempt: u32, due_at: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TabsEvent {
    /// The drawer asks to open or close (the trigger, an outside press, Escape).
    Changed { open: bool },
    /// A tab, Home, New session or Help was picked.
    Close,
    SettingsRequested { now: f64 },
    /// The drawer's content finished sliding out (`present: false`) or showed.
    ContentPresent { present: bool },
    CloseTimedOut { attempt: u32 },
    RouteChanged,
    ViewportChanged { narrow: bool },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TabsEffect {
    /// Start the close timer: wake at this clock time.
    WakeAt(f64),
    OpenSettings,
}

impl TabsDrawer {
    pub fn is_open(self) -> bool {
        self == TabsDrawer::Open
    }

    pub fn step(self, event: TabsEvent, attempts: &mut u32) -> (TabsDrawer, Option<TabsEffect>) {
        use TabsDrawer as S;
        use TabsEvent as E;
        let closing = matches!(self, S::ClosingToSettings { .. });
        match event {
            // A close the drawer reports while it slides out to Settings is the one under way.
            E::Changed { open } if open == self.is_open() || (closing && !open) => (self, None),
            E::Changed { open } => (if open { S::Open } else { S::Closed }, None),
            E::Close | E::RouteChanged if self == S::Closed => (self, None),
            E::Close | E::RouteChanged => (S::Closed, None),
            E::SettingsRequested { .. } if self != S::Open => (self, None),
            E::SettingsRequested { now } => {
                *attempts += 1;
                let due_at = now + TABS_CLOSE_TIMEOUT;
                (S::ClosingToSettings { attempt: *attempts, due_at }, Some(TabsEffect::WakeAt(due_at)))
            }
            E::ContentPresent { present } if !closing || present => (self, None),
            E::ContentPresent { .. } => (S::Closed, Some(TabsEffect::OpenSettings)),
            E::CloseTimedOut { attempt } => match self {
                S::ClosingToSettings { attempt: a, .. } if a == attempt => (S::Closed, Some(TabsEffect::OpenSettings)),
                _ => (self, None),
            },
            // Leaving the narrow layout unmounts the drawer before it can report its exit.
            E::ViewportChanged { narrow } if narrow || self == S::Closed => (self, None),
            E::ViewportChanged { .. } => (S::Closed, closing.then_some(TabsEffect::OpenSettings)),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// More drawer (`MobileViewTabs`)

/// The More drawer: shut, its menu of views, or one `drawer` kind view inside it, which may show
/// a nested view of its own in its place (`mobile-panel-drawer.tsx`'s `view`). Shut from a
/// view, it keeps drawing that view while it slides away, as Desktop's store keeps `drawer`
/// until the next open.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum MoreDrawer {
    #[default]
    Closed,
    ClosedFrom { view: String, nested: Option<Nested> },
    Menu,
    Panel { view: String, nested: Option<Nested> },
}

/// A nested view a drawer kind view opened in the drawer (`useDrawer().open`): its title, and
/// what its panel draws it from.
#[derive(Clone, Debug, PartialEq)]
pub struct Nested {
    pub title: String,
    pub data: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MoreEvent {
    /// More pressed: the menu, from the top.
    Open,
    /// A `drawer` kind view picked from the menu.
    Drill { view: String },
    /// The drawer kind view `view` opened a nested view (`navigation.open`).
    Nest { view: String, nested: Nested },
    /// The nested view's back button (`navigation.back`).
    Back,
    /// A `menu` kind view picked, an outside press, Escape, or the window going wide.
    Close,
}

impl MoreDrawer {
    pub fn is_open(&self) -> bool {
        matches!(self, MoreDrawer::Menu | MoreDrawer::Panel { .. })
    }

    /// The `drawer` kind view it draws: open on it, or sliding away from it.
    pub fn drawn_view(&self) -> Option<&str> {
        match self {
            MoreDrawer::Panel { view, .. } | MoreDrawer::ClosedFrom { view, .. } => Some(view),
            MoreDrawer::Closed | MoreDrawer::Menu => None,
        }
    }

    /// The nested view drawn in place of the drawer kind view.
    pub fn nested(&self) -> Option<&Nested> {
        match self {
            MoreDrawer::Panel { nested, .. } | MoreDrawer::ClosedFrom { nested, .. } => nested.as_ref(),
            MoreDrawer::Closed | MoreDrawer::Menu => None,
        }
    }

    pub fn step(self, event: MoreEvent) -> MoreDrawer {
        match (self, event) {
            (_, MoreEvent::Open) => MoreDrawer::Menu,
            (MoreDrawer::Menu, MoreEvent::Drill { view }) => MoreDrawer::Panel { view, nested: None },
            (state, MoreEvent::Drill { .. }) => state,
            (MoreDrawer::Panel { view, .. }, MoreEvent::Nest { view: from, nested }) if view == from => {
                MoreDrawer::Panel { view, nested: Some(nested) }
            }
            (state, MoreEvent::Nest { .. }) => state,
            (MoreDrawer::Panel { view, .. }, MoreEvent::Back) => MoreDrawer::Panel { view, nested: None },
            (state, MoreEvent::Back) => state,
            (MoreDrawer::Panel { view, nested } | MoreDrawer::ClosedFrom { view, nested }, MoreEvent::Close) => {
                MoreDrawer::ClosedFrom { view, nested }
            }
            (_, MoreEvent::Close) => MoreDrawer::Closed,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Presence

/// A drawer on screen: gone, sliding in or resting, or sliding out until `due_at`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Presence {
    #[default]
    Gone,
    Shown,
    Leaving { due_at: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PresenceEvent {
    /// The drawer's machine says it is open (`true`) or shut, at `now`.
    Open { open: bool, now: f64 },
    Tick { now: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PresenceEffect {
    WakeAt(f64),
    /// The slide out ended: the content is no longer present.
    Gone,
}

impl Presence {
    pub fn step(self, event: PresenceEvent) -> (Presence, Option<PresenceEffect>) {
        use Presence as P;
        match (self, event) {
            (_, PresenceEvent::Open { open: true, .. }) => (P::Shown, None),
            (P::Shown, PresenceEvent::Open { open: false, now }) => {
                let due_at = now + SLIDE_OUT;
                (P::Leaving { due_at }, Some(PresenceEffect::WakeAt(due_at)))
            }
            (P::Leaving { due_at }, PresenceEvent::Tick { now }) if now + 1e-9 >= due_at => (P::Gone, Some(PresenceEffect::Gone)),
            (state, _) => (state, None),
        }
    }

    /// Whether the drawer is on screen at all.
    pub fn mounted(self) -> bool {
        self != Presence::Gone
    }
}

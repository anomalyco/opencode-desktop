//! Object-safe host boundary for the narrow-window (<768 px) shell feature.

use rune::{El, PointerPress, Tx};
use opencode_ui::palette::Pal;

use crate::Narrow;
use crate::session::NarrowView;

/// Window chrome metrics consumed by the narrow switcher, location bar, and drawers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NarrowChrome {
    /// One device pixel in shell points (`chrome().hairline`).
    pub hairline: f32,
    /// Settings › Bottom navigation keeps the titlebar and view switcher along the foot.
    pub bottom: bool,
}

impl Default for NarrowChrome {
    fn default() -> Self {
        Self { hairline: 1.0, bottom: false }
    }
}

pub use opencode_runtime::TabStatus;

/// Status badge for a tab shown in the narrow titlebar trigger and tabs drawer.
pub type NarrowTabStatus = opencode_runtime::TabStatus;

/// Pure-data summary of one tab for the narrow titlebar trigger and tabs drawer.
#[derive(Clone, Debug)]
pub struct NarrowTab {
    pub uid: String,
    pub title: String,
    pub is_draft: bool,
    pub status: Option<NarrowTabStatus>,
}

/// Object-safe host boundary for the narrow-window layout, view switcher, tabs drawer, and More drawer.
///
/// Every state machine, event handler, and poll/key/settle entrypoint in `opencode-feature-narrow`
/// operates on `&dyn NarrowHost` / `&mut dyn NarrowHost`. Generic `H: NarrowHost + 'static` bounds
/// appear only at `rune::El` view/callback boundaries, delegating 1-line to non-generic handlers.
pub trait NarrowHost: opencode_kernel::AppEnv {
    /// Narrow layout state (tracked read).
    fn narrow_state(&self) -> &Narrow;
    /// Narrow layout state (tracked write — callers only invoke when mutating).
    fn narrow_state_mut(&mut self) -> &mut Narrow;

    /// Whether a Storybook stage is currently mounted over the window.
    fn stage_active(&self) -> bool {
        false
    }
    /// Whether the chosen tab layout has a left sidebar when the window is wide.
    fn layout_has_sidebar(&self) -> bool {
        false
    }
    /// Whether the Settings screen currently covers the active route.
    fn settings_open(&self) -> bool {
        false
    }
    /// Whether the Home route is currently shown (and Settings is not open).
    fn is_home_route(&self) -> bool {
        false
    }

    /// Window chrome insets/metrics needed by the narrow switcher, location bar, and drawers.
    fn chrome(&self) -> NarrowChrome {
        NarrowChrome::default()
    }

    /// Currently active tab UID, if any.
    fn current_tab_uid(&self) -> Option<String> {
        None
    }
    /// Total number of open tabs in the window.
    fn tab_count(&self) -> usize {
        0
    }
    /// Pure-data descriptor of the single tab with `uid`, if present.
    fn narrow_tab(&self, uid: &str) -> Option<NarrowTab> {
        self.narrow_tabs().into_iter().find(|t| t.uid == uid)
    }
    /// Pure-data descriptors of every open tab, in strip order.
    fn narrow_tabs(&self) -> Vec<NarrowTab> {
        Vec::new()
    }
    /// Renders the 16 pt project avatar badge for non-draft tab `uid`.
    fn tab_avatar(&self, p: &Pal, uid: &str) -> El {
        let _ = (p, uid);
        rune::el()
    }

    /// Every plugin-contributed narrow view offered for the routed session (`PanelSpec.mobile`).
    fn narrow_views(&self) -> Vec<NarrowView> {
        Vec::new()
    }
    /// Renders the session location trigger for the narrow location bar, or `None` if session `session_id` is absent.
    fn location_trigger(&self, p: &Pal, session_id: &str) -> Option<El> {
        let _ = (p, session_id);
        None
    }
    /// Renders narrow panel view `view` for `session_id` at `width` (`mobile: true`).
    /// `view_title` is `Some` when `view` matches an entry from [`Self::narrow_views`].
    fn render_narrow_panel(&self, p: &Pal, session_id: &str, view: &str, view_title: Option<&str>, width: f32) -> El {
        let _ = (p, session_id, view, view_title, width);
        rune::el()
    }

    /// Prepares session/panel state when entering narrow `view` for `session_id`
    /// (for example, hydrating or creating the first terminal tab for `"terminal:main"`).
    fn on_enter_narrow_view(&mut self, session_id: &str, view: &str, tx: &mut Tx) {
        let _ = (session_id, view, tx);
    }
    /// Synchronizes narrow diff line-wrapping preference with active review sessions during poll.
    fn sync_narrow_review_wrap(&mut self) {}

    /// Opens the Settings screen (after the tabs drawer finishes sliding out).
    fn open_settings(&mut self) {}
    /// Starts a new session (from the tabs drawer).
    fn new_session(&mut self, tx: &mut Tx) {
        let _ = tx;
    }
    /// Navigates to Home (from the tabs drawer).
    fn toggle_home(&mut self, tx: &mut Tx) {
        let _ = tx;
    }
    /// Selects tab `uid` (from the tabs drawer).
    fn select_tab(&mut self, uid: &str, tx: &mut Tx) {
        let _ = (uid, tx);
    }
    /// Closes tab `uid` (from the tabs drawer's close button or middle-click).
    fn close_tab(&mut self, uid: &str, tx: &mut Tx) {
        let _ = (uid, tx);
    }
    /// Opens the context menu for tab `uid` on secondary press.
    fn open_tab_menu(&mut self, uid: &str, press: PointerPress, tx: &mut Tx) {
        let _ = (uid, press, tx);
    }
}

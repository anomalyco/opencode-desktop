//! How the session row splits between the chat panel and the side region
//! (`session/session-panel-width.ts`, `session/screen-layout.ts`). The side region has no width of
//! its own: the chat panel keeps one (600 until the divider is dragged, then the session's own) and
//! the side region takes the rest of the row, so it grows with the window rather than the chat.

use crate::App;

/// `DEFAULT_SESSION_WIDTH` (`shell/state/layout.tsx`).
pub const SESSION_DEFAULT_W: f32 = 600.0;
/// `SESSION_PANEL_WIDTH_MIN`: room for the session header's title beside its controls.
pub const SESSION_MIN_W: f32 = 400.0;
/// `SIDE_PANE_WIDTH_MIN`: how narrow a drag takes the side region.
pub const SIDE_PANE_MIN_W: f32 = 320.0;
/// `SESSION_PANEL_WIDTH_FIT`: the chat panel's floor until the divider is dragged.
const SESSION_FIT_W: f32 = 450.0;
/// `REVIEW_PANE_WIDTH_FIT` and `_SPLIT`: what the side region keeps until the divider is
/// dragged, wider while a panel shows split diffs.
const REVIEW_FIT_W: f32 = 480.0;
const REVIEW_FIT_SPLIT_W: f32 = 800.0;
/// The row's inline insets (`px-[var(--shell-inline-inset,8px)]`) and the gap between its panes.
const ROW_INSET: f32 = 8.0;
const ROW_GAP: f32 = 8.0;

/// `sessionPanelWidthMax`: the widest the chat panel gets in `available` (the row less its gap).
pub fn session_width_max(available: f32, split: bool, resized: bool) -> f32 {
    if resized {
        return SESSION_MIN_W.max(available - SIDE_PANE_MIN_W);
    }
    let pane = if split { REVIEW_FIT_SPLIT_W } else { REVIEW_FIT_W };
    SESSION_FIT_W.max(available - pane)
}

/// `clampSessionPanelWidth`: a stored width, raised to the minimum and kept below the maximum.
pub fn clamp_session_width(width: f32, available: f32, split: bool, resized: bool) -> f32 {
    width.max(SESSION_MIN_W).min(session_width_max(available, split, resized))
}

impl App {
    /// The session row's width less the gap between its panes (`available` in `screen-layout.ts`),
    /// for a window `zoomed_w` wide.
    pub fn session_row_available(&self, zoomed_w: f32) -> f32 {
        (zoomed_w - self.sidebar_shown_w() - ROW_INSET * 2.0 - ROW_GAP).max(0.0)
    }

    /// The width the routed session stores for its chat panel, and whether the user dragged it.
    fn stored_session_width(&self) -> (f32, bool) {
        let stored = self.active_session_id().and_then(|id| self.side.sessions.get(id)).and_then(|st| st.session_width);
        (stored.unwrap_or(SESSION_DEFAULT_W), stored.is_some())
    }

    /// Whether a side panel asks for the wider minimum (`region.wide`): any of them, shown or not,
    /// as the review does while its diffs are split.
    pub fn side_wide(&self) -> bool {
        self.all_panels().iter().any(|p| (p.wide)(self))
    }

    /// The chat panel's width beside an open side region (`resizedWidth`).
    pub fn chat_width(&self, zoomed_w: f32) -> f32 {
        let (width, resized) = self.stored_session_width();
        clamp_session_width(width, self.session_row_available(zoomed_w), self.side_wide(), resized)
    }

    /// The side region's open width: what the chat panel leaves of the row.
    pub fn side_width(&self, zoomed_w: f32) -> f32 {
        (self.session_row_available(zoomed_w) - self.chat_width(zoomed_w)).max(0.0)
    }

    /// The widest a drag takes the chat panel (`panel.max`).
    pub fn chat_width_max(&self, zoomed_w: f32) -> f32 {
        session_width_max(self.session_row_available(zoomed_w), self.side_wide(), true)
    }

    fn routed_side(&self) -> Option<&super::SessionSideState> {
        self.active_session_id().and_then(|id| self.side.sessions.get(id))
    }

    /// The side pane's share of the side region's height while a pane splits below it.
    pub fn split_ratio(&self) -> f32 {
        self.routed_side().and_then(|st| st.split_ratio).unwrap_or(super::state::SPLIT_DEFAULT_RATIO)
    }

    /// The bottom pane's height.
    pub fn bottom_height(&self) -> f32 {
        self.routed_side().and_then(|st| st.bottom_height).unwrap_or(super::state::BOTTOM_DEFAULT_H)
    }
}

//! How the session row splits between the chat panel and the side region
//! (`session/session-panel-width.ts`, `session/screen-layout.ts`). The side region has no width of
//! its own: the chat panel keeps one (600 until the divider is dragged, then the session's own) and
//! the side region takes the rest of the row, so it grows with the window rather than the chat.

use crate::host::PanelsHost;
use crate::state::{BOTTOM_DEFAULT_H, SPLIT_DEFAULT_RATIO, SessionSideState, all_panels};

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

/// The session row's width less the gap between its panes (`available` in `screen-layout.ts`),
/// for a window `zoomed_w` wide.
pub fn session_row_available(host: &dyn PanelsHost, zoomed_w: f32) -> f32 {
    (zoomed_w - host.sidebar_shown_w() - ROW_INSET * 2.0 - ROW_GAP).max(0.0)
}

/// The width the routed session stores for its chat panel, and whether the user dragged it.
fn stored_session_width(host: &dyn PanelsHost) -> (f32, bool) {
    let stored = routed_side(host).and_then(|st| st.session_width);
    (stored.unwrap_or(SESSION_DEFAULT_W), stored.is_some())
}

/// Whether a side panel asks for the wider minimum (`region.wide`): any of them, shown or not,
/// as the review does while its diffs are split.
pub fn side_wide(host: &dyn PanelsHost) -> bool {
    all_panels(host).iter().any(|p| p.is_wide(host))
}

/// The chat panel's width beside an open side region (`resizedWidth`).
pub fn chat_width(host: &dyn PanelsHost, zoomed_w: f32) -> f32 {
    let (width, resized) = stored_session_width(host);
    clamp_session_width(width, session_row_available(host, zoomed_w), side_wide(host), resized)
}

/// The side region's open width: what the chat panel leaves of the row.
pub fn side_width(host: &dyn PanelsHost, zoomed_w: f32) -> f32 {
    (session_row_available(host, zoomed_w) - chat_width(host, zoomed_w)).max(0.0)
}

/// The widest a drag takes the chat panel (`panel.max`).
pub fn chat_width_max(host: &dyn PanelsHost, zoomed_w: f32) -> f32 {
    session_width_max(session_row_available(host, zoomed_w), side_wide(host), true)
}

fn routed_side(host: &dyn PanelsHost) -> Option<&SessionSideState> {
    host.panel_session_id().and_then(|id| host.side().sessions.get(id))
}

/// The side pane's share of the side region's height while a pane splits below it.
pub fn split_ratio(host: &dyn PanelsHost) -> f32 {
    routed_side(host).and_then(|st| st.split_ratio).unwrap_or(SPLIT_DEFAULT_RATIO)
}

/// The bottom pane's height.
pub fn bottom_height(host: &dyn PanelsHost) -> f32 {
    routed_side(host).and_then(|st| st.bottom_height).unwrap_or(BOTTOM_DEFAULT_H)
}

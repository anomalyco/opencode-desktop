//! Whether a held drag selecting the terminal's text scrolls it, and which way. Plain `std`
//! only, so a table program outside the repo can include this file by path.

/// How near the text's top or bottom edge, in points, a drag starts scrolling
/// (ghostty-web `selection-manager.ts:68`, `AUTO_SCROLL_EDGE_SIZE`).
pub const EDGE: f32 = 30.0;
/// Lines each step scrolls (`selection-manager.ts:99`, `AUTO_SCROLL_SPEED`).
pub const LINES: isize = 3;
/// Seconds between steps (`selection-manager.ts:100`, `AUTO_SCROLL_INTERVAL`).
pub const STEP_SECS: f64 = 0.05;

/// -1 to scroll up into the scrollback, 1 down to newer lines, 0 not at all, for a pointer `y`
/// points below the text's top (negative above it) over text `height` tall: within [`EDGE`] of
/// an edge or past it, the top first, as `updateAutoScroll` does (`selection-manager.ts:767-779`).
pub fn edge_dir(y: f32, height: f32) -> i8 {
    if y < EDGE {
        return -1;
    }
    if y > height - EDGE {
        return 1;
    }
    0
}

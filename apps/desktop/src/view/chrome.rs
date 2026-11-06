//! Where the window's own controls sit over the shell (`Cx::chrome_insets`). On macOS the
//! traffic lights share the sidebar's header line. On Windows, as Desktop does there
//! (`titlebar.tsx`, `shell.tsx`), a titlebar row runs across the top with the sidebar toggle
//! at its start and Rune's caption buttons at its end, and the shell sits 1pt below it.

use std::cell::Cell;

use rune::{ChromeInsets, Color};

/// The titlebar the window declares (`App::titlebar`): Desktop's 44pt on Windows
/// (`defaults.ts`'s `titlebarHeight`), the session header's 48pt line elsewhere.
pub(crate) const TITLEBAR_H: f32 = if cfg!(target_os = "windows") { 44.0 } else { 48.0 };

/// Whether the shell keeps a titlebar row of its own above the sidebar and panels, rather than
/// sharing the session header's line with the window's controls.
const TITLEBAR_ROW: bool = cfg!(target_os = "windows");

/// The shell's inset above the sidebar and panels (`--shell-top-inset`).
const TOP_INSET: f32 = 8.0;
/// Below a titlebar row, Desktop keeps only the panels' outline clear.
const TOP_INSET_UNDER_ROW: f32 = 1.0;

/// The window's chrome as this frame's view sees it: plain data read once per frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct WindowChrome {
    /// Where the traffic lights end, from the window's leading edge; 0 without them (full
    /// screen, Windows, Linux).
    pub lights_end: f32,
    /// The titlebar row's height; 0 where the shell has none.
    pub row_h: f32,
}

thread_local! {
    static CHROME: Cell<WindowChrome> = const { Cell::new(WindowChrome { lights_end: 0.0, row_h: 0.0 }) };
}

/// Notes the window's chrome for this frame's view (`view::app`).
pub(crate) fn set_chrome(insets: ChromeInsets) {
    let row_h = if TITLEBAR_ROW { insets.height } else { 0.0 };
    CHROME.with(|c| c.set(WindowChrome { lights_end: insets.leading, row_h }));
}

/// The window's chrome for the view being built.
pub(crate) fn chrome() -> WindowChrome {
    CHROME.with(Cell::get)
}

impl WindowChrome {
    /// Where the sidebar and panels start: under the titlebar row, or the shell's inset.
    pub fn top(&self) -> f32 {
        if self.row_h > 0.0 { self.row_h + TOP_INSET_UNDER_ROW } else { TOP_INSET }
    }

    /// Whether the sidebar opens with its own header line (traffic lights, the toggle's slot).
    pub fn sidebar_header(&self) -> bool {
        self.row_h == 0.0
    }

    /// The room a header at the panel's top-left keeps for the collapsed sidebar toggle,
    /// counted from the shell inset plus the header's padding (18): past the traffic lights,
    /// or just the toggle and its gap (32). None while the toggle stays in the titlebar row.
    pub fn collapsed_slot(&self) -> f32 {
        if !self.sidebar_header() {
            return 0.0;
        }
        if self.lights_end > 0.0 { self.lights_end + 16.0 - 18.0 + 32.0 } else { 32.0 }
    }

    /// What a header's `toggle-space` takes: the collapsed slot less the toggle's 4pt gap
    /// while the sidebar is closed.
    pub fn toggle_space(&self, sidebar_open: bool) -> f32 {
        let slot = self.collapsed_slot();
        if sidebar_open || slot == 0.0 { 0.0 } else { slot - 4.0 }
    }

    /// The sidebar toggle's resting spot, top-left in window points, with the sidebar open
    /// `w` wide or closed. In the titlebar row it stays put, at Desktop's start padding (9).
    pub fn toggle_at(&self, open: bool, w: f32) -> (f32, f32) {
        if !self.sidebar_header() {
            return (9.0, ((self.row_h - 28.0) / 2.0).round());
        }
        let x = if open { w - 2.0 - 28.0 } else { 18.0 + self.collapsed_slot() - 4.0 - 28.0 };
        (x, 18.0)
    }
}

/// The caption buttons' symbols: white over a dark titlebar, black over a light one, as
/// Electron's `symbolColor` follows Desktop's tone. It's judged from the theme's own
/// background, so every theme reads in both modes: whichever of the two contrasts more
/// (relative luminance 0.179 is where they tie).
pub(crate) fn caption_color(bg: Color) -> Color {
    if bg.luminance() < 0.179 { Color::WHITE } else { Color::BLACK }
}

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

/// Desktop's app menu sits at the titlebar row's start, its toggle after it (`titlebar.tsx`):
/// the row's 9pt start padding (`--sidebar-inset-start` less 2), the menu's 36pt slot with
/// its 28pt button 4pt in, and the row's 6pt gap.
const ROW_PAD: f32 = 9.0;
const MENU_SLOT: f32 = 36.0;
const ROW_GAP: f32 = 6.0;

/// The window's chrome as this frame's view sees it: plain data read once per frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct WindowChrome {
    /// Where the traffic lights end, from the window's leading edge; 0 without them (full
    /// screen, Windows, Linux).
    pub lights_end: f32,
    /// The titlebar row's height; 0 where the shell has none.
    pub row_h: f32,
    /// What the caption buttons take of the row's trailing end; 0 without them (full screen,
    /// macOS, Linux).
    pub caption_w: f32,
    /// One device pixel in shell points, as Chromium draws a 1px CSS rule at this scale and zoom.
    pub hairline: f32,
}

thread_local! {
    static CHROME: Cell<WindowChrome> = const { Cell::new(WindowChrome { lights_end: 0.0, row_h: 0.0, caption_w: 0.0, hairline: 1.0 }) };
}

/// The titlebar's height in window points at the app's `zoom`, for `Cx::titlebar_height`.
/// With a titlebar row, Desktop's 44 grows with a zoom in and holds zoomed out, as its caption
/// buttons do (`titleBarOverlay`); without one, the session header's line.
pub(crate) fn titlebar_height(zoom: f32) -> f32 {
    if TITLEBAR_ROW { TITLEBAR_H * zoom.max(1.0) } else { TITLEBAR_H }
}

/// Notes the window's chrome for this frame's view (`view::app`), drawn at the app's `zoom`.
/// The insets are window points; the shell is laid out at 1/zoom of them. The row is the
/// titlebar's height (`titlebar_height`), so it lines up with the caption buttons at every
/// zoom: 44pt zoomed out (`44 / min(zoom, 1)` in the shell, as Desktop's), and 44 shell points
/// zoomed in.
pub(crate) fn set_chrome(insets: ChromeInsets, zoom: f32, scale: f32) {
    let row_h = if TITLEBAR_ROW && insets.height > 0.0 { titlebar_height(zoom) / zoom } else { 0.0 };
    let px = scale * zoom;
    let hairline = px.floor().max(1.0) / px;
    CHROME.with(|c| c.set(WindowChrome { lights_end: insets.leading, row_h, caption_w: insets.trailing / zoom, hairline }));
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
    /// `w` wide or closed. In the titlebar row it stays put, after the app menu.
    pub fn toggle_at(&self, open: bool, w: f32) -> (f32, f32) {
        if !self.sidebar_header() {
            return (ROW_PAD + MENU_SLOT + ROW_GAP, self.row_y());
        }
        let x = if open { w - 2.0 - 28.0 } else { 18.0 + self.collapsed_slot() - 4.0 - 28.0 };
        (x, 18.0)
    }

    /// The app menu's button, top-left in window points, in the titlebar row; None without one.
    pub fn menu_at(&self) -> Option<(f32, f32)> {
        (!self.sidebar_header()).then(|| (ROW_PAD + (MENU_SLOT - 28.0) / 2.0, self.row_y()))
    }

    /// Where a 28pt control sits in the titlebar row, centred.
    fn row_y(&self) -> f32 {
        ((self.row_h - 28.0) / 2.0).round()
    }
}

/// The caption buttons' symbols: white over a dark titlebar, black over a light one, as
/// Electron's `symbolColor` follows Desktop's tone. It's judged from the theme's own
/// background, so every theme reads in both modes: whichever of the two contrasts more
/// (relative luminance 0.179 is where they tie).
pub(crate) fn caption_color(bg: Color) -> Color {
    if bg.luminance() < 0.179 { Color::WHITE } else { Color::BLACK }
}

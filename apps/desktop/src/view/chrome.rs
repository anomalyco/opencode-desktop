//! Where the window's own controls sit over the shell (`Cx::chrome_insets`), per tab layout
//! (`titlebar.tsx`, `shell.tsx`). Top tabs keep a titlebar row everywhere: Desktop's 44pt on
//! Windows with the caption buttons at its end, 36pt elsewhere with the traffic lights at its
//! start. Side tabs and the inbox keep the row on Windows only, with the sidebar toggle in it;
//! elsewhere the traffic lights share the sidebar's header line. Below a Windows row the shell
//! sits 1pt down, elsewhere 8pt.

use std::cell::Cell;

use rune::{ChromeInsets, Color, Titlebar};

use crate::layout_state::TabLayout;

/// The titlebar the window declares at launch (`App::titlebar`): Desktop's 44pt on Windows
/// (`defaults.ts`'s `titlebarHeight`), the session header's 48pt line elsewhere.
pub(crate) const TITLEBAR_H: f32 = if cfg!(target_os = "windows") { 44.0 } else { 48.0 };

/// Windows keeps a titlebar row of its own above the sidebar and panels in every layout.
const WINDOWS: bool = cfg!(target_os = "windows");

/// Top tabs' row off Windows (`titlebarHeight`): 36pt, its 28pt controls 8pt down.
const TOP_ROW_H: f32 = 36.0;
const TOP_ROW_PAD: f32 = 8.0;

/// The shell's inset above the sidebar and panels (`--shell-top-inset`).
const TOP_INSET: f32 = 8.0;
/// Below a Windows row, Desktop keeps only the panels' outline clear.
const TOP_INSET_UNDER_ROW: f32 = 1.0;

/// The window's chrome as this frame's view sees it: plain data read once per frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WindowChrome {
    /// Where the traffic lights end, from the window's leading edge; 0 without them (full
    /// screen, Windows, Linux).
    pub lights_end: f32,
    /// The titlebar row's height; 0 where the shell has none.
    pub row_h: f32,
    /// Points above the row's controls: 8 in a 36pt row, 0 in Windows' 44pt one.
    pub row_pad: f32,
    /// What the caption buttons take of the row's trailing end; 0 without them (full screen,
    /// macOS, Linux).
    pub caption_w: f32,
    /// One device pixel in shell points, as Chromium draws a 1px CSS rule at this scale and zoom.
    pub hairline: f32,
    /// The layout the shell shows: top tabs' while the window is narrow.
    pub layout: TabLayout,
    /// The window is below Desktop's 768 px breakpoint (`narrow`).
    pub narrow: bool,
    /// A narrow window keeps its titlebar row along its foot (Settings › Bottom navigation).
    pub bottom: bool,
}

thread_local! {
    static CHROME: Cell<WindowChrome> = const {
        Cell::new(WindowChrome { lights_end: 0.0, row_h: 0.0, row_pad: 0.0, caption_w: 0.0, hairline: 1.0, layout: TabLayout::Horizontal, narrow: false, bottom: false })
    };
}

/// The titlebar's height in window points at the app's `zoom`, for `Cx::titlebar_height`.
/// Desktop's 44 grows with a zoom in and holds zoomed out, as its caption buttons do
/// (`titleBarOverlay`).
pub(crate) fn titlebar_height(zoom: f32) -> f32 {
    TITLEBAR_H * zoom.max(1.0)
}

/// The titlebar macOS declares for `layout` at `zoom` (`titlebar.tsx`): top tabs' 36pt band
/// with the traffic lights at its start, 14pt in and centred on its controls; at the window's
/// top edge, `(14, 14)`, while a narrow window keeps the row along its foot (`bottom`); or the
/// sidebar header's 48pt line, the buttons centred 27pt in and 32pt down at every zoom, as the
/// sidebar's icons are (`macVerticalTrafficLights`).
pub(crate) fn mac_titlebar(layout: TabLayout, zoom: f32, bottom: bool) -> Titlebar {
    match layout {
        TabLayout::Horizontal if bottom => Titlebar::new((TOP_ROW_H * zoom).round()).traffic_lights(14.0, 14.0),
        TabLayout::Horizontal => Titlebar::new((TOP_ROW_H * zoom).round()).traffic_lights(14.0, (22.0 * zoom - 7.0).round()),
        TabLayout::Vertical | TabLayout::Inbox => Titlebar::new(TITLEBAR_H).traffic_lights((27.0 * zoom - 7.0).round(), (32.0 * zoom - 7.0).round()),
    }
}

/// Notes the window's chrome for this frame's view (`view::app`), drawn at the app's `zoom`.
/// The insets and `scale` are the view's (`Cx::chrome_insets`, `Cx::env`), at the zoom. On Windows the row
/// is the titlebar's height (`titlebar_height`), so it lines up with the caption buttons at
/// every zoom: 44pt zoomed out (`44 / min(zoom, 1)` in the shell, as Desktop's), and 44 shell
/// points zoomed in.
pub(crate) fn set_chrome(insets: ChromeInsets, zoom: f32, scale: f32, layout: TabLayout, narrow: bool, bottom: bool) {
    let (row_h, row_pad) = match () {
        // Full screen hides the caption buttons, not the row.
        _ if WINDOWS => (titlebar_height(zoom) / zoom, 0.0),
        _ if layout == TabLayout::Horizontal => (TOP_ROW_H, TOP_ROW_PAD),
        _ => (0.0, 0.0),
    };
    let hairline = scale.floor().max(1.0) / scale;
    let chrome = WindowChrome { lights_end: insets.leading, row_h, row_pad, caption_w: insets.trailing, hairline, layout, narrow, bottom: narrow && bottom };
    CHROME.with(|c| c.set(chrome));
}

/// The window's chrome for the view being built.
pub(crate) fn chrome() -> WindowChrome {
    CHROME.with(Cell::get)
}

impl WindowChrome {
    /// Where the sidebar and panels start: under the titlebar row, or the shell's inset.
    pub fn top(&self) -> f32 {
        match () {
            // A titlebar along the foot leaves the panels' top edge to the window's (`shell.tsx:221`).
            _ if self.bottom => 0.0,
            _ if self.row_h > 0.0 && self.row_pad == 0.0 => self.row_h + TOP_INSET_UNDER_ROW,
            _ => self.row_h + TOP_INSET,
        }
    }

    /// Whether the sidebar opens with its own header line (traffic lights, the toggle's slot).
    /// Room the shell keeps along its foot for a titlebar there.
    pub fn foot(&self) -> f32 {
        if self.bottom { self.row_h } else { 0.0 }
    }

    pub fn sidebar_header(&self) -> bool {
        self.row_h == 0.0
    }

    /// The room a header at the panel's top-left keeps for the collapsed sidebar toggle,
    /// counted from the shell inset plus the header's padding (18): past the traffic lights and
    /// their 10pt gap (`macTrafficLightsGap`), or just the toggle and its gap (32). None while
    /// the toggle stays in the titlebar row, or top tabs have no sidebar.
    pub fn collapsed_slot(&self) -> f32 {
        if !self.sidebar_header() {
            return 0.0;
        }
        if self.lights_end > 0.0 { self.lights_end + 10.0 - 18.0 + 32.0 } else { 32.0 }
    }

    /// What a header's `toggle-space` takes: the collapsed slot less the toggle's 4pt gap
    /// while the sidebar is closed.
    pub fn toggle_space(&self, sidebar_open: bool) -> f32 {
        let slot = self.collapsed_slot();
        if sidebar_open || slot == 0.0 { 0.0 } else { slot - 4.0 }
    }

    /// The header line's sidebar toggle's resting spot, top-left in window points, with the
    /// sidebar open `w` wide or closed.
    pub fn toggle_at(&self, open: bool, w: f32) -> (f32, f32) {
        let x = if open { w - super::INSET_END - 28.0 } else { 18.0 + self.collapsed_slot() - 4.0 - 28.0 };
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

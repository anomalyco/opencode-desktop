//! Where a window is, and where it opens again (Desktop's `window-state.ts`): one machine per
//! window, stepped each time the app reads the window's placement (`App::poll_window`, at close
//! and at quit), whose effect is the frame to save. The frame saved is what restoring returns to
//! (the window's own frame while restored, the system's record while maximized, the frame from
//! before full screen while full screen), how it was shown, and the display it was on; `fit`
//! turns it into where a launch opens it on the displays there are then.

use rune::{Display, WindowPlacement};

use super::store::Frame;

/// Desktop's window size, in points, when none was saved.
const DEFAULT_SIZE: (f32, f32) = (1280.0, 800.0);

/// A frame a window restores to: its top-left and its content size, in desktop pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Normal {
    pub position: (i32, i32),
    pub size: (u32, u32),
}

/// How a window is shown, beside the frame it restores to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Restored,
    Maximized,
    /// Full screen; leaving it returns to maximized or restored.
    FullScreen { maximized: bool },
}

/// Where a window is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Opened as `mode` at `normal` (what it was saved with; none for a new window), not read yet.
    Opening { mode: Mode, normal: Option<Normal> },
    /// Read: shown as `mode` on `display`, restoring to `normal`.
    Placed { mode: Mode, normal: Normal, display: Option<[i32; 4]> },
}

impl Default for Placement {
    fn default() -> Self {
        Placement::Opening { mode: Mode::Restored, normal: None }
    }
}

impl Placement {
    /// A window opening at `frame` (`fit`'s), or where the system puts a new one.
    pub fn opening(frame: Option<&Frame>) -> Placement {
        let Some(f) = frame else { return Placement::default() };
        let mode = match (f.full_screen, f.maximized) {
            (true, maximized) => Mode::FullScreen { maximized },
            (false, true) => Mode::Maximized,
            (false, false) => Mode::Restored,
        };
        let normal = f.x.zip(f.y).map(|position| Normal { position, size: (f.width, f.height) });
        Placement::Opening { mode, normal }
    }

    /// The window was read at `p`. A minimized window changes nothing: it opens again as it
    /// was shown before. Returns the frame to save, unless it's minimized.
    pub fn step(self, p: WindowPlacement) -> (Placement, Option<Frame>) {
        if p.minimized {
            return (self, None);
        }
        let (was, before) = match self {
            Placement::Opening { mode, normal } => (mode, normal),
            Placement::Placed { mode, normal, .. } => (mode, Some(normal)),
        };
        let current = Normal { position: p.position, size: p.content_size };
        let restore = p.restore.map(|(position, size)| Normal { position, size });
        let (mode, normal) = match (p.full_screen, p.maximized) {
            // Full screen hides the frame it came from: keep the one from before it.
            (true, _) => (Mode::FullScreen { maximized: was_maximized(was) }, before.or(restore).unwrap_or(current)),
            // The system's record follows the window across displays; ours only as a fallback.
            (false, true) => (Mode::Maximized, restore.or(before).unwrap_or(current)),
            // Restored, or snapped: where it is on screen, as Desktop saves `getBounds()`.
            (false, false) => (Mode::Restored, current),
        };
        let display = p.display.map(|((x, y), (w, h))| [x, y, w as i32, h as i32]);
        let next = Placement::Placed { mode, normal, display };
        (next, Some(next.frame()))
    }

    fn frame(self) -> Frame {
        let Placement::Placed { mode, normal, display } = self else { return Frame::default() };
        Frame {
            x: Some(normal.position.0),
            y: Some(normal.position.1),
            width: normal.size.0,
            height: normal.size.1,
            maximized: was_maximized(mode),
            full_screen: matches!(mode, Mode::FullScreen { .. }),
            display,
        }
    }
}

fn was_maximized(mode: Mode) -> bool {
    matches!(mode, Mode::Maximized | Mode::FullScreen { maximized: true })
}

/// Where a window saved at `f` opens on `displays` (Desktop's `resolveWindowState`):
/// - a frame without a place opens where the system puts it, at its size;
/// - a maximized or full screen window whose display is still there opens on it, even when its
///   restored frame was left on another display (it was moved there maximized), its frame
///   centred there and no larger than it;
/// - a frame that still fits on a display opens there;
/// - otherwise (its display is gone, or the layout changed), the default size centred on the
///   primary display, keeping maximized and full screen.
pub fn fit(f: Frame, displays: &[Display]) -> Option<Frame> {
    let (Some(fx), Some(fy)) = (f.x, f.y) else { return Some(f) };
    let bounds = |d: &Display| [d.position.0, d.position.1, d.size.0 as i32, d.size.1 as i32];
    let within = |b: [i32; 4], slack: i32| {
        fx >= b[0] - slack && fy >= b[1] - slack && fx + f.width as i32 <= b[0] + b[2] + slack && fy + f.height as i32 <= b[1] + b[3] + slack
    };
    let target = (f.maximized || f.full_screen).then(|| f.display.and_then(|saved| displays.iter().find(|d| bounds(d) == saved))).flatten();
    if let Some(target) = target.filter(|d| !within(bounds(d), 0)) {
        let [x, y, w, h] = bounds(target);
        let (width, height) = (f.width.min(w as u32), f.height.min(h as u32));
        return Some(Frame { x: Some(x + (w - width as i32) / 2), y: Some(y + (h - height as i32) / 2), width, height, ..f });
    }
    // A frame may hang a little off its display (the shadow's width, a window dragged to an edge).
    if displays.iter().any(|d| within(bounds(d), 16)) {
        return Some(f);
    }
    let primary = displays.iter().find(|d| d.primary).or(displays.first())?;
    let (w, h) = default_size(primary);
    let x = primary.position.0 + (primary.size.0 as i32 - w as i32).max(0) / 2;
    let y = primary.position.1 + (primary.size.1 as i32 - h as i32).max(0) / 2;
    Some(Frame { x: Some(x), y: Some(y), width: w, height: h, display: Some(bounds(primary)), ..f })
}

/// Where a window with nothing saved opens, as Desktop's first window does (a `BrowserWindow`
/// 1280 × 800 with no position): centred across the primary display, at its top on macOS (AppKit
/// keeps it under the menu bar), centred down it elsewhere.
pub fn first(displays: &[Display]) -> Option<Frame> {
    let primary = displays.iter().find(|d| d.primary).or(displays.first())?;
    let (w, h) = default_size(primary);
    let x = primary.position.0 + (primary.size.0 as i32 - w as i32).max(0) / 2;
    let y = if cfg!(target_os = "macos") { primary.position.1 } else { primary.position.1 + (primary.size.1 as i32 - h as i32).max(0) / 2 };
    let display = [primary.position.0, primary.position.1, primary.size.0 as i32, primary.size.1 as i32];
    Some(Frame { x: Some(x), y: Some(y), width: w, height: h, display: Some(display), ..Frame::default() })
}

/// [`DEFAULT_SIZE`] in `d`'s pixels, no larger than `d`.
fn default_size(d: &Display) -> (u32, u32) {
    let px = |v: f32| (f64::from(v) * d.scale) as u32;
    (px(DEFAULT_SIZE.0).min(d.size.0), px(DEFAULT_SIZE.1).min(d.size.1))
}

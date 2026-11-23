//! Pinch to zoom (Settings › Preferences, Desktop's `pinchZoom` and `window/zoom-machine.ts`).
//! With it on, a trackpad pinch or Ctrl + mouse wheel zooms the window: a gesture's deltas
//! gather until they pass 20 px, then the zoom follows from where it stood by 0.2 for each
//! 20 px, between 20% and 1000%, and 160 ms without a delta ends the gesture. A pinch zooms the
//! window without saving the level; the zoom keys and Appearance › Zoom step on from wherever it
//! left the zoom, and turning the setting off puts the saved level back. With it off, a pinch or
//! Ctrl + wheel does nothing, as Desktop's window takes back Chromium's zoom.

use rune::{MagnifyEvent, Tx};

use crate::App;

/// Wheel pixels a gesture gathers before it zooms, and per step of [`STEP`].
const THRESHOLD: f32 = 20.0;
const STEP: f32 = 0.2;
/// A gesture ends this long after its last delta.
const END_S: f64 = 0.16;
pub const MIN: f32 = 0.2;
pub const MAX: f32 = 10.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Gesture {
    #[default]
    Idle,
    /// Deltas gathering until they pass the threshold.
    Gathering { total: f32 },
    /// Zooming from `start` by what has come since the threshold.
    Pinching { start: f32, total: f32 },
}

#[derive(Debug, Default)]
pub struct Pinch {
    gesture: Gesture,
    /// The clock at the last delta.
    last: f64,
    /// The saved level a pinch moved the window off, until a level is set again.
    level: Option<f32>,
}

impl Pinch {
    /// A wheel delta in Chromium's pixels (positive zooms out) at clock `now`, with the window
    /// at `zoom`: the zoom it moves the window to, if any (`zoom-machine.ts` `wheel`).
    pub fn wheel(&mut self, zoom: f32, delta: f32, now: f64) -> Option<f32> {
        if now - self.last > END_S {
            self.gesture = Gesture::Idle;
        }
        self.last = now;
        match self.gesture {
            Gesture::Idle => {
                self.gesture = if delta.abs() < THRESHOLD { Gesture::Gathering { total: delta } } else { Gesture::Pinching { start: zoom, total: 0.0 } };
                None
            }
            _ if delta == 0.0 => None,
            Gesture::Gathering { total } => {
                let total = total + delta;
                // The gesture zooms from the zoom at the moment it crossed the threshold.
                self.gesture = if total.abs() < THRESHOLD { Gesture::Gathering { total } } else { Gesture::Pinching { start: zoom, total: 0.0 } };
                None
            }
            Gesture::Pinching { start, total } => {
                let total = total + delta;
                self.gesture = Gesture::Pinching { start, total };
                let target = (start - total / THRESHOLD * STEP).clamp(MIN, MAX);
                (target != zoom).then_some(target)
            }
        }
    }

    /// A level is set (the zoom keys, Appearance › Zoom): the window is on its saved zoom again.
    pub fn leveled(&mut self) {
        self.gesture = Gesture::Idle;
        self.level = None;
    }
}

/// A pinch as the wheel pixels Desktop's window sees for it, which Chromium divides by the
/// page's zoom. Ctrl + wheel (and so a pinch off macOS) reaches Rune as `dy / 200` a point,
/// and Rune's wheel points are Chromium's wheel pixels on every platform (100 a notch on
/// Windows, rune#856). macOS reports a pinch itself, which Chromium turns into a Ctrl + wheel
/// of `-100 ln(1 + delta)` (measured on a Mac: Desktop's pinch zooms less the further in it is).
fn wheel_pixels(ev: &MagnifyEvent, zoom: f32) -> f32 {
    let pixels = if cfg!(target_os = "macos") && !ev.wheel { -100.0 * (1.0 + ev.delta).max(0.01).ln() } else { -ev.delta * 200.0 };
    pixels / zoom
}

impl App {
    /// A pinch or Ctrl + wheel over the window that nothing inside took (the open diagram and
    /// the browser's page take theirs first). Always taken, so it never scrolls.
    pub fn pinch(&mut self, ev: MagnifyEvent, tx: &mut Tx) -> bool {
        if !self.settings.pinch_zoom {
            return true;
        }
        let delta = wheel_pixels(&ev, *self.zoom);
        if let Some(zoom) = self.gestures.zoom_pinch.wheel(*self.zoom, delta, *self.clock) {
            tx.instant();
            self.gestures.zoom_pinch.level.get_or_insert(*self.zoom);
            *self.zoom = zoom;
        }
        true
    }

    /// Settings › Preferences › Pinch to zoom; turning it off puts the saved zoom back.
    pub fn set_pinch_zoom(&mut self, on: bool, tx: &mut Tx) {
        tx.instant();
        self.change_settings(|s| s.pinch_zoom = on);
        if let Some(level) = self.gestures.zoom_pinch.level.filter(|_| !on) {
            *self.zoom = level;
        }
        if !on {
            self.gestures.zoom_pinch.leveled();
        }
    }
}

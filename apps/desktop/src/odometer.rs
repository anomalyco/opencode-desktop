//! Live numbers that roll like an odometer, as SwiftUI's `.contentTransition(.numericText(value:))`
//! does in the iOS app: only the digits that change roll, up while the number rises and down when
//! it falls, through a soft edge; digits are tabular, and the width eases when it changes.
//!
//! Every ticking number in the app goes through [`odometer`]: subagent run times, the header's
//! context and cost, the subagents capsule, the queue's count, the question dock's "N of M" and a
//! tool group's count. Each is keyed by what it counts (a call, a session, a form), so a number
//! that changes because another session opened mounts fresh, at once, instead of rolling.

use std::cell::OnceCell;

use rune::{Color, El, Spring, TextStyle};
use rune_kit::{Kit, RollingText};

/// The roll and the width's ease: quick, with no overshoot, about SwiftUI's
/// `.snappy(duration: 0.25)` without its bounce.
pub const SPRING: Spring = Spring::new(0.3, 0.0);

thread_local! {
    static KIT: OnceCell<Kit> = const { OnceCell::new() };
}

/// `text`, showing the number `value`, keyed `key` (unique in the window and stable between
/// builds), in `font` with tabular digits. It widens to the right.
pub fn odometer(key: impl Into<String>, text: impl Into<String>, value: f64, font: TextStyle, color: Color) -> El {
    build(RollingText::new(key, text), value, font, color)
}

/// [`odometer`] for a number set against its right edge, so it widens to the left: new digits
/// roll in where they rest instead of sliding in with it.
pub fn odometer_end(key: impl Into<String>, text: impl Into<String>, value: f64, font: TextStyle, color: Color) -> El {
    build(RollingText::new(key, text).trailing(), value, font, color)
}

fn build(label: RollingText, value: f64, font: TextStyle, color: Color) -> El {
    KIT.with(|k| {
        let k = k.get_or_init(|| Kit::neutral(true));
        label.value(value).font(font.features("\"tnum\"")).color(color).spring(SPRING).build(k)
    })
}

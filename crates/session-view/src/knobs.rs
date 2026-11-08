//! Timeline and Working motion knobs and spring/curve helpers.

use rune::{Curve, Easing, Spring};
use rune_knobs::{Group, Knob, Knobs};

use crate::line_reveal::{LINE_GROW, NEW_LINE_REVEAL, SOFT_EDGE};

/// Its corner radius in points: 12 is Desktop's; 14 makes a full pill.
pub const CAPSULE_RADIUS: Knob = Knob::new("Capsule radius", 0.0, 14.0, 8.0).step(1.0).about("Corner radius of the subagent capsule");

/// Where "Jump to latest" sits: at the composer's right end, or centred above it as on Desktop.
pub const JUMP_PLACE: Knob = Knob::choice("Jump to latest", &["Right", "Centre"], 0)
    .about("Where Jump to latest sits over composer")
    .hints(&["At the composer's right end", "Centred above the composer"]);
/// How far above the composer, and above the pills over it, the timeline fades out into the
/// composer's top edge (p248).
pub const COMPOSER_FADE: Knob = Knob::new("Composer fade", 0.0, 48.0, 16.0).step(1.0).about("Timeline fade reach above composer and pills");

/// A message from another client glows as it lands: a quick rise, then a fast decay with a
/// slower tail (half-lives in seconds), faded to nothing between "Glow fades at" and its length.
pub const GLOW_LENGTH: Knob = Knob::new("Glow length", 0.3, 2.0, 0.9).about("Total seconds an arriving message stays lit");
pub const GLOW_RISE: Knob = Knob::new("Glow rise", 0.0, 0.3, 0.07).about("Seconds an arriving message takes to brighten");
pub const GLOW_DECAY: Knob = Knob::new("Glow decay", 0.02, 0.5, 0.11).about("Fast half-life of the arrival lift");
pub const GLOW_TAIL: Knob = Knob::new("Glow tail", 0.02, 0.8, 0.2).about("Slow tail half-life of arrival lift");
pub const GLOW_FADE: Knob = Knob::new("Glow fades at", 0.1, 1.9, 0.7).about("When the arrival lift finishes fading out");
/// Rows growing in and finished content gliding open, as a spring's duration.
pub const ROW_GROWTH: Knob = Knob::new("Row growth", 0.1, 0.8, 0.32).about("Spring duration for rows growing in");
/// The tail under a turn, where Working hands over to the turn's actions, as a spring.
pub const ACTIONS_TAIL: Knob = Knob::new("Actions tail", 0.1, 0.8, 0.34).about("Spring duration for the turn tail");
/// Jump to latest showing and hiding (tweens, seconds) and how far it rises as it shows.
pub const JUMP_IN: Knob = Knob::new("Jump in", 0.05, 0.5, 0.15).about("Seconds Jump to latest takes to show");
pub const JUMP_OUT: Knob = Knob::new("Jump out", 0.05, 0.5, 0.11).about("Seconds Jump to latest takes to hide");
pub const JUMP_RISE: Knob = Knob::new("Jump rise", 0.0, 12.0, 4.0).step(1.0).about("Points Jump to latest rises on enter");
/// An image flying out of its thumbnail into the viewer and back, as a spring.
pub const IMAGE_FLIGHT: Knob = Knob::new("Image flight", 0.1, 0.8, 0.25).about("Spring duration for opening image viewer");

/// Desktop's Sources experiment (`sources/experiment.ts`, Knobs › Experiments › Sources): a
/// "N sources" chip under a reply that searched the web, opening the turn's searches in a tab.
pub const SOURCES_ENABLED: Knob = Knob::choice("Sources", &["Off", "On"], 0).about("Pages a turn searched and read")
    .hints(&["Hide sources", "Show pages the turn consulted"]);
/// A site's own icon is fetched from that site, which tells it the page was looked at; letters
/// stay on this device.
pub const SOURCES_ICONS: Knob = Knob::choice("Site icons", &["Letters", "Site icon"], 0).about("How a source's site shows")
    .hints(&["Show initials without contacting the site", "Fetch the icon from the site"]);
pub const EXPERIMENTS: Group = Group::new("Experiments", "sources-chip", &[SOURCES_ENABLED, SOURCES_ICONS]);

pub const TIMELINE: &[Knob] = &[
    GLOW_LENGTH, GLOW_RISE, GLOW_DECAY, GLOW_TAIL, GLOW_FADE, ROW_GROWTH, NEW_LINE_REVEAL,
    LINE_GROW, SOFT_EDGE, ACTIONS_TAIL, JUMP_IN, JUMP_OUT, JUMP_RISE, IMAGE_FLIGHT,
];

pub const TIMELINE_GROUP: Group = Group::new("Timeline", "timeline", TIMELINE);

/// The Working row's sweep (`working.rs`): a wide soft band at a steady speed.
/// Seconds per pass in "Fixed period".
pub const WORKING_PERIOD: Knob = Knob::new("Period", 0.4, 4.0, 1.2).step(0.05).about("Seconds per sweep in fixed-period mode");
/// One pass per period whatever the label's width, or a steady speed.
pub const WORKING_PACE: Knob = Knob::choice("Pace", &["Fixed period", "pt/s"], 1)
    .about("Whether Working sweeps by period or speed")
    .hints(&["Same pass time for any width", "Constant speed in points per second"]);
pub const WORKING_SPEED: Knob = Knob::new("Speed (pt/s)", 20.0, 400.0, 90.0).step(5.0).about("Sweep speed in points per second");
/// The band's width, in points or as a percentage of the label's.
pub const WORKING_BAND: Knob = Knob::new("Band width", 8.0, 240.0, 140.0).step(1.0).about("Width of the moving highlight band");
pub const WORKING_UNIT: Knob = Knob::choice("Band unit", &["pt", "% of label"], 0)
    .about("Units for the Working highlight band width")
    .hints(&["Fixed width in window points", "Percentage of the label width"]);
/// How far the highlight goes from the weak colour to the strong one.
pub const WORKING_INTENSITY: Knob = Knob::new("Intensity", 0.0, 1.0, 0.85).about("Peak brightness of the Working sweep");
/// How much of the band's half-width is a soft edge.
pub const WORKING_SOFTNESS: Knob = Knob::new("Softness", 0.0, 1.0, 1.0).about("Softness of the sweep band's edges");
pub const WORKING_DIRECTION: Knob = Knob::choice("Direction", &["Left to right", "Right to left"], 0)
    .about("Direction the Working highlight sweeps across")
    .hints(&["Sweeps from left to right", "Sweeps from right to left"]);
/// Seconds of rest between passes.
pub const WORKING_PAUSE: Knob = Knob::new("Pause", 0.0, 3.0, 0.0).step(0.05).about("Seconds of rest between Working sweeps");

pub const WORKING: Group = Group::new(
    "Working",
    "working",
    &[WORKING_PACE, WORKING_SPEED, WORKING_PERIOD, WORKING_BAND, WORKING_UNIT, WORKING_INTENSITY, WORKING_SOFTNESS, WORKING_DIRECTION, WORKING_PAUSE],
);

/// A knob in seconds as an `f64`, to the microsecond: the decimal it was written as, not its
/// nearest `f32`.
pub fn secs(k: &Knobs, knob: &Knob) -> f64 {
    (f64::from(k.get(knob)) * 1e6).round() / 1e6
}

/// The arrival glow `t` seconds in (the iOS app's message reveal, shortened).
pub fn glow(k: &Knobs, t: f64) -> f32 {
    let (rise, fade, end) = (secs(k, &GLOW_RISE), secs(k, &GLOW_FADE), secs(k, &GLOW_LENGTH));
    let g = if t < rise {
        1.0 - (1.0 - t.max(0.0) / rise).powi(3)
    } else {
        let d = t - rise;
        0.78 * 0.5f64.powf(d / secs(k, &GLOW_DECAY)) + 0.22 * 0.5f64.powf(d / secs(k, &GLOW_TAIL))
    };
    let out = ((t - fade) / (end - fade).max(1e-3)).clamp(0.0, 1.0);
    (g * (1.0 - out * out * (3.0 - 2.0 * out))) as f32
}

/// How long a glow lasts.
pub fn glow_secs(k: &Knobs) -> f64 {
    secs(k, &GLOW_LENGTH)
}

pub fn growth(k: &Knobs) -> Spring {
    Spring::new(k.get(&ROW_GROWTH), 0.0)
}

pub fn actions_tail(k: &Knobs) -> Spring {
    Spring::new(k.get(&ACTIONS_TAIL), 0.0)
}

/// Jump to latest's fade and rise: a quick ease out as it shows, a quicker ease in as it hides.
pub fn jump(k: &Knobs, shown: bool) -> Curve {
    if shown {
        Curve::tween(k.get(&JUMP_IN), Easing::cubic(0.2, 0.8, 0.2, 1.0))
    } else {
        Curve::tween(k.get(&JUMP_OUT), Easing::cubic(0.4, 0.0, 1.0, 1.0))
    }
}

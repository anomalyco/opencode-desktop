//! Live look-and-feel sliders: ⌘⇧K, pick a region, drag a knob, and Copy puts the values on the
//! clipboard to paste back to an agent.

use rune::{Anim, Curve, El, Easing, KeyEvent, Spring, Tx};
use rune_knobs::{Group, Knob, Knobs, Preset};

use crate::App;

/// The subagent capsule's fill: 1 is Desktop's (the composer's surface), 0 a bare hairline.
pub const CAPSULE_FILL: Knob = Knob::new("Capsule fill", 0.0, 1.0, 1.0);
/// Its corner radius in points: 12 is Desktop's; 14 makes a full pill.
pub const CAPSULE_RADIUS: Knob = Knob::new("Capsule radius", 0.0, 14.0, 12.0).step(1.0);
/// Its hairline: 1 is Desktop's raised ring (the composer's), 0 the faint muted border.
pub const CAPSULE_RING: Knob = Knob::new("Capsule ring", 0.0, 1.0, 1.0);
/// Where "Jump to latest" sits: at the composer's right end, or centred above it as on Desktop.
pub const JUMP_PLACE: Knob = Knob::choice("Jump to latest", &["Right", "Centre"], 0);

/// Project icon marks (`project_icon::Look`); `--icon-style <name>` sets the style at launch.
pub const ICON_STYLE: Knob = Knob::choice("Icon style", &project_icon::Style::NAMES, 1);
pub const ICON_SATURATION: Knob = Knob::new("Icon saturation", 0.0, 1.6, 1.0);
pub const ICON_GRAIN: Knob = Knob::new("Icon grain", 0.0, 0.4, 0.12);

/// The composer ⇄ question morph's curve: a spring (duration and bounce) or an eased tween
/// over the duration.
pub const MORPH_CURVE: Knob = Knob::choice("Morph curve", &["Spring", "Ease out", "In-out", "Expo out"], 0);
/// The morph's duration in seconds: a spring's perceptual duration, a tween's length.
pub const MORPH_DURATION: Knob = Knob::new("Morph duration", 0.1, 1.0, 0.4).step(0.01);
/// The spring's bounce; tweens ignore it.
pub const MORPH_BOUNCE: Knob = Knob::new("Morph bounce", 0.0, 0.4, 0.0).step(0.01);
/// The arriving face's fade and rise, as a spring's duration.
pub const FACE_IN: Knob = Knob::new("Face in", 0.1, 1.0, 0.4).step(0.01);
/// How far the arriving face rises, in points.
pub const FACE_RISE: Knob = Knob::new("Face rise", 0.0, 16.0, 6.0).step(1.0);
/// The leaving face's fade, as a spring's duration.
pub const FADE_OUT: Knob = Knob::new("Fade out", 0.05, 0.5, 0.15).step(0.01);
/// A new question's text and answers sliding in, as a spring's duration.
pub const ARRIVE: Knob = Knob::new("Arrive", 0.1, 0.8, 0.3).step(0.01);
/// How far they slide, in points.
pub const ARRIVE_SLIDE: Knob = Knob::new("Arrive slide", 0.0, 32.0, 12.0).step(1.0);

/// The timeline's motion. The defaults are the curves it shipped with, and stay those exactly
/// (`timeline_defaults_are_what_shipped`).
///
/// A message from another client glows as it lands: a quick rise, then a fast decay with a
/// slower tail (half-lives in seconds), faded to nothing between "Glow fades at" and its length.
pub const GLOW_LENGTH: Knob = Knob::new("Glow length", 0.3, 2.0, 0.9);
pub const GLOW_RISE: Knob = Knob::new("Glow rise", 0.0, 0.3, 0.07);
pub const GLOW_DECAY: Knob = Knob::new("Glow decay", 0.02, 0.5, 0.11);
pub const GLOW_TAIL: Knob = Knob::new("Glow tail", 0.02, 0.8, 0.2);
pub const GLOW_FADE: Knob = Knob::new("Glow fades at", 0.1, 1.9, 0.7);
/// Rows growing in and finished content gliding open, as a spring's duration.
pub const ROW_GROWTH: Knob = Knob::new("Row growth", 0.1, 0.8, 0.32);
/// The tail under a turn, where Working hands over to the turn's actions, as a spring.
pub const ACTIONS_TAIL: Knob = Knob::new("Actions tail", 0.1, 0.8, 0.34);
/// Jump to latest showing and hiding (tweens, seconds) and how far it rises as it shows.
pub const JUMP_IN: Knob = Knob::new("Jump in", 0.05, 0.5, 0.15);
pub const JUMP_OUT: Knob = Knob::new("Jump out", 0.05, 0.5, 0.11);
pub const JUMP_RISE: Knob = Knob::new("Jump rise", 0.0, 12.0, 4.0).step(1.0);
/// An image flying out of its thumbnail into the viewer and back, as a spring.
pub const IMAGE_FLIGHT: Knob = Knob::new("Image flight", 0.1, 0.8, 0.25);

pub const TIMELINE: &[Knob] = &[
    GLOW_LENGTH, GLOW_RISE, GLOW_DECAY, GLOW_TAIL, GLOW_FADE, ROW_GROWTH, ACTIONS_TAIL, JUMP_IN, JUMP_OUT, JUMP_RISE,
    IMAGE_FLIGHT,
];

/// The defaults are Desktop's (`docks.morph`); the presets are the curves worth comparing.
const QUESTION_PRESETS: &[Preset] = &[
    Preset { name: "iOS", values: &[("Morph duration", 0.5), ("Face in", 0.5)] },
    Preset { name: "Bounce", values: &[("Morph duration", 0.42), ("Morph bounce", 0.12), ("Face in", 0.42)] },
    Preset { name: "Ease out", values: &[("Morph curve", 1.0), ("Morph duration", 0.3), ("Face in", 0.3)] },
    Preset { name: "In-out", values: &[("Morph curve", 2.0), ("Morph duration", 0.4)] },
    Preset { name: "Expo out", values: &[("Morph curve", 3.0), ("Morph duration", 0.45), ("Face in", 0.35), ("Fade out", 0.1)] },
];

pub const QUESTION_DOCK: Group =
    Group::new("Question dock", "question", &[MORPH_CURVE, MORPH_DURATION, MORPH_BOUNCE, FACE_IN, FACE_RISE, FADE_OUT, ARRIVE, ARRIVE_SLIDE])
        .presets(QUESTION_PRESETS)
        .default_label("Desktop");

/// The Working row's sweep (`working.rs`): a wide soft band at a steady speed.
/// Seconds per pass in "Fixed period".
pub const WORKING_PERIOD: Knob = Knob::new("Period", 0.4, 4.0, 1.2).step(0.05);
/// One pass per period whatever the label's width, or a steady speed.
pub const WORKING_PACE: Knob = Knob::choice("Pace", &["Fixed period", "pt/s"], 1);
pub const WORKING_SPEED: Knob = Knob::new("Speed (pt/s)", 20.0, 400.0, 90.0).step(5.0);
/// The band's width, in points or as a percentage of the label's.
pub const WORKING_BAND: Knob = Knob::new("Band width", 8.0, 240.0, 140.0).step(1.0);
pub const WORKING_UNIT: Knob = Knob::choice("Band unit", &["pt", "% of label"], 0);
/// How far the highlight goes from the weak colour to the strong one.
pub const WORKING_INTENSITY: Knob = Knob::new("Intensity", 0.0, 1.0, 0.85);
/// How much of the band's half-width is a soft edge.
pub const WORKING_SOFTNESS: Knob = Knob::new("Softness", 0.0, 1.0, 1.0);
pub const WORKING_DIRECTION: Knob = Knob::choice("Direction", &["Left to right", "Right to left"], 0);
/// Seconds of rest between passes.
pub const WORKING_PAUSE: Knob = Knob::new("Pause", 0.0, 3.0, 0.0).step(0.05);

pub const WORKING: Group = Group::new(
    "Working",
    "working",
    &[WORKING_PACE, WORKING_SPEED, WORKING_PERIOD, WORKING_BAND, WORKING_UNIT, WORKING_INTENSITY, WORKING_SOFTNESS, WORKING_DIRECTION, WORKING_PAUSE],
);

pub const GROUPS: &[Group] = &[
    Group::new("Composer", "composer", &[CAPSULE_FILL, CAPSULE_RADIUS, CAPSULE_RING, JUMP_PLACE]),
    Group::new("Icons", "sidebar", &[ICON_STYLE, ICON_SATURATION, ICON_GRAIN]),
    Group::new("Icon playground", "icon-playground", &[ICON_STYLE, ICON_SATURATION, ICON_GRAIN]),
    QUESTION_DOCK,
    Group::new("Timeline", "timeline", TIMELINE),
    WORKING,
];

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

/// The morph as the knobs have it: ease-out cubic, ease-in-out cubic and expo-out are the CSS
/// curves of those names.
pub fn morph(s: &App) -> Anim {
    let k = &s.knobs;
    let d = k.get(&MORPH_DURATION);
    let ease = |x1, y1, x2, y2| Anim::new(Curve::tween(d, Easing::cubic(x1, y1, x2, y2)));
    match k.get(&MORPH_CURVE) as usize {
        1 => ease(0.33, 1.0, 0.68, 1.0),
        2 => ease(0.65, 0.0, 0.35, 1.0),
        3 => ease(0.16, 1.0, 0.3, 1.0),
        _ => Spring::new(d, k.get(&MORPH_BOUNCE)).into(),
    }
}

/// A knob in seconds as a zero-bounce spring.
pub fn spring(s: &App, k: &Knob) -> Spring {
    Spring::new(s.knobs.get(k), 0.0)
}

/// Applies a Question dock preset by name (`--preset` on the command line); "Desktop" is
/// the defaults.
pub fn apply_preset(s: &mut App, name: &str) {
    let g = GROUPS.iter().find(|g| g.name == "Question dock").expect("Question dock knobs");
    let name = if name.eq_ignore_ascii_case("desktop") { "Default" } else { name };
    s.knobs.apply(g, name);
}

/// ⌘⇧K toggles the layer; Esc closes it.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.mods.alt || ev.mods.ctrl || !rune_knobs::key(&mut s.knobs, ev) {
        return false;
    }
    tx.instant();
    true
}

pub fn layer(s: &App) -> El {
    rune_knobs::layer(&s.knobs, GROUPS, rune_knobs::Style::dark(), |s: &mut App| &mut s.knobs)
}

#[cfg(test)]
mod tests {
    use rune::{Curve, Easing, Spring};
    use rune_knobs::Knobs;

    use super::*;

    /// The arrival glow as it shipped, before its numbers moved onto knobs.
    fn shipped_glow(t: f64) -> f32 {
        const GLOW_SECS: f64 = 0.9;
        const RISE: f64 = 0.07;
        let g = if t < RISE {
            1.0 - (1.0 - t.max(0.0) / RISE).powi(3)
        } else {
            let d = t - RISE;
            0.78 * 0.5f64.powf(d / 0.11) + 0.22 * 0.5f64.powf(d / 0.2)
        };
        let end = ((t - 0.7) / (GLOW_SECS - 0.7)).clamp(0.0, 1.0);
        (g * (1.0 - end * end * (3.0 - 2.0 * end))) as f32
    }

    #[test]
    fn timeline_defaults_are_what_shipped() {
        let k = Knobs::default();
        for i in 0..=150 {
            let t = i as f64 / 120.0 - 0.05;
            assert_eq!(glow(&k, t), shipped_glow(t), "glow at {t}s");
        }
        assert_eq!(glow_secs(&k), 0.9);
        assert_eq!(growth(&k), rune::GROWTH);
        assert_eq!(actions_tail(&k), Spring::new(0.34, 0.0));
        assert_eq!(jump(&k, true), Curve::tween(0.15, Easing::cubic(0.2, 0.8, 0.2, 1.0)));
        assert_eq!(jump(&k, false), Curve::tween(0.11, Easing::cubic(0.4, 0.0, 1.0, 1.0)));
        assert_eq!(k.get(&JUMP_RISE), 4.0);
        assert_eq!(Spring::new(k.get(&IMAGE_FLIGHT), 0.0), Spring::new(0.25, 0.0));
    }
}

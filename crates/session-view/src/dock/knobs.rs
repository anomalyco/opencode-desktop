//! The composer ⇄ dock morph's knobs (⌘⇧K → Question dock), whose defaults are Desktop's
//! `docks.morph`.

use rune::{Anim, Curve, Easing, Spring};
use rune_knobs::{Group, Knob, Knobs, Preset};

/// The composer ⇄ question morph's curve: a spring (duration and bounce) or an eased tween
/// over the duration.
pub const MORPH_CURVE: Knob = Knob::choice("Morph curve", &["Spring", "Ease out", "In-out", "Expo out"], 0)
    .about("Curve for the composer question morph")
    .hints(&["Zero-bounce or bouncy spring", "Cubic ease-out curve", "Cubic ease-in-out curve", "Exponential ease-out curve"]);
/// The morph's duration in seconds: a spring's perceptual duration, a tween's length.
pub const MORPH_DURATION: Knob = Knob::new("Morph duration", 0.1, 1.0, 0.4).step(0.01).about("Seconds the composer question morph takes");
/// The spring's bounce; tweens ignore it.
pub const MORPH_BOUNCE: Knob = Knob::new("Morph bounce", 0.0, 0.4, 0.0).step(0.01).about("Spring bounce on the question morph");
/// The arriving face's fade and rise, as a spring's duration.
pub const FACE_IN: Knob = Knob::new("Face in", 0.1, 1.0, 0.4).step(0.01).about("Seconds the arriving dock face fades in");
/// How far the arriving face rises, in points.
pub const FACE_RISE: Knob = Knob::new("Face rise", 0.0, 16.0, 6.0).step(1.0).about("Points the arriving dock face rises");
/// The leaving face's fade, as a spring's duration.
pub const FADE_OUT: Knob = Knob::new("Fade out", 0.05, 0.5, 0.15).step(0.01).about("Seconds the leaving dock face fades out");
/// A new question's text and answers sliding in, as a spring's duration.
pub const ARRIVE: Knob = Knob::new("Arrive", 0.1, 0.8, 0.3).step(0.01).about("Seconds a new question's options slide in");
/// How far they slide, in points.
pub const ARRIVE_SLIDE: Knob = Knob::new("Arrive slide", 0.0, 32.0, 12.0).step(1.0).about("Points a new question's options slide");
/// The tallest a long question gets, as a share of the pane; its answers scroll inside (p273).
pub const QUESTION_MAX: Knob = Knob::new("Max height", 0.3, 1.0, 0.6).step(0.05).about("Tallest the dock gets, share of pane");

/// The defaults are Desktop's (`docks.morph`); the presets are the curves worth comparing.
const QUESTION_PRESETS: &[Preset] = &[
    Preset { name: "iOS", values: &[("Morph duration", 0.5), ("Face in", 0.5)] },
    Preset { name: "Bounce", values: &[("Morph duration", 0.42), ("Morph bounce", 0.12), ("Face in", 0.42)] },
    Preset { name: "Ease out", values: &[("Morph curve", 1.0), ("Morph duration", 0.3), ("Face in", 0.3)] },
    Preset { name: "In-out", values: &[("Morph curve", 2.0), ("Morph duration", 0.4)] },
    Preset { name: "Expo out", values: &[("Morph curve", 3.0), ("Morph duration", 0.45), ("Face in", 0.35), ("Fade out", 0.1)] },
];

pub const QUESTION_DOCK: Group =
    Group::new("Question dock", "question", &[MORPH_CURVE, MORPH_DURATION, MORPH_BOUNCE, FACE_IN, FACE_RISE, FADE_OUT, ARRIVE, ARRIVE_SLIDE, QUESTION_MAX])
        .presets(QUESTION_PRESETS)
        .default_label("Desktop");

/// The morph as the knobs have it: ease-out cubic, ease-in-out cubic and expo-out are the CSS
/// curves of those names.
pub fn morph(k: &Knobs) -> Anim {
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
pub fn spring(k: &Knobs, knob: &Knob) -> Spring {
    Spring::new(k.get(knob), 0.0)
}

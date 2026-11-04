//! Live look-and-feel sliders: ⌘⇧K, pick a region, drag a knob, and Copy puts the values on the
//! clipboard to paste back to an agent.

use rune::{Anim, Curve, El, Easing, KeyEvent, Spring, Tx};
use rune_knobs::{Group, Knob, Preset};

use crate::App;
use crate::line_reveal::{LINE_GROW, NEW_LINE_REVEAL, SOFT_EDGE};

#[allow(unused_imports)]
pub use session_view::knobs::*;

/// The subagent capsule's fill: 1 is Desktop's (the composer's surface), 0 a bare hairline.
pub const CAPSULE_FILL: Knob = Knob::new("Capsule fill", 0.0, 1.0, 1.0).about("How solid the subagent capsule fill is");
/// Its corner radius in points: 12 is Desktop's; 14 makes a full pill.
pub const CAPSULE_RADIUS: Knob = Knob::new("Capsule radius", 0.0, 14.0, 12.0).step(1.0).about("Corner radius of the subagent capsule");
/// Its hairline: 1 is Desktop's raised ring (the composer's), 0 the faint muted border.
pub const CAPSULE_RING: Knob = Knob::new("Capsule ring", 0.0, 1.0, 1.0).about("Strength of the subagent capsule hairline");

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

pub const TIMELINE_WITH_REVEAL: &[Knob] = &[
    GLOW_LENGTH, GLOW_RISE, GLOW_DECAY, GLOW_TAIL, GLOW_FADE, ROW_GROWTH, NEW_LINE_REVEAL,
    LINE_GROW, SOFT_EDGE, ACTIONS_TAIL, JUMP_IN, JUMP_OUT, JUMP_RISE, IMAGE_FLIGHT,
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
    Group::new("Question dock", "question", &[MORPH_CURVE, MORPH_DURATION, MORPH_BOUNCE, FACE_IN, FACE_RISE, FADE_OUT, ARRIVE, ARRIVE_SLIDE, QUESTION_MAX])
        .presets(QUESTION_PRESETS)
        .default_label("Desktop");

rune_register::slice!(pub GROUP_LIST: Group);

rune_register::register!(GROUP_LIST: Group = [
    Group::new("Composer", "composer", &[CAPSULE_FILL, CAPSULE_RADIUS, CAPSULE_RING, JUMP_PLACE, COMPOSER_FADE]),
    Group::new("Timeline", "timeline", TIMELINE_WITH_REVEAL),
    crate::disclosure::GROUP,
    WORKING,
    crate::compaction::GROUP,
    crate::copied::GROUP,
]);

/// The panel's order, which module paths don't give. A group not named here goes after these.
const ORDER: &[&str] = &[
    "Composer", "Question dock", "Timeline", "Scroll bar", "Tool groups", "Working", "Terminal",
    "Header", "Dividers", "Copy", "Popover", "Stream", "Todo pill",
];

/// The knob groups the panel shows, in order. Each is registered from the module it tunes.
pub fn groups() -> &'static [Group] {
    static ALL: std::sync::OnceLock<Vec<Group>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| crate::stories::ordered(rune_register::collect(&GROUP_LIST), ORDER, |g| g.name))
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
    let g = groups().iter().find(|g| g.name == "Question dock").expect("Question dock knobs");
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
    rune_knobs::layer(&s.knobs, groups(), rune_knobs::Style::dark(), |s: &mut App| &mut s.knobs)
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
        assert_eq!(crate::line_reveal::line_grow(&k), Spring::new(0.08, 0.0));
        assert_eq!(crate::line_reveal::reveal(&k), crate::line_reveal::Reveal::SoftEdge);
        assert_eq!(jump(&k, true), Curve::tween(0.15, Easing::cubic(0.2, 0.8, 0.2, 1.0)));
        assert_eq!(jump(&k, false), Curve::tween(0.11, Easing::cubic(0.4, 0.0, 1.0, 1.0)));
        assert_eq!(k.get(&JUMP_RISE), 4.0);
        assert_eq!(Spring::new(k.get(&IMAGE_FLIGHT), 0.0), Spring::new(0.25, 0.0));
    }

    #[test]
    fn every_knob_says_what_it_does() {
        for g in groups() {
            for k in g.knobs {
                let words = k.about.split_whitespace().count();
                assert!((1..=8).contains(&words), "{} › {}: `.about` needs 1..=8 words, got {words} ({:?})", g.name, k.name, k.about);
                if !k.labels.is_empty() {
                    assert_eq!(k.hints.len(), k.labels.len(), "{} › {}: one hint per choice option", g.name, k.name);
                    for (opt, hint) in k.labels.iter().zip(k.hints) {
                        let hw = hint.split_whitespace().count();
                        assert!((1..=8).contains(&hw), "{} › {} ({opt}): hint needs 1..=8 words, got {hw} ({hint:?})", g.name, k.name);
                    }
                }
            }
        }
    }
}

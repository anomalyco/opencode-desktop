//! Live look-and-feel sliders: ⌘⇧K, pick a region, drag a knob, and Copy puts the values on the
//! clipboard to paste back to an agent.

use rune::{Anim, Curve, El, Easing, KeyEvent, Spring, Tx};
use rune_knobs::{Group, Knob, Preset};

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

/// The defaults are Desktop's (`docks.morph`); the presets are the curves worth comparing.
const QUESTION_PRESETS: &[Preset] = &[
    Preset { name: "iOS", values: &[("Morph duration", 0.5), ("Face in", 0.5)] },
    Preset { name: "Bounce", values: &[("Morph duration", 0.42), ("Morph bounce", 0.12), ("Face in", 0.42)] },
    Preset { name: "Ease out", values: &[("Morph curve", 1.0), ("Morph duration", 0.3), ("Face in", 0.3)] },
    Preset { name: "In-out", values: &[("Morph curve", 2.0), ("Morph duration", 0.4)] },
    Preset { name: "Expo out", values: &[("Morph curve", 3.0), ("Morph duration", 0.45), ("Face in", 0.35), ("Fade out", 0.1)] },
];

pub const GROUPS: &[Group] = &[
    Group::new("Composer", "composer", &[CAPSULE_FILL, CAPSULE_RADIUS, CAPSULE_RING, JUMP_PLACE]),
    Group::new("Icons", "sidebar", &[ICON_STYLE, ICON_SATURATION, ICON_GRAIN]),
    Group::new("Icon playground", "icon-playground", &[ICON_STYLE, ICON_SATURATION, ICON_GRAIN]),
    Group::new("Question dock", "question", &[MORPH_CURVE, MORPH_DURATION, MORPH_BOUNCE, FACE_IN, FACE_RISE, FADE_OUT, ARRIVE, ARRIVE_SLIDE])
        .presets(QUESTION_PRESETS)
        .default_label("Desktop"),
];

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

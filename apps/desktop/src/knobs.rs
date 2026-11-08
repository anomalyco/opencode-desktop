//! Live look-and-feel sliders: ⌘⇧K, pick a region, drag a knob, and Copy puts the values on the
//! clipboard to paste back to an agent.

use rune::{El, KeyEvent, Tx};
use rune_knobs::{Group, Knob};

use crate::App;

#[allow(unused_imports)]
pub use session_view::dock::knobs::{ARRIVE, ARRIVE_SLIDE, FACE_IN, FACE_RISE, FADE_OUT, MORPH_BOUNCE, MORPH_CURVE, MORPH_DURATION, QUESTION_DOCK, QUESTION_MAX};
#[allow(unused_imports)]
pub use session_view::knobs::*;

/// The subagent capsule's fill: 1 is Desktop's (the composer's surface), 0 a bare hairline.
pub const CAPSULE_FILL: Knob = Knob::new("Capsule fill", 0.0, 1.0, 1.0).about("How solid the subagent capsule fill is");
/// Its hairline: 1 is Desktop's raised ring (the composer's), 0 the faint muted border.
pub const CAPSULE_RING: Knob = Knob::new("Capsule ring", 0.0, 1.0, 1.0).about("Strength of the subagent capsule hairline");

/// The window's animation clock (Desktop's `motionGlobals.speed`): below 1 every animation runs
/// slower. The debug bar's SPEED cell steps it through 0.5x and 0.25x.
pub const SPEED: Knob = Knob::new("Speed", 0.25, 4.0, 1.0).step(0.05).about("Global animation speed");
/// Every animation at a quarter of its speed, on top of Speed (`motionGlobals.slow`).
pub const SLOW: Knob = Knob::choice("Slow motion (0.25x)", &["Off", "On"], 0).about("Run every animation at a quarter speed")
    .hints(&["Use the selected speed", "Slow the selected speed fourfold"]);

/// What the window's animation clock runs at, from Speed and Slow motion.
pub fn time_scale(s: &App) -> f32 {
    s.knobs.get(&SPEED) * if s.knobs.get(&SLOW) > 0.5 { 0.25 } else { 1.0 }
}

rune_register::slice!(pub GROUP_LIST: Group);

rune_register::register!(GROUP_LIST: Group = [
    Group::new("Global", "window", &[SPEED, SLOW]),
    Group::new("Composer", "composer", &[CAPSULE_FILL, CAPSULE_RADIUS, CAPSULE_RING, JUMP_PLACE, COMPOSER_FADE]),
    TIMELINE_GROUP,
    crate::disclosure::GROUP,
    WORKING,
    crate::compaction::GROUP,
    crate::copied::GROUP,
    session_view::knobs::EXPERIMENTS,
]);

/// The panel's order, which module paths don't give. A group not named here goes after these.
const ORDER: &[&str] = &[
    "Global", "Composer", "Question dock", "Timeline", "Scroll bar", "Tool groups", "Working", "Terminal",
    "Header", "Dividers", "Copy", "Popover", "Stream", "Todo pill",
];

/// The knob groups the panel shows, in order. Each is registered from the module it tunes.
pub fn groups() -> &'static [Group] {
    static ALL: std::sync::OnceLock<Vec<Group>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| crate::stories::ordered(rune_register::collect(&GROUP_LIST), ORDER, |g| g.name))
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
    let ours = crate::keymap::Bind::Knobs.matches_tx(ev, tx) || (ev.key == rune::Key::Escape && !ev.mods.alt && !ev.mods.ctrl);
    if !ours || !rune_knobs::key(&mut s.knobs, ev) {
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

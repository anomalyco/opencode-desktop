//! Live look-and-feel sliders: ⌘⇧K, pick a region, drag a knob, and Copy puts the values on the
//! clipboard to paste back to an agent.

use rune::{El, KeyEvent, Tx};
use rune_knobs::{Group, Knob};

use crate::App;

/// The subagent capsule's fill: 1 is Desktop's (the composer's surface), 0 a bare hairline.
pub const CAPSULE_FILL: Knob = Knob::new("Capsule fill", 0.0, 1.0, 1.0);

const GROUPS: &[Group] = &[Group::new("Composer", "composer", &[CAPSULE_FILL])];

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

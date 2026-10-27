//! Live look-and-feel sliders: ⌘⇧K, pick a region, drag a knob, and Copy puts the values on the
//! clipboard to paste back to an agent.

use rune::{El, KeyEvent, Tx};
use rune_knobs::{Group, Knob};

use crate::App;

/// The subagent capsule's fill: 1 is Desktop's (the composer's surface), 0 a bare hairline.
pub const CAPSULE_FILL: Knob = Knob::new("Capsule fill", 0.0, 1.0, 1.0);
/// Its corner radius in points: 12 is Desktop's; 14 makes a full pill.
pub const CAPSULE_RADIUS: Knob = Knob::new("Capsule radius", 0.0, 14.0, 12.0).step(1.0);
/// Its hairline: 1 is Desktop's raised ring (the composer's), 0 the faint muted border.
pub const CAPSULE_RING: Knob = Knob::new("Capsule ring", 0.0, 1.0, 1.0);

/// Project icon marks (`project_icon::Look`); `--icon-style <name>` sets the style at launch.
pub const ICON_STYLE: Knob = Knob::choice("Icon style", &project_icon::Style::NAMES, 1);
pub const ICON_SATURATION: Knob = Knob::new("Icon saturation", 0.0, 1.6, 1.0);
pub const ICON_GRAIN: Knob = Knob::new("Icon grain", 0.0, 0.4, 0.12);

const GROUPS: &[Group] = &[
    Group::new("Composer", "composer", &[CAPSULE_FILL, CAPSULE_RADIUS, CAPSULE_RING]),
    Group::new("Icons", "sidebar", &[ICON_STYLE, ICON_SATURATION, ICON_GRAIN]),
    Group::new("Icon playground", "icon-playground", &[ICON_STYLE, ICON_SATURATION, ICON_GRAIN]),
];

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

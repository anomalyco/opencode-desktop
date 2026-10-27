//! Live look-and-feel sliders: ⌘⇧K opens the panel, drag a knob, and Copy puts the values on the
//! clipboard to paste back to an agent.

use rune::{El, KeyEvent, Tx, row, spacer};
use rune_knobs::Knob;

use crate::App;
use crate::palette::Pal;

/// The subagent capsule's fill: 1 is Desktop's (the composer's surface), 0 a bare hairline.
pub const CAPSULE_FILL: Knob = Knob::new("Capsule fill", 0.0, 1.0, 1.0);

const ALL: &[Knob] = &[CAPSULE_FILL];

/// ⌘⇧K toggles the panel.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let m = ev.mods;
    if !(ev.is_command('k') || ev.is_command('K')) || !m.shift || m.alt || m.ctrl {
        return false;
    }
    tx.instant();
    s.knobs.open = !s.knobs.open;
    true
}

pub fn panel(s: &App, p: &Pal) -> Option<El> {
    if !s.knobs.open {
        return None;
    }
    let style = rune_knobs::Style { bg: p.layer2, text: p.text, muted: p.muted, track: p.hover, fill: p.text };
    Some(row().key("knobs-layer").absolute(0.0, 0.0).w_full().pt(48.0).pr(24.0).child(spacer()).child(rune_knobs::panel(&s.knobs, ALL, style, |s: &mut App| &mut s.knobs)))
}

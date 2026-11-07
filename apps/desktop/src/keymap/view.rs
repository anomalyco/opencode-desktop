//! The table drawn: keycap chips for a command.

use rune::{El, TextStyle, row, text};

use super::Bind;
use crate::palette::Pal;

/// `keybind-v2-label`: 11px, 530, line-height 1, 0.05px tracking, tabular figures (`keybind.css:55-75`).
const LABEL: TextStyle = TextStyle::new(11.0, 530.0).lh(11.0).tracking(0.05).features("\"tnum\", \"lnum\"");

/// A command's chord as keycap chips (Desktop's `Keybind`, whose labels are uppercase: CTRL
/// SHIFT T off macOS, ⇧ ⌘ T on it). Empty when the command has no chord here.
pub fn keycaps(p: &Pal, bind: Bind) -> El {
    chips(p, bind.keycaps())
}

/// Desktop's neutral `Keybind` (`keybind.css`): 2px apart, each key 14px tall and at least 14px
/// wide with 4px sides, 2px corners on `bg-layer-03`, its label uppercase in `text-muted`.
pub fn chips(p: &Pal, caps: Vec<String>) -> El {
    caps.iter().fold(row().key("keys").gap(2.0).center_y(), |r, c| {
        r.child(row().h(14.0).min_w(14.0).px(4.0).center().radius(2.0).fill(p.layer3).child(text(c.to_uppercase()).font(LABEL).color(p.muted)))
    })
}

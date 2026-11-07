//! The table drawn: keycap chips for a command, and Settings' read-only shortcuts list.

use rune::{El, TextStyle, col, el, row, spacer, text};

use super::{Bind, Section, all};
use crate::App;
use crate::palette::Pal;
use crate::view::{UI, UI_MED};

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

/// Settings' Keyboard shortcuts: each listed command and its chord here, by section
/// (Desktop's Settings > Shortcuts, read-only until shortcuts can be changed).
pub fn settings_section(_s: &App, p: &Pal) -> El {
    let mut sections = col().key("shortcuts").w_full().gap(24.0);
    for section in Section::ALL {
        let rows: Vec<_> = all().iter().filter(|b| b.section == section && !b.hidden).collect();
        if rows.is_empty() {
            continue;
        }
        let n = rows.len();
        let mut list = col().key(format!("shortcuts-{}", section.title())).w_full().px(20.0).radius(8.0).fill(p.layer1).box_shadow(Pal::ring_in(p.border_muted));
        for (i, b) in rows.into_iter().enumerate() {
            let keys = b.bind.label();
            let shown = if keys.is_empty() { text("Unassigned").font(UI).color(p.faint) } else { text(keys).font(UI_MED).color(p.text) };
            let line = row().key(format!("shortcut-{}", b.id)).w_full().h(56.0).gap(16.0).center_y().child(text(b.title).font(UI).color(p.text)).child(spacer()).child(shown);
            list = list.child(line).children((i + 1 < n).then(|| el().w_full().h(0.5).fill(p.border)));
        }
        sections = sections.child(col().w_full().gap(12.0).child(text(section.title()).font(UI).color(p.muted)).child(list));
    }
    crate::settings::section(p, "Keyboard shortcuts", sections)
}

//! The table drawn: keycap chips for a command, and Settings' read-only shortcuts list.

use rune::{El, col, el, row, spacer, text};

use super::{Bind, Section, all};
use crate::App;
use crate::palette::Pal;
use crate::view::{UI, UI_MED};

/// A command's chord as keycap chips (Desktop's `Keybind`, whose labels are uppercase: CTRL
/// SHIFT T off macOS, ⇧ ⌘ T on it). Empty when the command has no chord here.
pub fn keycaps(p: &Pal, bind: Bind) -> El {
    bind.keycaps().iter().fold(row().key("keys").gap(2.0).center_y(), |r, c| r.child(crate::queue::keycap(p, &c.to_uppercase())))
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
        let mut list = col().key(format!("shortcuts-{}", section.title())).w_full().px(20.0).radius(8.0).fill(p.layer1).border(0.5, p.border_muted);
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

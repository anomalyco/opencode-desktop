//! Pieces of a drawn menu's rows: a submenu row's chevron, a change count's note and a row that
//! only tells.

use rune::{El, Icon, TextStyle, icon, row, text};

use super::{Glyph, Item};
use crate::palette::Pal;
use crate::popover::MENU;

/// `menu-v2-item-chevron`: a submenu row's end.
pub(super) static CHEVRON: Icon = Icon { name: "menu-v2-chevron", view: 16.0, body: r#"<path d="M6 4L10 8L6 12V4Z" fill="currentColor"/>"# };

/// `· +added -removed`: the separator muted, the counts in the diff's inks as `DiffChanges`
/// standard draws them (the mono face at 14, 8 apart).
pub(super) fn diff_note(p: &Pal, added: u32, removed: u32) -> El {
    let st = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
    let count = TextStyle::new(14.0, 440.0).mono().lh(21.0);
    row().gap(8.0).center_y()
        .child(text("·").font(st).color(p.muted))
        .child(row().gap(8.0).center_y().child(text(format!("+{added}")).dir(rune::Direction::Ltr).font(count).color(p.diff_add)).child(text(format!("-{removed}")).dir(rune::Direction::Ltr).font(count).color(p.diff_del)))
}

/// A row that only tells (`SessionLocationDetail`): its 16 pt glyph in the muted ink and its
/// line, 13/440 in the base ink, 28 tall.
pub(super) fn detail_row(p: &Pal, key: &str, i: &Item) -> El {
    row()
        .key(key.to_owned())
        .w_full()
        .h(MENU.row_h)
        .pl(MENU.pad_l)
        .pr(MENU.pad_r)
        .gap(MENU.gap)
        .center_y()
        .children(i.glyph.and_then(Glyph::mono).map(|g| icon(g, 16.0).color(p.icon_muted)))
        .child(text(i.title.clone()).font(TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04)).color(p.text).truncate_fade().shrink())
}

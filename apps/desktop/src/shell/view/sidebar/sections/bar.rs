//! The selection bar: while tabs are selected, a small raised bar along the sidebar's foot counts
//! them and offers Group (⌘G, a new section of them) and Clear (Esc).

use super::*;
use crate::tabs::sections as sec;
use opencode_i18n::t;

const COUNT: TextStyle = TextStyle::new(12.0, 500.0).lh(16.0).features("\"tnum\"");
const BUTTON: TextStyle = TextStyle::new(12.0, 520.0).lh(16.0);

pub(super) fn bar(s: &App, p: &Pal) -> El {
    let n = sec::state(s).selection.len();
    let group = row()
        .key("sections-group")
        .role("button")
        .label_static(t!("rune.sections.new"))
        .help(t!("rune.sections.new"))
        .help_keys(crate::keymap::Bind::FindNext.keycaps())
        .h(28.0)
        .pl(9.0)
        .pr(6.0)
        .gap(6.0)
        .center_y()
        .radius(7.0)
        .focus_inset(p)
        .fill(p.accent.alpha(0.17))
        .hover(p.accent.alpha(0.26))
        .press(p.accent.alpha(0.32))
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            let uids = sec::state(s).selection.uids().to_vec();
            sec::create(s, &uids, tx);
        })
        .child(text(t!("rune.sections.group")).font(BUTTON).color(p.text))
        .child(crate::keymap::chips(p, crate::keymap::Bind::FindNext.keycaps()));
    let clear = row()
        .key("sections-clear")
        .role("button")
        .label_static(t!("rune.sections.clearSelection"))
        .help(t!("rune.sections.clearSelection"))
        .size(28.0, 28.0)
        .center()
        .radius(7.0)
        .focus_inset(p)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            sec::clear_selection(s);
        })
        .child(icon(&ic::XMARK_SMALL, 14.0).color(p.icon_muted));
    let mut b = row()
        .key("sections-bar")
        .role("toolbar")
        .label(t!("rune.sections.selected", count = n))
        .pin(Some(2.0), None, Some(2.0), Some(4.0))
        .h(38.0)
        .pl(10.0)
        .pr(4.0)
        .gap(4.0)
        .center_y()
        .radius(10.0)
        .fill(if p.dark { p.layer1 } else { p.bg_base })
        .box_shadow(Pal::ring_in(p.border))
        .child(text(t!("rune.sections.selected", count = n)).key("count").font(COUNT).color(p.muted).grow(1.0).shrink().min_w(0.0).truncate())
        .child(group)
        .child(clear)
        .raise();
    for shadow in p.floating {
        b = b.box_shadow(shadow);
    }
    b
}

//! The selection bar: while tabs are selected, a small raised bar along the sidebar's foot, as
//! wide as a tab's pill, counts them (on the tabs' spine) and offers Group (⌘G, a new section of
//! them) and Clear (Esc), its cross on the tabs' own end column.

use super::*;
use crate::tabs::sections as sec;
use opencode_i18n::t;

const COUNT: TextStyle = TextStyle::new(12.0, 500.0).lh(16.0).features("\"tnum\"");
const BUTTON: TextStyle = TextStyle::new(12.0, 520.0).lh(16.0);
const KEYS: TextStyle = TextStyle::new(11.0, 500.0).lh(16.0);
/// The bar's height; its buttons stand 6 pt in from its edges all round.
pub(super) const H: f32 = 36.0;
const BUTTON_H: f32 = 24.0;
/// The bar rises 8 pt as it fades in with the first pick and sinks as it goes.
const RISE: f32 = 8.0;
const IN: Spring = Spring::new(0.24, 0.0);
const OUT: Spring = Spring::new(0.18, 0.0);

pub(super) fn bar(s: &App, p: &Pal) -> El {
    let n = sec::state(s).selection.len();
    let keys = crate::keymap::Bind::FindNext.keycaps();
    // The keys as one quiet word, as a menu writes them: a keycap's box inside the button inside
    // the bar would be three boxes deep.
    let hint = text(keys.concat().to_uppercase()).key("keys").font(KEYS).color(p.text.alpha(0.5)).dir(rune::Direction::Ltr);
    let group = row()
        .key("sections-group")
        .role("button")
        .label_static(t!("rune.sections.new"))
        .help(t!("rune.sections.new"))
        .help_keys(keys)
        .h(BUTTON_H)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .focus_inset(p)
        .fill(p.accent.alpha(0.16))
        .hover(p.accent.alpha(0.10))
        .press(p.accent.alpha(0.16))
        .box_shadow(Pal::ring_in(p.accent.alpha(0.2)))
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            let uids = sec::state(s).selection.uids().to_vec();
            sec::create(s, &uids, tx);
        })
        .child(text(t!("rune.sections.group")).font(BUTTON).color(p.text))
        .child(hint);
    let clear = row()
        .key("sections-clear")
        .role("button")
        .label_static(t!("rune.sections.clearSelection"))
        .help(t!("rune.sections.clearSelection"))
        .size(BUTTON_H, BUTTON_H)
        .center()
        .radius(6.0)
        .focus_inset(p)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press(sec::clear_selection)
        .child(icon(&ic::XMARK_SMALL, 14.0).color(p.icon_muted));
    let mut b = row()
        .key("sections-bar")
        .role("toolbar")
        .label(t!("rune.sections.selected", count = n))
        .pin(Some(0.0), None, Some(0.0), Some(0.0))
        .h(H)
        .pl(8.0)
        .pr(6.0)
        .gap(2.0)
        .center_y()
        .radius(9.0)
        .fill(if p.dark { p.layer1 } else { p.bg_base })
        .child(text(t!("rune.sections.selected", count = n)).key("count").font(COUNT).color(p.muted).grow(1.0).shrink().min_w(0.0).truncate())
        .child(group)
        .child(clear)
        .transition(rune::Transition::offset(0.0, RISE).combined(rune::Transition::opacity()).springs(rune::SpringPair::new(IN, OUT)))
        .raise();
    // Lit from above in dark, where a shadow alone is lost on the sidebar; a plain hairline in light.
    b = if p.dark { b.border_gradient(0.5, Color::WHITE.alpha(0.16), Color::WHITE.alpha(0.06)) } else { b.box_shadow(Pal::ring_in(p.border)) };
    for shadow in p.floating {
        b = b.box_shadow(shadow);
    }
    b
}

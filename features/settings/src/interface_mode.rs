//! Settings › General › Interface layout (`interface-mode.tsx`'s `TabLayoutSetting`,
//! `interface-mode.css`): three 120pt radio cards in the navigation cards' style
//! (opencode#65800), each a window that shows only its navigation over its name. Picking one
//! switches the shell live, around the Settings page, while you watch.

use opencode_i18n::t;
use opencode_ui::palette::Pal;
use rune::{El, Tx, row};

use crate::host::SettingsHost;
use crate::mode_card::{GAP, Sketch, card};
use crate::types::TabLayout;

/// The row's control: the three cards, 10pt apart.
pub(crate) fn control<H: SettingsHost + 'static>(s: &H, p: &Pal) -> El {
    let current = s.tab_layout();
    let group = row()
        .key("interface-mode")
        .role("radiogroup")
        .label_static(t!("settings.general.row.interfaceMode.title"))
        .focus_group()
        .gap(GAP)
        .shrink()
        .min_w(0.0);
    let group = if crate::rows_wrap() { group.w_full().max_w(3.0 * 120.0 + 2.0 * GAP) } else { group };
    TabLayout::ALL.into_iter().fold(group, |r, layout| {
        let (key, sketch) = match layout {
            TabLayout::Horizontal => ("interface-mode-top", Sketch::Top),
            TabLayout::Vertical => ("interface-mode-side", Sketch::Side),
            TabLayout::Inbox => ("interface-mode-inbox", Sketch::Inbox),
        };
        r.child(card(p, key, layout.label(), sketch, layout == current).on_press_tx(move |s: &mut H, tx: &mut Tx| set_layout(s, layout, tx)))
    })
}

fn set_layout(host: &mut dyn SettingsHost, layout: TabLayout, tx: &mut Tx) {
    host.set_tab_layout(layout, tx);
}

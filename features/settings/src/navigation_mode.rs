//! Settings › Preferences › Navigation (`interface-mode.tsx`'s `NavigationSetting`,
//! `interface-mode.css`'s `navigation-mode`, #64668): under Experimental › Consolidated, two
//! 120 pt radio cards (Tabs and List), plus Tab position (`Top tabs` / `Side tabs`) when Tabs is chosen.

use opencode_i18n::t;
use opencode_ui::palette::Pal;
use rune::{El, Tx, row};

use crate::host::SettingsHost;
use crate::mode_card::{GAP, Sketch, card};
use crate::prefs::{Choice as _, Navigation};
use crate::types::TabLayout;

/// Two 120 pt radio cards, 10 pt apart (`navigation-mode`), in the layout cards' style
/// (`crate::mode_card`). The Tabs card shows the tabs where Tab position puts them.
pub(crate) fn control<H: SettingsHost + 'static>(s: &H, p: &Pal) -> El {
    let current = s.settings().navigation();
    let side = s.settings().tab_position() == TabLayout::Vertical;
    let group = row()
        .key("navigation-mode")
        .role("radiogroup")
        .label_static(t!("settings.general.row.navigation.title"))
        .focus_group()
        .gap(GAP)
        .shrink()
        .min_w(0.0);
    Navigation::ALL.iter().copied().fold(group, |r, mode| {
        let (key, sketch) = match mode {
            Navigation::Tabs => ("navigation-mode-tabs", if side { Sketch::Side } else { Sketch::Top }),
            Navigation::List => ("navigation-mode-list", Sketch::List),
        };
        r.child(card(p, key, mode.label(), sketch, mode == current).on_press_tx(move |s: &mut H, tx: &mut Tx| pick(s, mode, tx)))
    })
}

fn pick(host: &mut dyn SettingsHost, mode: Navigation, tx: &mut Tx) {
    crate::change_settings(host, |x| x.navigation = mode);
    host.sync_tab_layout(tx);
}

/// Tab position choices (`settings-tab-position`): Top tabs or Side tabs.
pub(crate) fn positions() -> Vec<(String, String)> {
    [TabLayout::Horizontal, TabLayout::Vertical].into_iter().map(|l| (l.name().to_owned(), l.label().to_owned())).collect()
}

pub(crate) fn pick_position(host: &mut dyn SettingsHost, id: &str, tx: &mut Tx) {
    host.set_tab_layout(TabLayout::parse(Some(id)), tx);
}


//! `SettingsServerSwitcher` (`settings/servers/switcher.tsx`): on the pages that show one
//! server's settings, while Settings lists more than one server, the outline button with the
//! server's health, "Server" and its name. Its menu chooses the server those pages show, inside
//! Settings alone: the window's tabs and page stay. Each choice has its health and name ("This
//! computer" beside the app's own), "Unreachable" when it can't be reached; Manage servers opens
//! Settings › Servers.

use opencode_kit::servers::ServerKey;
use opencode_kit::servers::health::indicator;
use rune::{Align, El, Tx, row, text};
use rune_kit::ButtonVariant;
use rune_theme_opencode::icons as ic;

use super::inventory::{self, Kind};
use crate::App;
use crate::palette::Pal;
use crate::popup::{self, Item, Menu};
use opencode_i18n::t;

/// The trigger; nothing with one server.
#[allow(deprecated)]
pub fn view(s: &App, p: &Pal) -> Option<El> {
    if inventory::list(s).len() < 2 {
        return None;
    }
    let selected = inventory::selected(s);
    let (ink, chevron) = (p.muted, p.icon_muted);
    let mark = crate::servers::indicator::view(p, selected.availability.health().as_ref());
    let name = selected.name.clone();
    let open = s.popup.open_at("settings-server-switcher");
    // `size="small" variant="outline"`, 6 in and 6 apart, 240 wide at most.
    let trigger = crate::widgets::button(format!("{} {name}", t!("settings.server.switcher.label")), ButtonVariant::Outline)
        .size(rune_kit::ButtonSize::Small)
        .expanded(open)
        .label_with(move |font, color| {
            row().gap(6.0).center_y().shrink().min_w(0.0)
                .child(row().size(16.0, 16.0).center().child(mark))
                .child(text(t!("settings.server.switcher.label")).font(rune::TextStyle { weight: 440.0, ..font }).color(ink))
                .child(text(name).font(font).color(color).truncate().shrink().min_w(0.0))
                .child(rune::icon(&ic::CHEVRON_DOWN, 16.0).color(chevron))
        })
        .on_press_tx(|s: &mut App, tx: &mut Tx| open_menu(s, tx))
        .build(crate::palette::kit(p))
        .px(6.0)
        .max_w(240.0)
        .key("settings-server-switcher");
    Some(trigger)
}

fn open_menu(s: &mut App, tx: &mut Tx) {
    let servers = inventory::list(s);
    let current = inventory::selected(s).key;
    let this_computer = t!("server.thisComputer");
    let items = servers.iter().map(|x| {
        let item = Item::new(x.key.as_str(), x.name.clone()).status(indicator(x.availability.health().as_ref())).checked(x.key == current);
        let item = if matches!(x.kind, Kind::Own) && x.name != this_computer { item.hint(this_computer) } else { item };
        if x.availability.unreachable() { item.note(t!("settings.server.switcher.unreachable")) } else { item }
    });
    // `.settings-server-switcher-menu`: as wide as its rows, 220 at least, its rows 8 in;
    // `placement="bottom-end" gutter={6}`.
    let menu = Menu::new(t!("settings.server.switcher.label")).items(items).separator().item(Item::new("\u{0}manage", t!("settings.server.switcher.manage")).slot_icon(&ic::SERVER))
        .align(Align::End).gap(6.0).item_inset(8.0).min_width(220.0).hug();
    popup::open(s, menu, "settings-server-switcher", tx, |s: &mut App, picked: Option<&str>, tx: &mut Tx| match picked {
        None => {}
        Some("\u{0}manage") => s.open_settings_page(super::super::Page::Servers, tx),
        Some(key) => select(s, &ServerKey::new(key), tx),
    });
}

/// `surface.selectServer`: Settings' per-server pages show `key` from now on, read afresh. The
/// window works with it while they show (`servers::runtime::desired`); its tabs and page stay.
pub fn select(s: &mut App, key: &ServerKey, tx: &mut Tx) {
    tx.instant();
    s.settings_pages.server.key = key.clone();
    if s.settings_pages.page.per_server() {
        crate::servers::runtime::switch(s, key);
        let page = s.settings_pages.page.clone();
        s.show_settings_page(page);
    }
    super::unavailable::poll(s, tx);
}

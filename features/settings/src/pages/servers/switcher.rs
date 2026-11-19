//! `SettingsServerSwitcher` (`settings/servers/switcher.tsx`): on the pages that show one
//! server's settings, while Settings lists more than one server, the outline button with the
//! server's health, "Server" and its name.

use opencode_i18n::t;
use opencode_kit::servers::ServerKey;
use opencode_servers::health::{Indicator, indicator};
use opencode_ui::popup::{self, Indicator as PopupIndicator, Item, Menu, MenuButton as _, PopupHost};
use opencode_ui::widgets;
use rune::{Align, El, Tx, row, text};
use rune_kit::ButtonVariant;
use rune_theme_opencode::icons as ic;
use session_view::palette::{Pal, kit};

use super::inventory::{self, Kind};
use crate::host::SettingsHost;

/// The trigger; nothing with one server.
#[allow(deprecated)]
pub fn view<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal) -> Option<El> {
    if inventory::list(s).len() < 2 {
        return None;
    }
    let selected = inventory::selected(s);
    let (ink, chevron) = (p.muted, p.icon_muted);
    let mark = super::status_indicator(p, selected.availability.health().as_ref());
    let name = selected.name.clone();
    let open = s.popup().open_at("settings-server-switcher");
    let trigger = widgets::button(format!("{} {name}", t!("settings.server.switcher.label")), ButtonVariant::Outline)
        .size(rune_kit::ButtonSize::Small)
        .expanded(open)
        .label_with(move |font, color| {
            row()
                .gap(6.0)
                .center_y()
                .shrink()
                .min_w(0.0)
                .child(row().size(16.0, 16.0).center().child(mark))
                .child(text(t!("settings.server.switcher.label")).font(rune::TextStyle { weight: 440.0, ..font }).color(ink))
                .child(text(name).font(font).color(color).truncate().shrink().min_w(0.0))
                .child(rune::icon(&ic::CHEVRON_DOWN, 16.0).color(chevron))
        })
        .build(kit(p))
        .menu_button("settings-server-switcher", |s: &mut H, tx: &mut Tx| open_menu(s, tx))
        .px(6.0)
        .max_w(240.0)
        .key("settings-server-switcher");
    Some(trigger)
}

fn popup_indicator(ind: Indicator) -> PopupIndicator {
    match ind {
        Indicator::Healthy => PopupIndicator::Healthy,
        Indicator::Unreachable => PopupIndicator::Unreachable,
        Indicator::Unknown => PopupIndicator::Unknown,
        Indicator::Auth => PopupIndicator::Auth,
        Indicator::Connecting => PopupIndicator::Connecting,
        Indicator::Incompatible => PopupIndicator::Incompatible,
    }
}

fn build_switcher_menu(s: &dyn SettingsHost) -> Menu {
    let servers = inventory::list(s);
    let current = inventory::selected(s).key;
    let this_computer = t!("server.thisComputer");
    let items = servers.iter().map(|x| {
        let item = Item::new(x.key.as_str(), x.name.clone()).status(popup_indicator(indicator(x.availability.health().as_ref()))).checked(x.key == current);
        let item = if matches!(x.kind, Kind::Own { .. }) && x.name != this_computer { item.hint(this_computer) } else { item };
        if x.availability.unreachable() { item.note(t!("settings.server.switcher.unreachable")) } else { item }
    });
    Menu::new(t!("settings.server.switcher.label"))
        .items(items)
        .separator()
        .item(Item::new("\u{0}manage", t!("settings.server.switcher.manage")).slot_icon(&ic::SERVER))
        .align(Align::End)
        .gap(6.0)
        .item_inset(8.0)
        .min_width(220.0)
        .hug()
}

fn pick_switcher(s: &mut dyn SettingsHost, picked: Option<&str>, tx: &mut Tx) {
    match picked {
        None => {}
        Some("\u{0}manage") => crate::pages::open_page(s, crate::pages::Page::Servers, tx),
        Some(key) => select(s, &ServerKey::new(key), tx),
    }
}

fn open_menu<H: SettingsHost + PopupHost + 'static>(s: &mut H, tx: &mut Tx) {
    let menu = build_switcher_menu(s);
    popup::open(s, menu, "settings-server-switcher", tx, |s: &mut H, picked: Option<&str>, tx: &mut Tx| pick_switcher(s, picked, tx));
}

/// `surface.selectServer`: Settings' per-server pages show `key` from now on, read afresh.
pub fn select(s: &mut dyn SettingsHost, key: &ServerKey, tx: &mut Tx) {
    tx.instant();
    s.settings_pages_mut().server.key = key.clone();
    if s.settings_pages().page.per_server() {
        s.switch_server(key);
        let page = s.settings_pages().page.clone();
        crate::pages::show_page(s, page);
    }
    super::unavailable::poll(s, tx);
}

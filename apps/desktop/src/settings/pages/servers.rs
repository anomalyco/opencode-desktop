//! Settings › Server / Servers (`settings/servers/servers.tsx`): with one server, its connection
//! (its health, name and address, and its row menu) and its preferences (Terminal shell, from the
//! server's shells, written to its global config); with more, one card per server
//! ([`inventory`]), each with its connection row (the app's own, one added by address, or the row
//! its plugin draws) and, while its pages can load, its own Terminal shell. Add server offers the
//! plugins' `server.add` items.
//!
//! [`switcher`] is Desktop's `SettingsServerSwitcher` on the pages that show one server's
//! settings, and [`unavailable`] the page they show while that server can't be reached.

pub mod inventory;
pub mod switcher;
pub mod unavailable;

use std::collections::HashMap;

use opencode_kit::servers::ServerKey;
use rune::{Align, El, Tx, col, el, icon, row, rule, text};
use rune_kit::ButtonVariant;
use rune_theme_opencode::icons as ic;
use serde_json::{Value, json};

use super::{ghost_menu_button, header_with, page_with};
use crate::App;
use crate::jobs::Apply;
use crate::palette::Pal;
use crate::popup::{self, Item, Menu};
use crate::settings::{self, setting};
use crate::widgets::BODY_MED;
use inventory::{Kind, Server};
use opencode_i18n::t;

const META: rune::TextStyle = rune::TextStyle::new(11.0, 440.0).lh(11.0);

/// What the page read: each server's shells and its global config's choice.
#[derive(Default)]
pub struct State {
    pub shells: HashMap<ServerKey, Shells>,
    /// Add server or Edit server, while it's open.
    pub dialog: crate::servers::dialog::Dialog,
}

/// One server's shells (`ConfigShellOption`: path, name, acceptable) and its configured one.
#[derive(Clone, Debug, Default)]
pub struct Shells {
    pub list: Vec<(String, String, bool)>,
    pub current: String,
}

/// Settings' own server (`SettingsSurface.server`): the one its per-server pages show, which
/// the switcher changes inside Settings alone, and the Retry of the page shown while it can't be
/// reached ([`opencode_kit::servers::settings::step`]) with its timer.
pub struct Scope {
    pub key: ServerKey,
    pub retry: opencode_kit::servers::settings::Retry,
    pub wait: unavailable::Wait,
}

impl Default for Scope {
    fn default() -> Scope {
        Scope { key: ServerKey::sidecar(), retry: opencode_kit::servers::settings::Retry::new(ServerKey::sidecar()), wait: unavailable::Wait::Off }
    }
}

/// How many servers Settings lists.
pub fn count(s: &App) -> usize {
    inventory::list(s).len()
}

/// Reads each reachable server's shells and config.
pub fn load(s: &mut App) {
    for server in inventory::list(s).into_iter().filter(|x| x.availability.pages_connection().is_some()) {
        let key = server.key.clone();
        crate::servers::runtime::with_server(s, &server.key, |s| {
            s.job(move |srv| {
                let shells = srv.get("/api/config/shell").ok();
                let config = srv.get("/api/config").ok();
                Box::new(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    let st = s.settings_pages.servers.shells.entry(key).or_default();
                    if let Some(list) = shells.as_ref().and_then(Value::as_array) {
                        st.list = list.iter().filter_map(|v| Some((v["path"].as_str()?.to_owned(), v["name"].as_str()?.to_owned(), v["acceptable"].as_bool().unwrap_or(true)))).collect();
                    }
                    if let Some(entries) = config.as_ref().and_then(Value::as_array) {
                        st.current = global_shell(entries);
                    }
                }) as Apply
            });
        });
    }
}

/// The shell the global config sets: the last document's before the first folder's.
fn global_shell(entries: &[Value]) -> String {
    let end = entries.iter().position(|e| e["type"] == "directory").unwrap_or(entries.len());
    entries[..end].iter().filter(|e| e["type"] == "document").filter_map(|e| e["info"]["shell"].as_str()).last().unwrap_or_default().to_owned()
}

/// `createShellOptions`: Auto first, then each shell by its name (its path where two share one),
/// "(terminal only)" after one the agent can't use, and the configured one when unlisted.
fn options(st: &Shells) -> Vec<(String, String)> {
    let mut out = vec![(String::new(), t!("settings.general.row.shell.autoDefault").to_owned())];
    for (path, name, acceptable) in &st.list {
        let ambiguous = st.list.iter().filter(|(_, n, _)| n == name).count() > 1;
        let value = if ambiguous { path.clone() } else { name.clone() };
        let shown = if ambiguous { path.clone() } else { name.clone() };
        out.push((value, if *acceptable { shown } else { format!("{shown} ({})", t!("settings.general.row.shell.terminalOnly")) }));
    }
    if !st.current.is_empty() && !out.iter().any(|(v, _)| *v == st.current) {
        out.push((st.current.clone(), st.current.clone()));
    }
    out
}

/// Sets server `key`'s shell in its global config; the choice goes back if the write fails.
fn set_shell(s: &mut App, key: &ServerKey, value: &str) {
    let st = s.settings_pages.servers.shells.entry(key.clone()).or_default();
    let previous = std::mem::replace(&mut st.current, value.to_owned());
    let (value, back) = (value.to_owned(), key.clone());
    crate::servers::runtime::with_server(s, key, |s| {
        s.job(move |srv| {
            let body = json!({ "shell": if value.is_empty() { Value::Null } else { Value::String(value) } });
            let failed = srv.write("PATCH", "/api/experimental/config", Some(body)).err();
            Box::new(move |s: &mut App, tx: &mut Tx| {
                if let Some(e) = failed {
                    tx.instant();
                    s.settings_pages.servers.shells.entry(back).or_default().current = previous;
                    crate::status_toast::say(s, t!("common.requestFailed"), &e);
                }
            }) as Apply
        });
    });
}

pub fn view(s: &App, p: &Pal) -> El {
    let servers = inventory::list(s);
    // `AddServerMenu`: Desktop's own Add server, a menu with the plugins' kinds after it once
    // there are any.
    let open = s.popup.open_at("settings-add-server");
    let add = ghost_menu_button(p, "settings-add-server", t!("dialog.server.add.button"), Some(&ic::PLUS), true, true, open, move |s, tx| {
        let items = crate::plugin_host::server_menu_items(s, "server.add");
        if items.is_empty() {
            return crate::servers::dialog::open(s, None, tx);
        }
        let menu = Menu::new(t!("dialog.server.add.button")).align(Align::End).min_width(160.0).item(Item::new("add", t!("dialog.server.add.button"))).items(items.into_iter().map(|(key, title)| Item::new(key, title)));
        popup::open(s, menu, "settings-add-server", tx, |s: &mut App, picked, tx| match picked {
            Some("add") => crate::servers::dialog::open(s, None, tx),
            Some(key) => crate::plugin_host::run_menu_item(s, key, "", tx),
            None => {}
        });
    });
    let single = servers.len() <= 1;
    let title = if single { t!("settings.section.server") } else { t!("settings.tab.servers") };
    let selected = &s.settings_pages.server.key;
    let mut body = col().w_full().gap(32.0);
    if single {
        let own = &servers[0];
        body = body.child(super::section(p, "settings-server-connection", Some(t!("settings.server.section.connection")), card(p, vec![connection_row(s, p, own)])));
        if own.availability.pages_connection().is_some() {
            body = body.child(super::section(p, "settings-server-preferences", Some(t!("settings.tab.preferences")), settings::list(p, "settings-server-preferences-list", shell_row(s, p, own, "settings-shell"))));
        }
    } else {
        for server in &servers {
            let mut rows = vec![connection_row(s, p, server)];
            if server.availability.pages_connection().is_some() {
                // Search reveals the shell of the server per-server pages show; each other's has
                // its own target (`settings-shell:<key>`).
                let target = if server.key == *selected { "settings-shell".to_owned() } else { format!("settings-shell:{}", server.key) };
                rows.extend(shell_row(s, p, server, &target));
            }
            body = body.child(col().key(format!("settings-server-{}", server.key)).role("region").label(server.name.clone()).w_full().child(card(p, rows)));
        }
    }
    body = body.children(crate::plugin_host::settings_pages(s, p, "servers"));
    page_with(p, "settings-servers-page", header_with(p, title, t!("settings.servers.description"), Some(add)), body, true)
}

/// Server `server`'s Terminal shell row, revealed by search as `target`.
fn shell_row(s: &App, p: &Pal, server: &Server, target: &str) -> Vec<El> {
    let st = s.settings_pages.servers.shells.get(&server.key).cloned().unwrap_or_default();
    let key = server.key.clone();
    let anchor = format!("settings-shell-select-{}", server.key);
    let control = select_for(s, p, &anchor, t!("settings.general.row.shell.title"), options(&st), &st.current, move |s, value| set_shell(s, &key, value));
    super::rows(s, p, 20.0, vec![(target, setting(p, target, t!("settings.general.row.shell.title"), t!("settings.general.row.shell.description"), control))])
}

/// [`select`] whose pick runs a closure.
fn select_for(s: &App, p: &Pal, key: &str, label: &str, options: Vec<(String, String)>, current: &str, pick: impl Fn(&mut App, &str) + Clone + 'static) -> El {
    let shown = options.iter().find(|(id, _)| id == current).map_or_else(String::new, |(_, l)| l.clone());
    let anchor = format!("{key}-trigger");
    let (label, current) = (label.to_owned(), current.to_owned());
    let open = s.popup.open_at(&anchor);
    rune_kit::Select::new(key.to_owned(), &[shown.as_str()]).current(Some(&shown)).label(label.clone()).open(open).trigger(crate::palette::kit(p))
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            let menu = Menu::picker(label.clone(), options.clone(), &current);
            let pick = pick.clone();
            popup::open(s, menu, anchor.clone(), tx, move |s: &mut App, picked, tx| {
                if let Some(id) = picked {
                    tx.instant();
                    pick(s, id);
                }
            });
        })
}

/// `settings-list` as a server card has it: 16 pt in. A connection row with a row after it ends
/// 16 lower than its 20 top, on a hairline, 16 above the next (`.settings-servers-row:not(:last-child)`).
fn card(p: &Pal, rows: Vec<El>) -> El {
    let n = rows.len();
    rows.into_iter().enumerate().fold(col().key("settings-server-list").w_full().px(16.0).radius(8.0).fill(p.layer1).box_shadow(Pal::ring_in(p.border_muted)), |l, (at, r)| {
        if at + 1 < n { l.child(r.pb(16.0)).child(rule(0.5, p.border)).child(el().h(16.0)) } else { l.child(r) }
    })
}

/// A server's connection row (`ServerConnectionRow`): a plugin's server as its plugin draws it,
/// else its health, its name over its address, and its menu.
fn connection_row(s: &App, p: &Pal, server: &Server) -> El {
    match &server.kind {
        Kind::Plugin(rec) => {
            let c = crate::plugin_host::contributed_servers(s).into_iter().find(|c| c.key == server.key.as_str());
            let drawn = c.and_then(|c| crate::plugin_host::server_row(s, p, &c)).unwrap_or_else(|| text(rec.name.clone()).font(BODY_MED.lh(16.0)).color(p.text));
            col().key(format!("settings-server-row-{}", server.key)).w_full().py(20.0).child(drawn)
        }
        Kind::Http(http) => col().key(format!("settings-server-row-{}", server.key)).w_full().py(20.0).child(http_row(p, http)),
        Kind::Own => own_row(p, server),
    }
}

/// `.settings-servers-row` of the app's own server: the health mark, the name over the address,
/// the ⋯ menu (Edit is for servers added by address).
fn own_row(p: &Pal, server: &Server) -> El {
    let url = server.address();
    let dots = row().key("settings-server-menu").role("button").label(t!("common.moreOptions")).size(24.0, 24.0).center().radius(4.0)
        .hover(p.hover).press(p.pressed)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            let url = inventory::list(s).into_iter().next().map(|x| x.address()).unwrap_or_default();
            let menu = Menu::new(t!("settings.section.server")).align(Align::End).group(t!("settings.section.server"))
                .item(Item::new("edit", t!("dialog.server.menu.edit")).enabled(false))
                .item(Item::new("copy", t!("rune.settings.server.copyAddress")).enabled(!url.is_empty()));
            // In a browser the page's server is signed in to by the page, which can sign out.
            let menu = if crate::page_sign_in::can_sign_out() { menu.separator().item(Item::new("sign-out", t!("rune.web.signOut"))) } else { menu };
            popup::open(s, menu, "settings-server-menu", tx, move |_: &mut App, picked, tx| match picked {
                Some("copy") => { tx.copy(url.clone()); }
                Some("sign-out") => crate::page_sign_in::sign_out(),
                _ => {}
            });
        })
        .child(icon(&ic::OUTLINE_DOTS, 16.0).color(p.icon_muted));
    row().key("settings-server-local").w_full().py(20.0).gap(16.0).center_y()
        .child(row().grow(1.0).shrink().min_w(0.0).gap(10.0).items_start()
            .child(crate::servers::indicator::view(p, server.health.as_ref()))
            .child(col().grow(1.0).shrink().min_w(0.0).gap(6.0)
                .child(text(server.name.clone()).font(BODY_MED.lh(16.0)).color(p.text).truncate())
                .children((!url.is_empty()).then(|| text(url).font(META).color(p.muted).truncate()))))
        .child(dots)
}

/// A server added by address (`ServerConnectionRow`'s own row), inside its card's padding: its
/// health (a lock once it rejects the saved sign-in, with Authenticate), its name over its
/// address, and its menu (Edit, Remove).
#[allow(deprecated)]
pub(crate) fn http_row(p: &Pal, server: &crate::servers::Server) -> El {
    let key = opencode_kit::servers::ServerConn::http_server(&server.url).map_or_else(|| ServerKey::new(&server.url), |c| c.key());
    let health = crate::servers::health::get(&key);
    let anchor = format!("settings-server-menu-{}", server.url);
    let url = server.url.clone();
    let dots = row().key(anchor.clone()).role("button").label(t!("common.moreOptions")).size(24.0, 24.0).center().radius(4.0)
        .hover(p.hover).press(p.pressed)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            let menu = Menu::new(t!("settings.section.server")).align(Align::End).group(t!("settings.section.server"))
                .item(Item::new("edit", t!("dialog.server.menu.edit")))
                .separator()
                .item(Item::new("remove", t!("dialog.server.menu.remove")));
            let menu = if crate::page_sign_in::is_page_server(&url) { menu.separator().item(Item::new("sign-out", t!("rune.web.signOut"))) } else { menu };
            let url = url.clone();
            popup::open(s, menu, anchor.clone(), tx, move |s: &mut App, picked, tx| match picked {
                Some("sign-out") => crate::page_sign_in::sign_out(),
                Some("edit") => crate::servers::dialog::open(s, Some(&url), tx),
                Some("remove") => {
                    tx.instant();
                    crate::servers::remove_offering(s, &url);
                    crate::servers::dialog::sync(s, tx);
                }
                _ => {}
            });
        })
        .child(icon(&ic::OUTLINE_DOTS, 16.0).color(p.icon_muted));
    let authenticate = opencode_kit::servers::health::is_unauthorized(health.as_ref()).then(|| {
        let url = server.url.clone();
        crate::widgets::button(t!("server.action.authenticate"), ButtonVariant::Neutral).size(rune_kit::ButtonSize::Small)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| crate::servers::dialog::open(s, Some(&url), tx))
            .build(crate::palette::kit(p))
            .key(format!("settings-server-authenticate-{}", server.url))
    });
    row().w_full().gap(16.0).center_y()
        .child(row().grow(1.0).shrink().min_w(0.0).gap(10.0).items_start()
            .child(crate::servers::indicator::view(p, health.as_ref()))
            .child(col().grow(1.0).shrink().min_w(0.0).gap(6.0)
                .child(text(server.label()).font(BODY_MED.lh(16.0)).color(p.text).truncate())
                .child(text(server.url.clone()).font(META).color(p.muted).truncate())))
        .child(row().gap(8.0).center_y().children(authenticate).child(dots))
}

/// Desktop's `SettingsServerSwitcher` on a per-server page's header ([`switcher::view`]).
pub fn switcher(s: &App, p: &Pal) -> Option<El> {
    switcher::view(s, p)
}

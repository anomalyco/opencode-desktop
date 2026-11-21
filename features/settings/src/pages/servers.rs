//! Settings › Server / Servers (`settings/servers/servers.tsx`, `servers/registry/row-menu.tsx`):
//! with one server, its connection (health, name, address, and row menu) and its preferences
//! (Terminal shell, from the server's shells, written to its global config); with more, one card
//! per server ([`inventory`]), with each reachable server's Terminal shell picker moved into a
//! submenu of its `...` row menu (`anomalyco/opencode#63539`).

pub mod inventory;
pub mod switcher;
pub mod unavailable;

use std::collections::HashMap;

use opencode_i18n::t;
use opencode_kit::servers::ServerKey;
use opencode_servers::ServerConn;
use opencode_servers::health::{Health, Indicator, indicator, is_unauthorized};
use opencode_ui::popup::{self, Item, Menu, MenuButton as _, PopupHost};
use opencode_ui::widgets::{self, BODY_MED};
use rune::{Align, El, Tx, col, el, icon, row, rule, text};
use rune_kit::ButtonVariant;
use rune_theme_opencode::icons as ic;
use serde_json::Value;
use opencode_ui::focus::FocusExt;
use opencode_ui::palette::{Pal, kit};

use super::{ghost_menu_button, header_with, page_with};
use crate::host::{HttpServer, SettingsHost};
use crate::jobs::{SettingsJob, SettingsRequest};
use crate::{list, setting};
use inventory::{Kind, Server};

const META: rune::TextStyle = rune::TextStyle::new(11.0, 440.0).lh(11.0);

/// What the page read: each server's shells and its global config's choice.
#[derive(Default)]
pub struct State {
    pub shells: HashMap<ServerKey, Shells>,
}

/// One server's shells (`ConfigShellOption`: path, name, acceptable) and its configured one.
#[derive(Clone, Debug, Default)]
pub struct Shells {
    pub list: Vec<(String, String, bool)>,
    pub current: String,
}

/// Settings' own server (`SettingsSurface.server`): the one its per-server pages show, which
/// the switcher changes inside Settings alone, and the Retry of the page shown while it can't be
/// reached with its timer.
pub struct Scope {
    pub key: ServerKey,
    pub retry: opencode_servers::settings::Retry,
    pub wait: unavailable::Wait,
}

impl Default for Scope {
    fn default() -> Scope {
        Scope {
            key: ServerKey::sidecar(),
            retry: opencode_servers::settings::Retry::new(ServerKey::sidecar()),
            wait: unavailable::Wait::Off,
        }
    }
}

/// How many servers Settings lists.
pub fn count(s: &dyn SettingsHost) -> usize {
    inventory::list(s).len()
}

/// Reads each reachable server's shells and config.
pub fn load(s: &mut dyn SettingsHost) {
    let keys: Vec<ServerKey> = inventory::list(s)
        .into_iter()
        .filter(|x| x.availability.pages_connection().is_some())
        .map(|x| x.key)
        .collect();
    for key in keys {
        s.spawn_job(SettingsJob::ForServer {
            server: key.clone(),
            request: SettingsRequest::LoadServerShells { key },
        });
    }
}

/// The shell the global config sets: the last document's before the first folder's.
pub fn global_shell(entries: &[Value]) -> String {
    let end = entries.iter().position(|e| e["type"] == "directory").unwrap_or(entries.len());
    entries[..end]
        .iter()
        .filter(|e| e["type"] == "document")
        .filter_map(|e| e["info"]["shell"].as_str())
        .next_back()
        .unwrap_or_default()
        .to_owned()
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
fn set_shell(s: &mut dyn SettingsHost, key: &ServerKey, value: &str) {
    let st = s.settings_pages_mut().servers.shells.entry(key.clone()).or_default();
    let previous = std::mem::replace(&mut st.current, value.to_owned());
    s.spawn_job(SettingsJob::ForServer {
        server: key.clone(),
        request: SettingsRequest::SetServerShell {
            key: key.clone(),
            value: value.to_owned(),
            previous,
        },
    });
}

/// Desktop's `ServerHealthIndicator` drawn for `health`.
pub fn status_indicator(p: &Pal, health: Option<&Health>) -> El {
    status_mark(p, indicator(health))
}

/// The health mark itself: 6 wide and 14 tall, or 16 for incompatible warning.
pub fn status_mark(p: &Pal, ind: Indicator) -> El {
    let dot = |color| el().size(6.0, 6.0).my(3.5).radius(3.0).fill(color);
    let slot = || row().w(6.0).h(14.0).center().role("status");
    match ind {
        Indicator::Healthy => dot(widgets::status_ink(p, "connected")),
        Indicator::Unreachable => dot(widgets::status_ink(p, "failed")),
        Indicator::Unknown => dot(p.icon_faint),
        Indicator::Auth => slot().label_static(t!("server.status.authentication")).child(icon(&ic::LOCK, 12.0).color(p.icon_muted)),
        Indicator::Connecting => slot().label_static(t!("server.status.connecting")).child(rune_kit::Loader::new().size(12.0).build(kit(p))),
        Indicator::Incompatible => icon(&ic::WARNING, 16.0).color(p.v1_icon_warning),
    }
}

fn handle_add_pick(s: &mut dyn SettingsHost, picked: Option<&str>, tx: &mut Tx) {
    match picked {
        Some("add") => s.open_server_dialog(None, tx),
        Some(key) => s.run_server_add_menu_item(key, tx),
        None => {}
    }
}

pub fn view<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal) -> El {
    let servers = inventory::list(s);
    let open = s.popup().open_at("settings-add-server");
    let add = ghost_menu_button(p, "settings-add-server", t!("dialog.server.add.button"), Some(&ic::PLUS), true, true, open, move |s: &mut H, tx: &mut Tx| {
        let items = s.server_add_menu_items();
        if items.is_empty() {
            return s.open_server_dialog(None, tx);
        }
        let menu = Menu::new(t!("dialog.server.add.button"))
            .align(Align::End)
            .gap(4.0)
            .min_width(160.0)
            .item(Item::new("add", t!("dialog.server.add.button")))
            .items(items.into_iter().map(|(key, title)| Item::new(key, title)));
        popup::open(s, menu, "settings-add-server", tx, |s: &mut H, picked, tx| handle_add_pick(s, picked, tx));
    });
    let single = servers.len() <= 1;
    let title = if single { t!("settings.section.server") } else { t!("settings.tab.servers") };
    let selected = &s.settings_pages().server.key;
    let mut body = col().w_full().gap(32.0);
    if single {
        if let Some(own) = servers.first() {
            body = body.child(super::section(p, "settings-server-connection", Some(t!("settings.server.section.connection")), card(p, vec![connection_row(s, p, own, false)])));
            if own.availability.pages_connection().is_some() {
                body = body.child(super::section(
                    p,
                    "settings-server-preferences",
                    Some(t!("settings.tab.preferences")),
                    list(p, "settings-server-preferences-list", shell_row(s, p, own, "settings-shell")),
                ));
            }
        }
    } else {
        for server in &servers {
            let ready = server.availability.pages_connection().is_some();
            let row = connection_row(s, p, server, ready);
            let row = if ready {
                let target = if server.key == *selected { "settings-shell".to_owned() } else { format!("settings-shell:{}", server.key) };
                super::revealable(s, p, &target, 16.0, row)
            } else {
                row
            };
            body = body.child(col().key(format!("settings-server-{}", server.key)).role("region").label(server.name.clone()).w_full().child(card(p, vec![row])));
        }
    }
    body = body.children(s.plugin_sections_for_page(p, "servers"));
    page_with(p, "settings-servers-page", header_with(p, title, t!("settings.servers.description"), Some(add)), body, true)
}

fn shell_row<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal, server: &Server, target: &str) -> Vec<El> {
    let st = s.settings_pages().servers.shells.get(&server.key).cloned().unwrap_or_default();
    let key = server.key.clone();
    let anchor = format!("settings-shell-select-{}", server.key);
    let control = select_for(s, p, &anchor, t!("settings.general.row.shell.title"), options(&st), &st.current, move |s, value| set_shell(s, &key, value));
    super::rows(s, p, 20.0, vec![(target, setting(p, target, t!("settings.general.row.shell.title"), t!("settings.general.row.shell.description"), control))])
}

fn select_for<H: SettingsHost + PopupHost + 'static>(
    s: &H,
    p: &Pal,
    key: &str,
    label: &str,
    options: Vec<(String, String)>,
    current: &str,
    pick: impl Fn(&mut dyn SettingsHost, &str) + Clone + 'static,
) -> El {
    let shown = options.iter().find(|(id, _)| id == current).map_or_else(String::new, |(_, l)| l.clone());
    let anchor = format!("{key}-trigger");
    let (label, current) = (label.to_owned(), current.to_owned());
    let open = s.popup().open_at(&anchor);
    rune_kit::Select::new(key.to_owned(), &[shown.as_str()])
        .current(Some(&shown))
        .label(label.clone())
        .open(open)
        .trigger(kit(p))
        .menu_button(anchor.clone(), move |s: &mut H, tx: &mut Tx| {
            let menu = Menu::picker(label.clone(), options.clone(), &current);
            let pick = pick.clone();
            popup::open(s, menu, anchor.clone(), tx, move |s: &mut H, picked, tx| {
                if let Some(id) = picked {
                    tx.instant();
                    pick(s, id);
                }
            });
        })
}

fn card(p: &Pal, rows: Vec<El>) -> El {
    let n = rows.len();
    rows.into_iter().enumerate().fold(
        col().key("settings-server-list").w_full().px(16.0).radius(8.0).fill(p.layer1).box_shadow(Pal::ring_in(p.border_muted)),
        |l, (at, r)| if at + 1 < n { l.child(r.pb(16.0)).child(rule(0.5, p.border)).child(el().h(16.0)) } else { l.child(r) },
    )
}

fn connection_row<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal, server: &Server, shell_menu: bool) -> El {
    let shell_key = shell_menu.then(|| server.key.clone());
    match &server.kind {
        Kind::Plugin(rec) => {
            let drawn = s.plugin_server_row(p, server.key.as_str()).unwrap_or_else(|| text(rec.name.clone()).font(BODY_MED.lh(16.0)).color(p.text));
            col().key(format!("settings-server-row-{}", server.key)).w_full().py(20.0).child(drawn)
        }
        Kind::Http(http) => {
            let key = ServerConn::http_server(&http.url).map_or_else(|| ServerKey::new(&http.url), |c| c.key());
            let health = s.server_health(&key);
            let open = s.popup().open_at(&format!("settings-server-menu-{}", http.url));
            col().key(format!("settings-server-row-{}", server.key)).w_full().py(20.0).child(http_row_with::<H>(p, http, health.as_ref(), shell_key, open))
        }
        Kind::Own { .. } => own_row::<H>(s, p, server, shell_key),
    }
}

/// Radio-style Terminal shell submenu (`packages/app/src/servers/registry/row-menu.tsx` `ShellSubmenu`).
fn shell_submenu(opts: &[(String, String)], current: &str) -> Menu {
    let items = opts.iter().enumerate().map(|(i, (val, label))| {
        Item::new(format!("shell:{i}"), label.clone()).checked(val == current)
    });
    Menu::new(t!("settings.general.row.shell.title"))
        .min_width(172.0)
        .items(items)
        .hug()
}

fn handle_shell_pick(s: &mut dyn SettingsHost, picked: &str, shell_key: Option<&ServerKey>, opts: &[(String, String)], tx: &mut Tx) -> bool {
    let Some(idx) = picked.strip_prefix("shell:").and_then(|n| n.parse::<usize>().ok()) else {
        return false;
    };
    if let (Some(key), Some((value, _))) = (shell_key, opts.get(idx)) {
        tx.instant();
        set_shell(s, key, value);
    }
    true
}

fn handle_own_menu_pick(s: &mut dyn SettingsHost, shell_key: Option<&ServerKey>, opts: &[(String, String)], picked: Option<&str>, tx: &mut Tx) {
    match picked {
        Some(p) if handle_shell_pick(s, p, shell_key, opts, tx) => {}
        Some("sign-out") => s.page_sign_out(),
        _ => {}
    }
}

fn build_own_menu(s: &dyn SettingsHost, shell_key: Option<&ServerKey>) -> (Menu, Vec<(String, String)>) {
    let mut menu = Menu::new(t!("settings.section.server"))
        .align(Align::End)
        .gap(6.0)
        .group(t!("settings.section.server"))
        .item(Item::new("edit", t!("dialog.server.menu.edit")).enabled(false));
    let opts = if let Some(key) = shell_key {
        let st = s.settings_pages().servers.shells.get(key).cloned().unwrap_or_default();
        let opts = options(&st);
        menu = menu.submenu("settings-shell-submenu", t!("settings.general.row.shell.title"), shell_submenu(&opts, &st.current));
        opts
    } else {
        Vec::new()
    };
    let menu = if s.can_page_sign_out() { menu.separator().item(Item::new("sign-out", t!("rune.web.signOut"))) } else { menu };
    (menu, opts)
}

fn own_row<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal, server: &Server, shell_key: Option<ServerKey>) -> El {
    let url = server.address();
    let open = s.popup().open_at("settings-server-menu");
    let dots = row()
        .key("settings-server-menu")
        .role("button")
        .label_static(t!("common.moreOptions"))
        .size(20.0, 20.0)
        .center()
        .radius(4.0)
        .focus_default(p);
    let dots = if open { dots.fill(p.pressed) } else { dots.hover(p.hover) };
    let dots = dots
        .press(p.pressed)
        .menu_button("settings-server-menu", move |s: &mut H, tx: &mut Tx| {
            if let Some(key) = &shell_key
                && !s.settings_pages().servers.shells.contains_key(key)
            {
                s.spawn_job(SettingsJob::ForServer {
                    server: key.clone(),
                    request: SettingsRequest::LoadServerShells { key: key.clone() },
                });
            }
            let (menu, opts) = build_own_menu(s, shell_key.as_ref());
            let key = shell_key.clone();
            popup::open(s, menu, "settings-server-menu", tx, move |s: &mut H, picked, tx| {
                handle_own_menu_pick(s, key.as_ref(), &opts, picked, tx);
            });
        })
        .child(icon(&ic::OUTLINE_DOTS, 16.0).color(p.icon_muted));
    row()
        .key("settings-server-local")
        .w_full()
        .py(20.0)
        .gap(16.0)
        .center_y()
        .child(
            row()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .gap(10.0)
                .items_start()
                .child(status_indicator(p, server.health.as_ref()))
                .child(
                    col()
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0)
                        .gap(6.0)
                        .child(text(server.name.clone()).font(BODY_MED.lh(16.0)).color(p.text).truncate())
                        .children((!url.is_empty()).then(|| text(url).font(META).color(p.muted).truncate())),
                ),
        )
        .child(dots)
}

fn build_http_menu(s: &dyn SettingsHost, url: &str, shell_key: Option<&ServerKey>) -> (Menu, Vec<(String, String)>) {
    let mut menu = Menu::new(t!("settings.section.server"))
        .align(Align::End)
        .gap(6.0)
        .group(t!("settings.section.server"))
        .item(Item::new("edit", t!("dialog.server.menu.edit")));
    let opts = if let Some(key) = shell_key {
        let st = s.settings_pages().servers.shells.get(key).cloned().unwrap_or_default();
        let opts = options(&st);
        menu = menu.submenu("settings-shell-submenu", t!("settings.general.row.shell.title"), shell_submenu(&opts, &st.current));
        opts
    } else {
        Vec::new()
    };
    menu = menu.separator().item(Item::new("remove", t!("dialog.server.menu.remove")));
    let menu = if s.is_page_server(url) { menu.separator().item(Item::new("sign-out", t!("rune.web.signOut"))) } else { menu };
    (menu, opts)
}

fn handle_http_menu_pick(s: &mut dyn SettingsHost, url: &str, shell_key: Option<&ServerKey>, opts: &[(String, String)], picked: Option<&str>, tx: &mut Tx) {
    match picked {
        Some(p) if handle_shell_pick(s, p, shell_key, opts, tx) => {}
        Some("sign-out") => s.page_sign_out(),
        Some("edit") => s.open_server_dialog(Some(url), tx),
        Some("remove") => {
            tx.instant();
            s.remove_http_server(url, tx);
        }
        _ => {}
    }
}

fn open_edit_http(s: &mut dyn SettingsHost, url: &str, tx: &mut Tx) {
    s.open_server_dialog(Some(url), tx);
}

/// A server added by address (`ServerConnectionRow`'s own row), inside its card's padding.
#[allow(deprecated)]
pub fn http_row<H: SettingsHost + PopupHost + 'static>(p: &Pal, server: &HttpServer, health: Option<&Health>) -> El {
    http_row_with::<H>(p, server, health, None, false)
}

#[allow(deprecated)]
fn http_row_with<H: SettingsHost + PopupHost + 'static>(p: &Pal, server: &HttpServer, health: Option<&Health>, shell_key: Option<ServerKey>, open: bool) -> El {
    let anchor = format!("settings-server-menu-{}", server.url);
    let url = server.url.clone();
    let dots = row()
        .key(anchor.clone())
        .role("button")
        .label_static(t!("common.moreOptions"))
        .size(20.0, 20.0)
        .center()
        .radius(4.0)
        .focus_default(p);
    let dots = if open { dots.fill(p.pressed) } else { dots.hover(p.hover) };
    let dots = dots
        .press(p.pressed)
        .menu_button(anchor.clone(), move |s: &mut H, tx: &mut Tx| {
            if let Some(key) = &shell_key
                && !s.settings_pages().servers.shells.contains_key(key)
            {
                s.spawn_job(SettingsJob::ForServer {
                    server: key.clone(),
                    request: SettingsRequest::LoadServerShells { key: key.clone() },
                });
            }
            let (menu, opts) = build_http_menu(s, &url, shell_key.as_ref());
            let url = url.clone();
            let key = shell_key.clone();
            popup::open(s, menu, anchor.clone(), tx, move |s: &mut H, picked, tx| {
                handle_http_menu_pick(s, &url, key.as_ref(), &opts, picked, tx);
            });
        })
        .child(icon(&ic::OUTLINE_DOTS, 16.0).color(p.icon_muted));
    let authenticate = is_unauthorized(health).then(|| {
        let url = server.url.clone();
        widgets::button(t!("server.action.authenticate"), ButtonVariant::Neutral)
            .size(rune_kit::ButtonSize::Small)
            .on_press_tx(move |s: &mut H, tx: &mut Tx| open_edit_http(s, &url, tx))
            .build(kit(p))
            .key(format!("settings-server-authenticate-{}", server.url))
    });
    row()
        .w_full()
        .gap(16.0)
        .center_y()
        .child(
            row()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .gap(10.0)
                .items_start()
                .child(status_indicator(p, health))
                .child(
                    col()
                        .grow(1.0)
                        .shrink()
                        .min_w(0.0)
                        .gap(6.0)
                        .child(text(server.label()).font(BODY_MED.lh(16.0)).color(p.text).truncate())
                        .child(text(server.url.clone()).font(META).color(p.muted).truncate()),
                ),
        )
        .child(row().gap(8.0).center_y().children(authenticate).child(dots))
}

/// Desktop's `SettingsServerSwitcher` on a per-server page's header ([`switcher::view`]).
pub fn switcher<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal) -> Option<El> {
    switcher::view(s, p)
}

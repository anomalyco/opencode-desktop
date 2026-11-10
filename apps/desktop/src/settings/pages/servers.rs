//! Settings › Server (`settings/servers/servers.tsx`): the server's connection (its health,
//! name and address, and its row menu) and its preferences (Terminal shell, from the server's
//! shells, written to its global config). The servers plugins' server sources list (WSL, SSH)
//! join it, each row drawn by its plugin (`ServerEntry.row`), the page becoming Servers; Add
//! server offers the plugins' `server.add` items. [`switcher`] is Desktop's
//! `SettingsServerSwitcher` on the pages that show one server's settings.

use rune::{Align, Color, El, Tx, col, el, icon, row, text};
use rune_kit::ButtonVariant;
use rune_theme_opencode::icons as ic;
use serde_json::{Value, json};

use super::{ghost_menu_button, header_with, page_with, select};
use crate::App;
use crate::jobs::Apply;
use crate::palette::Pal;
use crate::popup::{self, Item, Menu};
use crate::settings::{self, setting};
use crate::widgets::BODY_MED;
use opencode_i18n::t;

const META: rune::TextStyle = rune::TextStyle::new(11.0, 440.0).lh(11.0);

/// What the page read from the server: its shells and the global config's choice.
#[derive(Default)]
pub struct State {
    /// `(path, name, acceptable)` (`ConfigShellOption`).
    pub shells: Vec<(String, String, bool)>,
    pub shell: String,
    /// The machine's name, for the local server's row (Desktop's `os.hostname()`).
    pub host: String,
    /// Whether a read answered, so its health shows.
    pub reached: Option<bool>,
    /// Add server or Edit server, while it's open.
    pub dialog: crate::servers::dialog::Dialog,
}

/// The servers listed: this window's, and each in the registry (plugin sources' and those added
/// by address).
pub fn count(s: &App) -> usize {
    1 + crate::plugin_host::contributed_servers(s).len()
}

/// The name this window's server goes by.
pub fn name(s: &App) -> String {
    let st = &s.settings_pages.servers;
    if !st.host.is_empty() {
        return st.host.clone();
    }
    match s.server_base().trim_start_matches("http://").trim_start_matches("https://") {
        "" => t!("rune.settings.server.local").to_owned(),
        base => base.to_owned(),
    }
}

/// Reads the shells and the config, and the machine's name.
pub fn load(s: &App) {
    if s.settings_pages.servers.host.is_empty() && s.live() && !cfg!(test) {
        let waker = s.waker.clone();
        let out = s.jobs_sender();
        std::thread::spawn(move || {
            let host = std::process::Command::new("hostname").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default();
            let _ = out.send(Box::new(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.settings_pages.servers.host = host;
            }) as Apply);
            if let Some(w) = waker {
                w();
            }
        });
    }
    s.job(|srv| {
        let shells = srv.get("/api/config/shell").ok();
        let config = srv.get("/api/config").ok();
        Box::new(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            crate::servers::refresh(s);
            let st = &mut s.settings_pages.servers;
            st.reached = Some(shells.is_some() || config.is_some());
            if let Some(list) = shells.as_ref().and_then(Value::as_array) {
                st.shells = list.iter().filter_map(|v| Some((v["path"].as_str()?.to_owned(), v["name"].as_str()?.to_owned(), v["acceptable"].as_bool().unwrap_or(true)))).collect();
            }
            if let Some(entries) = config.as_ref().and_then(Value::as_array) {
                st.shell = global_shell(entries);
            }
        }) as Apply
    });
}

/// The shell the global config sets: the last document's before the first folder's.
fn global_shell(entries: &[Value]) -> String {
    let end = entries.iter().position(|e| e["type"] == "directory").unwrap_or(entries.len());
    entries[..end].iter().filter(|e| e["type"] == "document").filter_map(|e| e["info"]["shell"].as_str()).last().unwrap_or_default().to_owned()
}

/// `createShellOptions`: Auto first, then each shell by its name (its path where two share one),
/// "(terminal only)" after one the agent can't use, and the configured one when unlisted.
fn options(st: &State) -> Vec<(String, String)> {
    let mut out = vec![(String::new(), t!("settings.general.row.shell.autoDefault").to_owned())];
    for (path, name, acceptable) in &st.shells {
        let ambiguous = st.shells.iter().filter(|(_, n, _)| n == name).count() > 1;
        let value = if ambiguous { path.clone() } else { name.clone() };
        let shown = if ambiguous { path.clone() } else { name.clone() };
        out.push((value, if *acceptable { shown } else { format!("{shown} ({})", t!("settings.general.row.shell.terminalOnly")) }));
    }
    if !st.shell.is_empty() && !out.iter().any(|(v, _)| *v == st.shell) {
        out.push((st.shell.clone(), st.shell.clone()));
    }
    out
}

fn set_shell(s: &mut App, value: &str, _: &mut Tx) {
    let previous = std::mem::replace(&mut s.settings_pages.servers.shell, value.to_owned());
    let value = value.to_owned();
    s.job(move |srv| {
        let body = json!({ "shell": if value.is_empty() { Value::Null } else { Value::String(value) } });
        let failed = srv.write("PATCH", "/api/experimental/config", Some(body)).err();
        Box::new(move |s: &mut App, tx: &mut Tx| {
            if let Some(e) = failed {
                tx.instant();
                s.settings_pages.servers.shell = previous;
                crate::status_toast::say(s, t!("common.requestFailed"), &e);
            }
        }) as Apply
    });
}

pub fn view(s: &App, p: &Pal) -> El {
    let st = &s.settings_pages.servers;
    let plugin_servers = crate::plugin_host::contributed_servers(s);
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
    // `ServerHealthIndicator`'s dot: `bg-icon-success-base`, `bg-icon-critical-base`.
    let token = |name: &str, fallback| session_view::palette::token(p.dark, name).unwrap_or(fallback);
    let health = if !s.live() { p.icon_faint } else if st.reached == Some(false) { token("icon-critical-base", p.danger) } else { token("icon-success-base", p.success) };
    let own = connection_row(p, "settings-server-local", health, &name(s), &own_address(s));
    let shell = super::rows(s, p, 20.0, vec![("settings-shell", setting(p, "settings-shell", t!("settings.general.row.shell.title"), t!("settings.general.row.shell.description"), select(s, p, "settings-shell-select", t!("settings.general.row.shell.title"), options(st), &st.shell, set_shell)))]);
    let title = if plugin_servers.is_empty() { t!("settings.section.server") } else { t!("settings.tab.servers") };
    let mut body = col().w_full().gap(32.0);
    if plugin_servers.is_empty() {
        body = body
            .child(super::section(p, "settings-server-connection", Some(t!("settings.server.section.connection")), connection_list(p, vec![own])))
            .child(super::section(p, "settings-server-preferences", Some(t!("settings.tab.preferences")), settings::list(p, "settings-server-preferences-list", shell)));
    } else {
        body = body.child(super::section(p, "settings-server-connection", None, connection_list(p, vec![own]).child(el().w_full().h(0.5).fill(p.border)).children(shell)));
        for c in &plugin_servers {
            let drawn = crate::plugin_host::server_row(s, p, c).unwrap_or_else(|| text(c.rec.name.clone()).font(BODY_MED.lh(16.0)).color(p.text));
            body = body.child(super::section(p, &format!("settings-server-{}", c.key), None, connection_list(p, vec![col().key(format!("settings-server-row-{}", c.key)).w_full().py(20.0).child(drawn)])));
        }
    }
    body = body.children(crate::plugin_host::settings_pages(s, p, "servers"));
    page_with(p, "settings-servers-page", header_with(p, title, t!("settings.servers.description"), Some(add)), body, true)
}

/// `settings-list` as a server card has it: 16 pt in.
fn connection_list(p: &Pal, rows: Vec<El>) -> El {
    rows.into_iter().fold(col().key("settings-server-list").w_full().px(16.0).radius(8.0).fill(p.layer1).box_shadow(Pal::ring_in(p.border_muted)), |l, r| l.child(r))
}

/// `.settings-servers-row`: the health dot, the name over the address, the ⋯ menu.
fn connection_row(p: &Pal, key: &str, health: Color, name: &str, url: &str) -> El {
    let dots = row().key("settings-server-menu").role("button").label(t!("common.moreOptions")).size(24.0, 24.0).center().radius(4.0)
        .hover(p.hover).press(p.pressed)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            let url = own_address(s);
            let menu = Menu::new(t!("settings.section.server")).align(Align::End).group(t!("settings.section.server"))
                .item(Item::new("edit", t!("dialog.server.menu.edit")).enabled(false))
                .item(Item::new("copy", t!("rune.settings.server.copyAddress")).enabled(!url.is_empty()))
                .item(Item::new("remove", t!("dialog.server.menu.remove")).enabled(false));
            popup::open(s, menu, "settings-server-menu", tx, move |_: &mut App, picked, tx| {
                if picked == Some("copy") {
                    tx.copy(url.clone());
                }
            });
        })
        .child(icon(&ic::OUTLINE_DOTS, 16.0).color(p.icon_muted));
    row().key(key.to_owned()).w_full().py(20.0).gap(16.0).center_y()
        .child(row().grow(1.0).shrink().min_w(0.0).gap(10.0).items_start()
            .child(el().size(6.0, 6.0).my(3.5).radius(3.0).fill(health))
            .child(col().grow(1.0).shrink().min_w(0.0).gap(6.0)
                .child(text(name.to_owned()).font(BODY_MED.lh(16.0)).color(p.text).truncate())
                .children((!url.is_empty()).then(|| text(url.to_owned()).font(META).color(p.muted).truncate()))))
        .child(dots)
}

/// A server added by address (`ServerConnectionRow`'s own row), inside its section's padding: its
/// health (a lock once it rejects the saved sign-in, with Authenticate), its name over its
/// address, and its menu (Edit, Remove).
#[allow(deprecated)]
pub(crate) fn http_row(p: &Pal, server: &crate::servers::Server) -> El {
    let health = crate::servers::health(&server.url);
    let token = |name: &str, fallback| session_view::palette::token(p.dark, name).unwrap_or(fallback);
    let indicator = match health {
        Some(crate::servers::Health::Unauthorized) => row().w(6.0).h(14.0).center().role("status").label(t!("server.status.authentication")).child(icon(crate::plugin_host::icons::resolve("", "lock"), 12.0).color(p.icon_muted)),
        Some(crate::servers::Health::Healthy) => el().size(6.0, 6.0).my(3.5).radius(3.0).fill(token("icon-success-base", p.success)),
        Some(crate::servers::Health::Unreachable) => el().size(6.0, 6.0).my(3.5).radius(3.0).fill(token("icon-critical-base", p.danger)),
        None => el().size(6.0, 6.0).my(3.5).radius(3.0).fill(p.icon_faint),
    };
    let anchor = format!("settings-server-menu-{}", server.url);
    let url = server.url.clone();
    let dots = row().key(anchor.clone()).role("button").label(t!("common.moreOptions")).size(24.0, 24.0).center().radius(4.0)
        .hover(p.hover).press(p.pressed)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            let menu = Menu::new(t!("settings.section.server")).align(Align::End).group(t!("settings.section.server"))
                .item(Item::new("edit", t!("dialog.server.menu.edit")))
                .separator()
                .item(Item::new("remove", t!("dialog.server.menu.remove")));
            let url = url.clone();
            popup::open(s, menu, anchor.clone(), tx, move |s: &mut App, picked, tx| match picked {
                Some("edit") => crate::servers::dialog::open(s, Some(&url), tx),
                Some("remove") => {
                    tx.instant();
                    crate::servers::remove(&url);
                    crate::servers::dialog::sync(s, tx);
                }
                _ => {}
            });
        })
        .child(icon(&ic::OUTLINE_DOTS, 16.0).color(p.icon_muted));
    let authenticate = (health == Some(crate::servers::Health::Unauthorized)).then(|| {
        let url = server.url.clone();
        crate::widgets::button(t!("server.action.authenticate"), ButtonVariant::Neutral).size(rune_kit::ButtonSize::Small)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| crate::servers::dialog::open(s, Some(&url), tx))
            .build(crate::palette::kit(p))
            .key(format!("settings-server-authenticate-{}", server.url))
    });
    row().w_full().gap(16.0).center_y()
        .child(row().grow(1.0).shrink().min_w(0.0).gap(10.0).items_start()
            .child(indicator)
            .child(col().grow(1.0).shrink().min_w(0.0).gap(6.0)
                .child(text(server.label()).font(BODY_MED.lh(16.0)).color(p.text).truncate())
                .child(text(server.url.clone()).font(META).color(p.muted).truncate())))
        .child(row().gap(8.0).center_y().children(authenticate).child(dots))
}

/// The app's own server's address, also while the window works with a contributed one.
fn own_address(s: &App) -> String {
    crate::servers::runtime::own_server().map_or_else(|| s.server_base().to_owned(), |own| own.base)
}

/// Desktop's `SettingsServerSwitcher` on the pages that show one server's settings (Providers,
/// Models, Extensions, Projects, Worktrees): "Server name", choosing which server the window
/// works with. Nothing while no other server is listed.
#[allow(deprecated)]
pub fn switcher(s: &App, p: &Pal) -> Option<El> {
    let listed: Vec<_> = crate::plugin_host::contributed_servers(s).into_iter().filter(|c| c.rec.listed).collect();
    if listed.is_empty() {
        return None;
    }
    let current = match &s.server_key() {
        Some(key) => listed.iter().find(|c| &c.key == key).map_or_else(|| key.clone(), |c| c.rec.name.clone()),
        None => name(s),
    };
    let ink = p.muted;
    let dot = el().size(6.0, 6.0).radius(3.0).fill(p.success);
    // `size="small"`: 24 tall, as rune-kit's older `Small` keeps it.
    let trigger = crate::widgets::button(format!("{} {current}", t!("settings.server.switcher.label")), ButtonVariant::Outline)
        .size(rune_kit::ButtonSize::Small)
        .label_with(move |font, color| {
            row().gap(6.0).center_y().child(el().size(16.0, 16.0).center().child(dot)).child(text(t!("settings.server.switcher.label")).font(font).color(ink)).child(text(current).font(font).color(color).truncate()).child(icon(&ic::CHEVRON_DOWN, 16.0).color(ink))
        })
        .on_press_tx(|s: &mut App, tx: &mut Tx| open_switcher(s, tx))
        .build(crate::palette::kit(p))
        .key("settings-server-switcher");
    Some(trigger)
}

fn open_switcher(s: &mut App, tx: &mut Tx) {
    let listed: Vec<_> = crate::plugin_host::contributed_servers(s).into_iter().filter(|c| c.rec.listed).collect();
    let current = s.server_key().unwrap_or_default();
    let mut menu = Menu::new(t!("settings.server.switcher.label")).item(Item::new("", name(s)).checked(current.is_empty()));
    for c in &listed {
        menu = menu.item(Item::new(c.key.clone(), c.rec.name.clone()).checked(current == c.key));
    }
    // `.settings-server-switcher-menu`: 220 wide at least.
    let menu = menu.separator().item(Item::new("manage", t!("settings.server.switcher.manage"))).align(Align::End).min_width(220.0);
    popup::open(s, menu, "settings-server-switcher", tx, |s: &mut App, picked: Option<&str>, tx: &mut Tx| match picked {
        None => {}
        Some("manage") => s.open_settings_page(super::Page::Servers, tx),
        Some("") => s.focus_server(&opencode_kit::servers::ServerKey::sidecar(), tx),
        Some(key) => s.focus_server(&opencode_kit::servers::ServerKey::new(key), tx),
    });
}
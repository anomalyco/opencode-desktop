//! Settings › Server (`settings/servers/servers.tsx`): the server's connection (its health,
//! name and address, and its row menu) and its preferences (Terminal shell, from the server's
//! shells, written to its global config). The servers plugins' server sources list (WSL, SSH)
//! join it as rows of their own, the page becoming Servers; Add server offers the plugins'
//! `server.add` items.

use rune::{Color, El, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;
use serde_json::{Value, json};

use super::{ghost_button, header_with, page_with, select};
use crate::App;
use crate::jobs::Apply;
use crate::palette::Pal;
use crate::popup::{self, Item, Menu};
use crate::settings::{self, setting};
use crate::widgets::BODY_MED;

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
}

/// The servers listed: this window's, and each plugin source's.
pub fn count(s: &App) -> usize {
    1 + crate::plugin_host::servers(s).len()
}

/// The name this window's server goes by.
pub fn name(s: &App) -> String {
    let st = &s.settings_pages.servers;
    if !st.host.is_empty() {
        return st.host.clone();
    }
    match s.server_base().trim_start_matches("http://").trim_start_matches("https://") {
        "" => "Local server".to_owned(),
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
    let mut out = vec![(String::new(), "Auto (Default)".to_owned())];
    for (path, name, acceptable) in &st.shells {
        let ambiguous = st.shells.iter().filter(|(_, n, _)| n == name).count() > 1;
        let value = if ambiguous { path.clone() } else { name.clone() };
        let shown = if ambiguous { path.clone() } else { name.clone() };
        out.push((value, if *acceptable { shown } else { format!("{shown} (terminal only)") }));
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
                crate::status_toast::say(s, "Request failed", &e);
            }
        }) as Apply
    });
}

pub fn view(s: &App, p: &Pal) -> El {
    let st = &s.settings_pages.servers;
    let plugin_servers = crate::plugin_host::servers(s);
    let add_items = crate::plugin_host::server_menu_items(s, "server.add");
    let add = ghost_button(p, "settings-add-server", "Add server", Some(&ic::PLUS), true, !add_items.is_empty(), |s, tx| {
        let items = crate::plugin_host::server_menu_items(s, "server.add");
        let menu = Menu::new("Add server").align(rune::Align::End).items(items.into_iter().map(|(key, title)| Item::new(key, title)));
        popup::open(s, menu, "settings-add-server", tx, |s: &mut App, picked, tx| {
            if let Some(key) = picked {
                crate::plugin_host::run_menu_item(s, key, "", tx);
            }
        });
    });
    let health = if !s.live() { p.icon_faint } else if st.reached == Some(false) { p.danger } else { p.success };
    let own = connection_row(p, "settings-server-local", health, &name(s), s.server_base());
    let shell = super::rows(s, p, 20.0, vec![("settings-shell", setting(p, "settings-shell", "Terminal shell", "Shell used by the terminal and agent tools", select(s, p, "settings-shell-select", "Terminal shell", options(st), &st.shell, set_shell)))]);
    let title = if plugin_servers.is_empty() { "Server" } else { "Servers" };
    let mut body = col().w_full().gap(32.0);
    if plugin_servers.is_empty() {
        body = body
            .child(super::section(p, "settings-server-connection", Some("Connection"), connection_list(p, vec![own])))
            .child(super::section(p, "settings-server-preferences", Some("Preferences"), settings::list(p, "settings-server-preferences-list", shell)));
    } else {
        body = body.child(super::section(p, "settings-server-connection", None, connection_list(p, vec![own]).child(el().w_full().h(0.5).fill(p.border)).children(shell)));
        for srv in &plugin_servers {
            let ink = match srv.state {
                3 => p.success,
                4 => p.danger,
                _ => p.icon_faint,
            };
            body = body.child(super::section(p, &format!("settings-server-{}", srv.id), None, connection_list(p, vec![connection_row(p, &format!("settings-server-row-{}", srv.id), ink, &srv.name, &srv.url)])));
        }
    }
    body = body.children(crate::plugin_host::settings_pages(s, p, "servers"));
    page_with(p, "settings-servers-page", header_with(p, title, "Manage server connections", Some(add)), body, true)
}

/// `settings-list` as a server card has it: 16 pt in.
fn connection_list(p: &Pal, rows: Vec<El>) -> El {
    rows.into_iter().fold(col().key("settings-server-list").w_full().px(16.0).radius(8.0).fill(p.layer1).box_shadow(Pal::ring_in(p.border_muted)), |l, r| l.child(r))
}

/// `.settings-servers-row`: the health dot, the name over the address, the ⋯ menu.
fn connection_row(p: &Pal, key: &str, health: Color, name: &str, url: &str) -> El {
    let dots = row().key("settings-server-menu").role("button").label("More options").size(24.0, 24.0).center().radius(4.0)
        .hover(p.hover).press(p.pressed)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            let url = s.server_base().to_owned();
            let menu = Menu::new("Server").align(rune::Align::End).group("Server")
                .item(Item::new("edit", "Edit").enabled(false))
                .item(Item::new("copy", "Copy address").enabled(!url.is_empty()))
                .item(Item::new("remove", "Remove").enabled(false));
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

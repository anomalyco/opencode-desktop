use super::*;
use crate::popup::MenuButton as _;
use crate::popup::{self, Item, Menu};
use crate::projects::home::{self as home_state, ServerView};
use opencode_i18n::t;
use opencode_kit::servers::ServerKey;
use opencode_servers::ServerConn;
use opencode_servers::health::Indicator;

// ---------------------------------------------------------------------------------------------
// Home's projects grouped by server (`home/projects/view.tsx:276-511`, `view.css`,
// `servers/registry/row.tsx`): once more than one server is listed, or one asks for a sign-in,
// each server's row (its collapse chevron, status mark, name and badge, and its More options and
// Add project at the end), then its sign-in button or its projects.

/// The chevron's turn (`transition-transform duration-150 ease-in-out`).
const TURN: Curve = Curve::tween(0.15, Easing::cubic(0.42, 0.0, 0.58, 1.0));

/// Home groups its projects by server: more than one is listed, or one is signing in
/// (`view.tsx:247-251`).
pub(super) fn grouped(s: &App) -> bool {
    let servers = &s.kernel.slots.get(&crate::projects::HOME_STATE).servers;
    servers.len() > 1 || servers.iter().any(|v| v.sign_in_required() || v.connecting())
}

/// The groups, `gap-4` in the column (`gap-1` in the narrow dropdown); each `gap-1`, its row
/// then, past a hairline, its sign-in button or its projects. `width` is a row's.
pub(super) fn groups(s: &App, p: &Pal, dropdown: bool, width: f32) -> El {
    let sel = home_state::selection(s);
    let mut list = col().key("home-servers").w_full().gap(if dropdown { 4.0 } else { 16.0 });
    for v in &s.kernel.slots.get(&crate::projects::HOME_STATE).servers {
        let collapsed = home_state::collapsed(&v.key);
        let selected = sel.server.as_ref() == Some(&v.key) && sel.directory.is_none();
        let mut group = col().key(format!("home-server-{}", v.key)).w_full().gap(4.0).child(server_row(s, p, v, selected, collapsed));
        let signing = v.sign_in_required() || v.connecting();
        let projects = projects_of(s, v);
        if signing {
            group = group.child(hairline(p)).child(row().key("authenticate").w_full().px(6.0).py(4.0).child(authenticate_button(p, v)));
        } else if v.healthy() && !projects.is_empty() && !collapsed {
            let mut rows = col().key("projects").w_full().gap(4.0);
            for proj in &projects {
                let active = sel.server.as_ref() == Some(&v.key) && sel.directory.as_deref().is_some_and(|d| same_dir(d, &proj.directory));
                rows = rows.child(super::projects::project_row(s, p, &v.key, proj, active, width));
            }
            group = group.child(hairline(p)).child(rows);
        }
        list = list.child(group);
    }
    list
}

/// A server's projects: the one the window works against read now, the others as of the poll.
fn projects_of(s: &App, v: &ServerView) -> Vec<model::Project> {
    if v.key == *crate::servers::runtime::active(s) {
        return s.used_projects().into_iter().cloned().collect();
    }
    v.projects.clone()
}

fn same_dir(a: &str, b: &str) -> bool {
    opencode_servers::home::same_directory_key(a) == opencode_servers::home::same_directory_key(b)
}

/// `mx-3 h-px bg-v2-border-border-base`.
fn hairline(p: &Pal) -> El {
    el().key("hairline").mx(12.0).h(1.0).fill(p.border)
}

/// `HomeServerRow`: 28 tall, the row's surface (`view.css`: bg-base, layer-01 hovered or while
/// its menu is open, layer-03 selected, at 60% while the server is down), the nav button with
/// its chevron, status and name, and its actions at the end, revealed on hover.
fn server_row(s: &App, p: &Pal, v: &ServerView, selected: bool, collapsed: bool) -> El {
    let key = v.key.clone();
    let menu = menu_key(&key);
    let menu_open = s.popup.open_at(&menu);
    let healthy = v.healthy();
    let auth = v.sign_in_required();
    let incompatible = v.incompatible();
    let dimmed = !healthy && !incompatible;
    let can_toggle = healthy && !projects_of(s, v).is_empty();
    let (rest, hovered) = if selected { (p.layer3, p.layer3) } else { (p.bg_base, p.layer1) };
    let surface = |c: Color| if dimmed { c.alpha(c.a * 0.6) } else { c };
    let shown = if menu_open { hovered } else { rest };

    let chevron_key = key.clone();
    let chevron = el()
        .key("collapse")
        .role("button")
        .label(if collapsed { t!("home.server.expand") } else { t!("home.server.collapse") })
        .size(20.0, 20.0)
        .ml(-2.0)
        .mr(-6.0)
        .radius(4.0)
        .center()
        .opacity(if can_toggle { 1.0 } else { 0.4 })
        .child(icon(&ic::CHEVRON_DOWN, 14.0).color(p.icon_muted).rotate(if collapsed || !can_toggle { -std::f32::consts::FRAC_PI_2 } else { 0.0 }).mirror_in_rtl().rotate_motion(TURN));
    let chevron = if can_toggle {
        chevron.hover(p.hover).expanded(!collapsed).on_press_tx(move |_: &mut App, tx: &mut Tx| home_state::toggle_collapsed(&chevron_key, tx))
    } else {
        chevron.disabled(true).on_press(|_: &mut App| {})
    };
    let mut label = row()
        .key("label")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(4.0)
        .center_y()
        .opacity(if dimmed { 0.6 } else { 1.0 })
        .child(text(v.label.clone()).key("name").font(UI).color(if selected { p.text } else { p.muted }).group_color(p.text).shrink().min_w(0.0).truncate().dir(rune::Direction::Auto));
    if let Some(badge) = v.conn.label() {
        // `rounded-[3px] border px-1 py-0.5 text-[9px] leading-none text-muted`.
        label = label.child(
            row().key("badge").px(4.0).py(2.0).radius(3.0).box_shadow(Pal::ring_in(p.border)).child(text(badge.to_owned()).font(rune::TextStyle::new(9.0, 440.0).lh(9.0)).color(p.muted)),
        );
    }
    let focus_key = key.clone();
    let nav = row()
        .key("nav")
        .role("button")
        .label(v.label.clone())
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .h(28.0)
        .px(6.0)
        .gap(8.0)
        .center_y()
        .radius(6.0).focus_inset(p)
        .hover_group()
        .child(chevron)
        .child(el().key("status").size(16.0, 16.0).mr(-2.0).center().child(indicator(p, crate::servers::health::indicator(&v.key))))
        .child(label);
    let nav = if healthy || auth { nav.on_press_tx(move |s: &mut App, tx: &mut Tx| focus(s, &focus_key, tx)) } else { nav };

    // `pl-2`, two 20 pt buttons `gap-1`, `right-1`: the fade runs over the first 8.
    let actions_w = 8.0 + 20.0 + 4.0 + 20.0;
    let fade = surface(hovered);
    let (menu_target, add_target) = (key.clone(), key.clone());
    let add_enabled = !v.failed() || auth;
    let mut add = super::projects::icon_button(p, &format!("home-server-add-{}", v.key), t!("home.project.add"), &ic::FOLDER_ADD_LEFT, false).help(t!("home.project.add"));
    add = if add_enabled { add.on_press_tx(move |s: &mut App, tx: &mut Tx| choose_project(s, &add_target, tx)) } else { add.disabled(true).opacity(0.5) };
    let actions = row()
        .key("actions")
        // 2 clear of the row's top and bottom, where the row's inset focus ring runs.
        .pin(None, Some(2.0), Some(4.0), Some(2.0))
        .w(actions_w)
        .pl(8.0)
        .gap(4.0)
        .center_y()
        .linear_gradient(std::f32::consts::FRAC_PI_2, [(0.0, fade.alpha(0.0)), (8.0 / actions_w, fade), (1.0, fade)])
        .child(super::projects::icon_button(p, &menu, t!("common.moreOptions"), &ic::OUTLINE_DOTS, menu_open).menu_button(menu.clone(), move |s: &mut App, tx: &mut Tx| open_menu(s, &menu_target, tx)))
        .child(add);
    let mut r = row()
        .key(format!("hs-{}", v.key))
        .w_full()
        .h(28.0)
        .center_y()
        .radius(6.0)
        .hover_group()
        .fill(surface(shown))
        .hover(surface(hovered))
        .child(nav)
        .child(if menu_open || session_view::style::hover_none() { actions } else { actions.group_opacity(0.0, 1.0) });
    if auth {
        r = r.help(t!("server.status.authentication"));
    } else if incompatible {
        let version = opencode_servers::health::version(v.health.as_ref()).unwrap_or("1").to_owned();
        r = r.help(t!("server.row.incompatible", version = version));
    }
    r
}

/// `ServerHealthIndicator` (`servers/registry/row.tsx`): a 6 pt dot (success, critical or faint),
/// a small lock while it waits for a sign-in, a 12 pt loader while it connects, or the warning.
fn indicator(p: &Pal, mark: Indicator) -> El {
    let token = |name: &str, fallback: Color| session_view::palette::token(p.dark, name).unwrap_or(fallback);
    let dot = |c: Color| el().key("dot").size(6.0, 6.0).my(3.5).radius(3.0).fill(c);
    match mark {
        // Desktop's v1 `icon-success-base` and `icon-critical-base`, as the MCP dots take them.
        Indicator::Healthy => dot(crate::widgets::status_ink(p, "connected")),
        Indicator::Unreachable => dot(crate::widgets::status_ink(p, "failed")),
        Indicator::Unknown => dot(p.icon_faint),
        Indicator::Auth => row().key("lock").w(6.0).h(14.0).center().role("status").label_static(t!("server.status.authentication")).child(icon(&ic::LOCK, 14.0).color(p.icon_muted)),
        Indicator::Connecting => row().key("loader").w(6.0).h(14.0).center().role("status").label_static(t!("server.status.connecting")).child(session_view::style::activity_ring_sized(p, 12.0)),
        Indicator::Incompatible => icon(&ic::WARNING, 14.0).key("warning").color(token("icon-warning-base", p.v1_icon_warning)),
    }
}

/// `home-server-authenticate`: a small neutral button across the row (`Connecting…` with a
/// loader, disabled, while a plugin starts the server).
fn authenticate_button(p: &Pal, v: &ServerView) -> El {
    let connecting = v.connecting();
    let key = v.key.clone();
    let label = if connecting { t!("server.status.connecting") } else { t!("server.action.authenticate") };
    #[allow(deprecated)]
    let mut b = crate::widgets::button(label, rune_kit::ButtonVariant::Neutral).size(rune_kit::ButtonSize::Small);
    if connecting {
        let ring = session_view::style::activity_ring_sized(p, 14.0);
        b = b.disabled(true).label_with(move |font, color| row().gap(6.0).center_y().child(ring).child(text(label).font(font).color(color)));
    } else {
        b = b.on_press_tx(move |s: &mut App, tx: &mut Tx| {
            authenticate(s, &key, tx);
        });
    }
    b.build(crate::palette::kit(p)).key("home-server-authenticate").w_full()
}

fn menu_key(key: &ServerKey) -> String {
    format!("hs-menu-{key}")
}

/// A server row: Home focuses the server, all of its projects; one waiting for a sign-in asks
/// for it first (`server.focus`).
fn focus(s: &mut App, key: &ServerKey, tx: &mut Tx) {
    tx.instant();
    s.project_state.home_menu_open = false;
    if authenticate(s, key, tx) {
        return;
    }
    home_state::focus(s, key);
    super::picked(s, tx);
}

/// An added server that rejects its saved sign-in opens its Edit server dialog, for a new
/// password or pairing link (`actions.authenticate`). True when it asked.
fn authenticate(s: &mut App, key: &ServerKey, tx: &mut Tx) -> bool {
    let Some(v) = s.kernel.slots.get(&crate::projects::HOME_STATE).servers.iter().find(|v| v.key == *key) else { return false };
    if !v.sign_in_required() {
        return false;
    }
    tx.instant();
    s.project_state.home_menu_open = false;
    match &v.conn {
        ServerConn::Http { http, .. } => {
            let url = http.url.clone();
            crate::servers::dialog::open(s, Some(&url), tx);
        }
        // A plugin's server signs in through its plugin (`ServerConnection.authenticate`).
        ServerConn::Plugin { .. } => crate::plugin_host::connect_server(s, key.as_str(), tx),
        ServerConn::Sidecar { .. } => {}
    }
    true
}

/// Add project on `key`: this computer's folder dialog for the app's own server, the server's
/// own folders for another (`useDirectoryPicker`); the folders picked become its projects.
pub(super) fn choose_project(s: &mut App, key: &ServerKey, tx: &mut Tx) {
    s.project_state.home_menu_open = false;
    if authenticate(s, key, tx) {
        return;
    }
    if s.kernel.slots.get(&crate::projects::HOME_STATE).servers.iter().any(|v| v.key == *key && v.failed()) {
        return;
    }
    let server = key.clone();
    crate::projects::remote_picker::pick_with(s, key, t!("command.project.open").to_owned(), tx, move |h, dir, tx| {
        home_state::add(h, &server, vec![dir], tx);
    });
}

/// `ServerRowMenuView`: Edit (an added server's), Hide from project list (the app's own, while
/// another shows), then Remove (an added server's).
fn open_menu(s: &mut App, key: &ServerKey, tx: &mut Tx) {
    let Some(v) = s.kernel.slots.get(&crate::projects::HOME_STATE).servers.iter().find(|v| v.key == *key) else { return };
    let http = match &v.conn {
        ServerConn::Http { http, .. } => Some(http.url.clone()),
        _ => None,
    };
    let can_hide = v.conn.builtin() && s.kernel.slots.get(&crate::projects::HOME_STATE).servers.len() > 1;
    let mut menu = Menu::new(t!("settings.section.server"))
        .min_width(160.0)
        .align(Align::End)
        .gap(6.0)
        .group(t!("settings.section.server"))
        .item(Item::new("edit", t!("dialog.server.menu.edit")).enabled(http.is_some()));
    if can_hide {
        menu = menu.item(Item::new("hide", t!("dialog.server.menu.hide")));
    }
    if http.is_some() {
        menu = menu.separator().item(Item::new("remove", t!("dialog.server.menu.remove")));
    }
    tx.instant();
    let key = key.clone();
    popup::open_drawn(s, menu, menu_key(&key), tx, move |s: &mut App, picked, tx| match (picked, &http) {
        (Some("edit"), Some(url)) => crate::servers::dialog::open(s, Some(url), tx),
        (Some("hide"), _) => {
            tx.instant();
            home_state::set_hidden(&key, true);
        }
        (Some("remove"), Some(url)) => {
            tx.instant();
            crate::servers::remove_offering(s, url);
            crate::servers::dialog::sync(s, tx);
        }
        _ => {}
    });
}

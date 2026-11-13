//! Customize's View options (`SidebarViewMenu`, `sidebar-view-menu.tsx`): Group by, Sort by and
//! Show, then a menu per filter (Server only once there are two servers), then Reset filters and
//! Mark all as read.

use super::grouping::{Grouping, Ordering, Status, groupings};
use super::*;
use crate::popup::{self, Item, Menu};
use crate::settings::Choice;
use opencode_i18n::t;

/// The trigger's key, which the menu opens under.
pub(super) const TRIGGER: &str = "customize-options";

/// What the menu lists besides the view: the servers and projects to filter by, and whether a
/// visible row is unread.
pub(super) struct Options {
    pub servers: Vec<(String, String)>,
    pub projects: Vec<(String, String)>,
    pub can_mark_read: bool,
    /// The unread sessions Mark all as read clears, on the server the window works against.
    pub unread: Vec<String>,
}

pub(super) fn open(s: &mut App, o: Options, tx: &mut Tx) {
    let st = &s.project_state.customize;
    let multi = o.servers.len() > 1;
    let grouping = groupings(multi).fold(Menu::new(t!("sidebar.view.grouping")), |m, g| m.item(Item::new(format!("grouping:{}", g.name()), g.label()).checked(g == st.grouping)));
    let ordering = Ordering::ALL
        .iter()
        .fold(Menu::new(t!("sidebar.view.ordering")), |m, o| m.item(Item::new(format!("ordering:{}", o.name()), o.label()).checked(*o == st.ordering)))
        .separator()
        .item(Item::new("ordering-hint", t!("sidebar.view.ordering.hint")).detail());
    let show = Menu::new(t!("sidebar.view.show"))
        .item(Item::new("show:recent", t!("sidebar.view.show.recent")).checked(st.show.recent))
        .item(Item::new("show:subagents", t!("sidebar.view.show.subagents")).checked(st.show.subagents))
        .item(Item::new("show:serverLabels", t!("sidebar.view.show.serverLabels")).checked(st.show.server_labels));
    let f = &st.filters;
    let status = Status::ALL.iter().fold(Menu::new(t!("sidebar.view.filter.status")), |m, v| m.item(Item::new(format!("filter:status:{}", v.name()), v.label()).checked(f.status.contains(v))));
    let servers = o.servers.iter().fold(Menu::new(t!("sidebar.view.filter.server")), |m, (k, l)| m.item(Item::new(format!("filter:server:{k}"), l.clone()).checked(f.server.contains(k))));
    let projects = o
        .projects
        .iter()
        .cloned()
        .chain([("none".to_owned(), t!("sidebar.view.project.none").to_owned())])
        .fold(Menu::new(t!("sidebar.view.filter.project")), |m, (k, l)| {
            let on = f.project.contains(&k);
            m.item(Item::new(format!("filter:project:{k}"), l).checked(on))
        })
        .rows_before_scroll(10.0);
    let count = |n: usize, title: &str| if n > 0 { format!("{title} · {n}") } else { title.to_owned() };
    let mut menu = Menu::new(t!("sidebar.view.options"))
        .min_width(200.0)
        .gap(4.0)
        .submenu("grouping", format!("{} · {}", t!("sidebar.view.grouping"), st.grouping.label()), grouping)
        .submenu("ordering", format!("{} · {}", t!("sidebar.view.ordering"), st.ordering.label()), ordering)
        .submenu("show", t!("sidebar.view.show"), show)
        .separator()
        .submenu("filter-status", count(f.status.len(), t!("sidebar.view.filter.status")), status);
    if multi {
        menu = menu.submenu("filter-server", count(f.server.len(), t!("sidebar.view.filter.server")), servers);
    }
    menu = menu
        .submenu("filter-project", count(f.project.len(), t!("sidebar.view.filter.project")), projects)
        .separator()
        .item(Item::new("reset", t!("sidebar.view.resetFilters")).enabled(f.count() > 0))
        .item(Item::new("read", t!("sidebar.view.markAllRead")).enabled(o.can_mark_read));
    tx.instant();
    let unread = o.unread;
    popup::open_drawn(s, menu, TRIGGER.to_owned(), tx, move |s: &mut App, picked, tx| {
        tx.instant();
        if let Some(id) = picked {
            pick(s, id, &unread);
        }
    });
}

fn pick(s: &mut App, id: &str, unread: &[String]) {
    let st = &mut s.project_state.customize;
    let toggle = |list: &mut Vec<String>, v: &str| match list.iter().position(|x| x == v) {
        Some(i) => _ = list.remove(i),
        None => list.push(v.to_owned()),
    };
    match id.split_once(':') {
        Some(("grouping", v)) => st.grouping = Grouping::parse(Some(v)),
        Some(("ordering", v)) => st.ordering = Ordering::parse(Some(v)),
        Some(("show", "recent")) => st.show.recent = !st.show.recent,
        Some(("show", "subagents")) => st.show.subagents = !st.show.subagents,
        Some(("show", "serverLabels")) => st.show.server_labels = !st.show.server_labels,
        Some(("filter", rest)) => match rest.split_once(':') {
            Some(("status", v)) => {
                let v = Status::parse(Some(v));
                match st.filters.status.iter().position(|x| *x == v) {
                    Some(i) => _ = st.filters.status.remove(i),
                    None => st.filters.status.push(v),
                }
            }
            Some(("server", v)) => toggle(&mut st.filters.server, v),
            Some(("project", v)) => toggle(&mut st.filters.project, v),
            _ => {}
        },
        _ if id == "reset" => st.filters = Default::default(),
        _ if id == "read" => mark_read(s, unread),
        _ => {}
    }
}

/// Mark all as read: every listed unread session on the server the window works against.
pub(super) fn mark_read(s: &mut App, ids: &[String]) {
    for id in ids {
        let Some(info) = s.sessions.iter_mut().find(|i| i.id == *id) else { continue };
        info.viewed = info.idle.max(info.viewed);
        let idle = info.idle.unwrap_or(*s.now_ms);
        s.write(opencode_kit::client::Write::View { session: id.clone(), idle });
    }
}


//! The Customize sidebar (Settings › Experimental › Organize vertical sidebar, Customize;
//! `sidebar-customized.tsx`): a Sessions header with View options and New session, then the open
//! tabs and the server's sessions grouped, sorted and filtered (`grouping`), each group's tabs under
//! its header and its other sessions after them, five at a time (ten in one ungrouped list).
//!
//! The tabs are every server's; the sessions that aren't open are those of the server the window
//! works against.

use std::collections::{HashMap, HashSet};

use crate::popup::MenuButton as _;

use super::grouping::{self as m, Grouping, Group, Kind, Row, Status, ViewState};
use super::rows::{HEADER, Lead, SessionRow, ghost_button, group_header, session_row};
use super::*;
use crate::view::tab_strip::Sections;
use opencode_i18n::t;
use rune_kit::shell::TabBreak;

/// Rows a group's footer shows before Show more, and in one ungrouped list (`RECENT_STEP`,
/// `RECENT_STEP_UNGROUPED`).
const STEP: usize = 5;
const STEP_UNGROUPED: usize = 10;

/// What a row is beyond the view's fields: its tab, its session and its project.
struct Item<'a> {
    tab: Option<usize>,
    server: opencode_kit::servers::ServerKey,
    session: Option<&'a SessionInfo>,
    project: Option<&'a model::Project>,
}

/// The rows (`rows` memo, `sidebar-customized.tsx:178-286`): the open tabs in their order, then
/// the server's root sessions (and its subagents, when shown) that no tab holds.
fn gather(s: &App, show_subagents: bool) -> (Vec<Row>, Vec<Item<'_>>) {
    let current = if s.settings_open() { None } else { s.current_tab() };
    let here = crate::servers::runtime::active(s).clone();
    let mut rows = vec![];
    let mut items = vec![];
    for (order, t) in s.tabs.iter().enumerate() {
        let uid = t.uid();
        let face = crate::view::tab_strip::face(s, t);
        let session = (t.server == here && !t.is_draft()).then(|| s.session(&t.id)).flatten();
        let key = if t.is_draft() { uid.clone() } else { super::state::key(t.server.as_str(), &t.id) };
        rows.push(Row {
            key,
            server: t.server.as_str().to_owned(),
            project: face.project.map(|p| p.id.clone()),
            open: true,
            active: current.as_deref() == Some(uid.as_str()),
            parent: None,
            status: if t.is_draft() { Status::Idle } else { m::status_of(s.tab_status_of(t)) },
            updated: session.map_or(0, |i| s.recent_page.active_ms(i)),
            created: session.map_or(0, |i| i.created),
            order,
        });
        items.push(Item { tab: Some(order), server: t.server.clone(), session, project: face.project });
    }
    let open: HashSet<String> = rows.iter().map(|r| r.key.clone()).collect();
    for info in s.sessions.iter().filter(|i| i.archived.is_none() && !crate::view::is_automation(&i.title)) {
        if info.parent.is_some() && !show_subagents {
            continue;
        }
        let key = super::state::key(here.as_str(), &info.id);
        if open.contains(&key) {
            continue;
        }
        let project = s.project_for_session(info);
        rows.push(Row {
            key,
            server: here.as_str().to_owned(),
            project: project.map(|p| p.id.clone()),
            open: false,
            active: !s.settings_open() && *s.route == Route::here(info.id.clone()),
            parent: info.parent.as_deref().map(|p| super::state::key(here.as_str(), p)),
            status: m::status_of(s.tab_status(&info.id)),
            updated: s.recent_page.active_ms(info),
            created: info.created,
            order: 0,
        });
        items.push(Item { tab: None, server: here.clone(), session: Some(info), project });
    }
    (rows, items)
}

/// What a section's footer is (`SectionFooter`): the group's other sessions, or the one current
/// session a collapsed group keeps showing.
enum Footer<'g> {
    Group(&'g Group),
    Row(usize),
}

/// A section's shape (`SectionPlan`): its header, its open tabs and its footer.
struct Plan<'g> {
    key: String,
    header: Option<(&'g Group, bool)>,
    tabs: Vec<usize>,
    hide_avatars: bool,
    footer: Option<Footer<'g>>,
}

fn active_recent(rows: &[Row], g: &Group) -> Option<usize> {
    g.rows.iter().copied().find(|&i| rows[i].active && !rows[i].open)
}

fn leaf_plan<'g>(rows: &[Row], items: &[Item], g: &'g Group) -> Plan<'g> {
    Plan {
        key: g.key.clone(),
        header: (g.kind != Kind::All).then_some((g, false)),
        tabs: g.rows.iter().filter(|&&i| rows[i].open && (!g.collapsed || rows[i].active)).filter_map(|&i| items[i].tab).collect(),
        hide_avatars: g.kind == Kind::Project,
        footer: if g.collapsed { active_recent(rows, g).map(Footer::Row) } else { Some(Footer::Group(g)) },
    }
}

/// `plans`: a leaf group's one section; a Server → Project server's title above its projects'
/// sections, or, collapsed, with only the current tab and session under it.
fn plans<'g>(rows: &[Row], items: &[Item], groups: &'g [Group]) -> Vec<Plan<'g>> {
    groups
        .iter()
        .flat_map(|g| match &g.children {
            None => vec![leaf_plan(rows, items, g)],
            Some(children) if !g.collapsed => std::iter::once(Plan { key: g.key.clone(), header: Some((g, true)), tabs: vec![], hide_avatars: false, footer: None }).chain(children.iter().map(|c| leaf_plan(rows, items, c))).collect(),
            Some(children) => {
                let active = children.iter().find(|c| c.rows.iter().any(|&i| rows[i].active));
                vec![Plan {
                    key: g.key.clone(),
                    header: Some((g, true)),
                    tabs: active.map(|c| c.rows.iter().filter(|&&i| rows[i].active && rows[i].open).filter_map(|&i| items[i].tab).collect()).unwrap_or_default(),
                    hide_avatars: false,
                    footer: active.and_then(|c| active_recent(rows, c)).map(Footer::Row),
                }]
            }
        })
        .collect()
}

/// Walks a row's parents among the listed rows: its top ancestor and how deep it is.
fn anchor(rows: &[Row], by_key: &HashMap<&str, usize>, mut i: usize) -> (usize, usize) {
    let mut depth = 0;
    while let Some(&parent) = rows[i].parent.as_deref().and_then(|p| by_key.get(p)) {
        if depth > rows.len() {
            break;
        }
        i = parent;
        depth += 1;
    }
    (i, depth)
}

/// The Customize list in the sidebar's list slot.
pub(crate) fn customized(s: &App, p: &Pal) -> El {
    let st = &s.project_state.customize;
    let servers = crate::servers::registry::list(s);
    // The server groupings only separate anything with a second server; the saved choice
    // returns with it.
    let grouping = if servers.len() > 1 || !st.grouping.by_server() { st.grouping } else { Grouping::Project };
    let (rows, items) = gather(s, st.show.subagents);
    let groups = m::view(&rows, &ViewState { grouping, ordering: st.ordering, show: st.show, filters: &st.filters, collapsed: &st.collapsed });
    let listed: Vec<usize> = groups.iter().flat_map(|g| g.children.iter().flatten().chain(std::iter::once(g))).flat_map(|g| g.rows.iter().copied()).collect();
    let by_key: HashMap<&str, usize> = listed.iter().map(|&i| (rows[i].key.as_str(), i)).collect();
    let group_of: HashMap<usize, &Group> = groups.iter().flat_map(|g| g.children.iter().flatten().chain(std::iter::once(g))).flat_map(|g| g.rows.iter().map(move |&i| (i, g))).collect();
    let ctx = Ctx { s, p, rows: &rows, items: &items, by_key: &by_key, group_of: &group_of };

    let mut list = col().key("customize-sections").w_full().pt(4.0).child(ctx.sections(&plans(&rows, &items, &groups)));
    let filtered = st.filters.count() > 0;
    if filtered && !listed.iter().any(|&i| !rows[i].active) {
        list = list.child(no_match(p));
    }

    // Projects and servers to filter by, and the unread sessions Mark all as read clears.
    let mut projects: Vec<(String, String)> = vec![];
    for it in &items {
        if let Some(proj) = it.project.filter(|proj| !projects.iter().any(|(id, _)| *id == proj.id)) {
            projects.push((proj.id.clone(), proj.name.clone()));
        }
    }
    let server_opts: Vec<(String, String)> = servers.iter().map(|e| (e.key().as_str().to_owned(), crate::servers::registry::label(&e.conn))).collect();
    let here = crate::servers::runtime::active(s).clone();
    let unread: Vec<String> = listed.iter().filter_map(|&i| items[i].session.filter(|ses| items[i].server == here && (ses.unread() || crate::session_menu::unread::manual(s, &here, &ses.id))).map(|ses| ses.id.clone())).collect();

    let header = header(s, p, server_opts, projects, unread);
    let viewport = col().grow(1.0).shrink().min_h(0.0);
    let scroll = session_view::scroller::scroller("customize-scroll").build::<App>(p, viewport, list);
    col().key("customize").w_full().grow(1.0).shrink().min_h(0.0).child(header).child(scroll)
}

/// The Sessions header (`sidebar-customized.tsx:675-697`): its label, View options and New session.
fn header(s: &App, p: &Pal, servers: Vec<(String, String)>, projects: Vec<(String, String)>, unread: Vec<String>) -> El {
    let open = s.popup.open_at(super::menu::TRIGGER);
    let options = super::super::home::projects::icon_button(p, super::menu::TRIGGER, t!("sidebar.view.options"), &ic::OUTLINE_SLIDERS, open).menu_button(super::menu::TRIGGER, move |s: &mut App, tx: &mut Tx| {
        let o = super::menu::Options { servers: servers.clone(), projects: projects.clone(), can_mark_read: !unread.is_empty(), unread: unread.clone() };
        super::menu::open(s, o, tx)
    });
    let new = super::super::home::projects::icon_button(p, "customize-new", t!("command.session.new"), &ic::PLUS, false).on_press_tx(|s: &mut App, tx: &mut Tx| s.new_session(tx));
    row()
        .key("customize-header")
        .w_full()
        .h(28.0)
        .pl(8.0)
        .pr(4.0)
        .gap(6.0)
        .center_y()
        .child(text(t!("sidebar.view.sessions")).font(HEADER).color(p.muted).grow(1.0).shrink().truncate())
        .child(options)
        .child(new)
}

/// "No other sessions match" and its Reset filters (`sidebar-customized.tsx:700-711`).
fn no_match(p: &Pal) -> El {
    row()
        .key("customize-no-match")
        .w_full()
        .px(8.0)
        .py(8.0)
        .gap(4.0)
        .child(text(t!("sidebar.view.noMatch")).font(UI_SMALL).color(p.faint).shrink())
        .child(
            row().key("reset").role("button").label(t!("sidebar.view.resetFilters")).focus_inset(p).hover_group().on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.project_state.customize.filters = Default::default();
            }).child(text(t!("sidebar.view.resetFilters")).font(UI_SMALL).color(p.faint).group_color(p.text)),
        )
}

struct Ctx<'a, 'g> {
    s: &'a App,
    p: &'a Pal,
    rows: &'a [Row],
    items: &'a [Item<'a>],
    by_key: &'a HashMap<&'a str, usize>,
    group_of: &'a HashMap<usize, &'g Group>,
}

impl Ctx<'_, '_> {
    /// The sections as official's one list has them (`sidebar-customized.tsx:481-496`,
    /// `tab-strip.tsx:1052-1073`): ONE strip of every section's open tabs, each section's header
    /// before its first tab and its other rows after its last, everything 3 apart
    /// (`tab-nav.css:179-181`). One strip, so the window's one drag names one tab, and a drag
    /// stays between its section's header and footer (`tab-strip.tsx:1205-1206`).
    ///
    /// An open tab's subagents stand after the section's tabs, not under their own tab as
    /// official's do (`tabFooter`, `sidebar-customized.tsx:522-532`): a row between two tabs
    /// would end the drag there.
    fn sections(&self, plans: &[Plan]) -> El {
        // What stands between the tabs: its key, the tab it stands before, and its height.
        let mut parts: Vec<(String, usize, El, f32)> = vec![];
        let mut indices: Vec<usize> = vec![];
        let mut bare: Vec<bool> = vec![];
        for (n, plan) in plans.iter().enumerate() {
            if let Some((g, section)) = plan.header {
                // A leaf group's 28 pt row, or a section's 24 pt title 8 under what is above it.
                let h = if !section { 28.0 } else if n == 0 { 24.0 } else { 32.0 };
                parts.push((format!("head-{}", plan.key), indices.len(), self.header(g, section, n == 0), h));
            }
            bare.extend(plan.tabs.iter().map(|_| plan.hide_avatars));
            indices.extend(&plan.tabs);
            let (rows, h) = self.after(plan);
            if !rows.is_empty() {
                parts.push((format!("foot-{}", plan.key), indices.len(), col().key(format!("cz-foot-{}", plan.key)).w_full().gap(3.0).children(rows), h));
            }
        }
        // No open tab: the headers and rows alone, as the list's rows.
        if indices.is_empty() {
            return col().key("cz-rows").w_full().gap(3.0).children(parts.into_iter().map(|(_, _, row, _)| row));
        }
        let room: f32 = parts.iter().map(|(.., h)| h + 3.0).sum();
        let h = rune::memo::layout_with("cz-tabs-list", |r| r[3]).unwrap_or(indices.len() as f32 * 31.0 + room + 16.0);
        let breaks = parts.into_iter().map(|(key, at, row, h)| TabBreak::new(key, at, row, h)).collect();
        col().key("cz-tabs-box").w_full().h(h).child(crate::view::tab_strip::grouped_strip(self.s, self.p, "cz-tabs", &indices, Sections { breaks, bare }))
    }

    /// What stands after a section's tabs: the sessions under its open tabs (their subagents),
    /// then its footer, each row 28 high (Show more 24) and 3 apart; and their height together.
    fn after(&self, plan: &Plan) -> (Vec<El>, f32) {
        let mut rows = if plan.tabs.is_empty() { vec![] } else { self.tab_children(plan) };
        let mut more = false;
        match plan.footer {
            Some(Footer::Group(g)) => {
                let (footer, button) = self.footer(g);
                rows.extend(footer);
                more = button.is_some();
                rows.extend(button);
            }
            Some(Footer::Row(i)) => rows.extend(self.session(i, true)),
            None => {}
        }
        let n = rows.len() as f32;
        let h = if rows.is_empty() { 0.0 } else { n * 28.0 + (n - 1.0) * 3.0 - if more { 4.0 } else { 0.0 } };
        (rows, h)
    }
    fn header(&self, g: &Group, section: bool, first: bool) -> El {
        let s = self.s;
        let project = (g.kind == Kind::Project).then(|| self.items.iter().find_map(|it| it.project.filter(|p| p.id == g.id))).flatten();
        let (label, lead) = match (g.kind, project) {
            (Kind::Server, _) => (crate::servers::registry::label_of(s, &opencode_kit::servers::ServerKey::new(&g.id)), Lead::Server),
            (_, Some(proj)) => (proj.name.clone(), Lead::Project(proj)),
            _ => (t!("sidebar.view.other").to_owned(), Lead::Other),
        };
        // A group's server, or the first of its rows' servers, takes its new sessions.
        let new_session = project.map(|proj| {
            let server = g.server.clone().or_else(|| g.rows.first().map(|&i| self.rows[i].server.clone())).unwrap_or_else(|| crate::servers::runtime::active(s).as_str().to_owned());
            (proj.directory.clone(), opencode_kit::servers::ServerKey::new(&server))
        });
        group_header(s, self.p, &g.key, label, lead, section, first, g.collapsed, g.open, new_session)
    }

    /// The sessions under each open tab in the section: its subagents (`tabFooter`), after them.
    fn tab_children(&self, plan: &Plan) -> Vec<El> {
        let Some(Footer::Group(g)) = plan.footer else { return vec![] };
        g.rows.iter().copied().filter(|&i| {
            let (top, _) = anchor(self.rows, self.by_key, i);
            top != i && self.rows[top].open
        }).filter_map(|i| self.session(i, false)).collect()
    }

    /// `GroupFooter`: the group's sessions no open tab anchors, its first `limit` top-level ones
    /// with their subagents, and Show more while there are more.
    fn footer(&self, g: &Group) -> (Vec<El>, Option<El>) {
        let rows = self.rows;
        let footer: Vec<usize> = g.rows.iter().copied().filter(|&i| !rows[anchor(rows, self.by_key, i).0].open).collect();
        let top: Vec<usize> = footer.iter().copied().filter(|&i| anchor(rows, self.by_key, i).0 == i).collect();
        let step = if g.kind == Kind::All { STEP_UNGROUPED } else { STEP };
        let limit = self.s.project_state.customize.limits.get(&g.key).copied().unwrap_or(step);
        let shown: HashSet<usize> = top.iter().copied().take(limit).collect();
        let out: Vec<El> = footer.iter().copied().filter(|&i| shown.contains(&anchor(rows, self.by_key, i).0)).filter_map(|i| self.session(i, false)).collect();
        let key = g.key.clone();
        let more = (top.len() > limit).then(|| {
            ghost_button(self.p, &format!("cz-more-{}", g.key), t!("sidebar.organize.showMore"), true).on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.project_state.customize.limits.insert(key.clone(), limit + step);
            })
        });
        (out, more)
    }

    /// A session row (`SessionRowItem`), `flat` for a collapsed group's current one; none for a
    /// row whose session the server has not listed.
    fn session(&self, i: usize, flat: bool) -> Option<El> {
        let (row, it) = (&self.rows[i], &self.items[i]);
        let info = it.session?;
        let group = self.group_of.get(&i).copied();
        let st = &self.s.project_state.customize;
        // A server's label where several mix: in one list, or a project spanning servers.
        let label = (st.show.server_labels && group.is_none_or(|g| g.kind == Kind::All || g.spans_servers)).then(|| crate::servers::runtime::tag(&it.server)).flatten();
        // Rows outside a project group carry their project's avatar; subagents follow their parent.
        let project = it.project.filter(|_| group.is_none_or(|g| g.kind != Kind::Project) && row.parent.is_none());
        let depth = if flat { 0 } else { anchor(self.rows, self.by_key, i).1 };
        Some(session_row(self.s, self.p, format!("cz-row-{}", row.key), SessionRow { server: &it.server, session: info, active: row.active, open_tab: false, label, depth, project }))
    }
}

//! The Customize sidebar's grouping, ordering and filtering (`shell/titlebar/sidebar-view.ts`):
//! pure, over rows the view gathers from the open tabs and the server's sessions.

use std::collections::{BTreeMap, HashMap, HashSet};

pub use crate::projects::sidebar::{Filters, Grouping, Ordering, Show, Status, groupings};

pub fn status_of(mark: Option<crate::TabStatus>) -> Status {
    match mark {
        Some(crate::TabStatus::Busy) => Status::Running,
        Some(crate::TabStatus::Attention) => Status::Input,
        Some(crate::TabStatus::Unread) => Status::Unread,
        Some(crate::TabStatus::Failed) => Status::Failed,
        None => Status::Idle,
    }
}

/// What the view needs (`SidebarViewState`).
pub struct ViewState<'a> {
    pub grouping: Grouping,
    pub ordering: Ordering,
    pub show: Show,
    pub filters: &'a Filters,
    pub collapsed: &'a BTreeMap<String, bool>,
}

/// One row (`SidebarRow`): an open tab, a recent session or a subagent.
#[derive(Clone, Debug)]
pub struct Row {
    pub key: String,
    pub server: String,
    pub project: Option<String>,
    pub open: bool,
    pub active: bool,
    pub parent: Option<String>,
    pub status: Status,
    pub updated: i64,
    pub created: i64,
    /// Position in the saved tab order, for open rows.
    pub order: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    All,
    Project,
    Server,
}

/// A group (`SidebarGroup`). `rows` are indices into the rows the view was built from.
#[derive(Clone, Debug)]
pub struct Group {
    pub key: String,
    pub kind: Kind,
    /// The project id, "other", the server key, or "all".
    pub id: String,
    /// The server of a server group, or of a project group nested in one.
    pub server: Option<String>,
    pub collapsed: bool,
    /// Open top-level rows in the group, nested groups included.
    pub open: usize,
    /// Leaf groups: sorted rows, subagents right after their parent. Server → Project parents: none.
    pub rows: Vec<usize>,
    pub spans_servers: bool,
    pub children: Option<Vec<Group>>,
}

/// `sidebarView`: the groups `rows` fall in under `state`.
pub fn view(rows: &[Row], state: &ViewState) -> Vec<Group> {
    let kept = kept_rows(rows, state);
    let keys: HashSet<&str> = kept.iter().map(|&i| rows[i].key.as_str()).collect();
    let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
    for &i in &kept {
        if let Some(parent) = rows[i].parent.as_deref().filter(|p| keys.contains(p)) {
            children.entry(parent).or_default().push(i);
        }
    }
    for list in children.values_mut() {
        list.sort_by(|&a, &b| rows[b].updated.cmp(&rows[a].updated).then_with(|| rows[a].key.cmp(&rows[b].key)));
    }
    let mut top: Vec<usize> = kept.iter().copied().filter(|&i| rows[i].parent.as_deref().is_none_or(|p| !keys.contains(p))).collect();
    top.sort_by(|&a, &b| {
        let (a, b) = (&rows[a], &rows[b]);
        b.open.cmp(&a.open).then_with(|| compare(state.ordering, a, b)).then_with(|| a.key.cmp(&b.key))
    });
    let with_children = |i: usize| {
        let mut out = vec![];
        let mut stack = vec![i];
        while let Some(i) = stack.pop() {
            out.push(i);
            if let Some(list) = children.get(rows[i].key.as_str()) {
                stack.extend(list.iter().rev());
            }
        }
        out
    };
    build(rows, &top, &with_children, state)
}

fn kept_rows(rows: &[Row], state: &ViewState) -> Vec<usize> {
    let visible: Vec<usize> = (0..rows.len()).filter(|&i| rows[i].active || is_visible(&rows[i], state)).collect();
    let by_key: HashMap<&str, usize> = visible.iter().map(|&i| (rows[i].key.as_str(), i)).collect();
    let attached = |mut i: usize| {
        let mut seen = 0;
        loop {
            let row = &rows[i];
            if row.active || row.parent.is_none() {
                return true;
            }
            match row.parent.as_deref().and_then(|p| by_key.get(p)) {
                Some(&parent) if seen < rows.len() => {
                    i = parent;
                    seen += 1;
                }
                _ => return false,
            }
        }
    };
    visible.iter().copied().filter(|&i| attached(i)).collect()
}

fn is_visible(row: &Row, state: &ViewState) -> bool {
    if row.parent.is_some() && !state.show.subagents {
        return false;
    }
    if !row.open && row.parent.is_none() && !state.show.recent {
        return false;
    }
    let f = state.filters;
    if !f.status.is_empty() && !f.status.contains(&row.status) {
        return false;
    }
    if !f.server.is_empty() && !f.server.contains(&row.server) {
        return false;
    }
    f.project.is_empty() || f.project.iter().any(|p| *p == row.project.as_deref().unwrap_or("none"))
}

fn compare(ordering: Ordering, a: &Row, b: &Row) -> std::cmp::Ordering {
    match ordering {
        Ordering::Manual if a.open && b.open => a.order.cmp(&b.order),
        Ordering::Manual | Ordering::Updated => b.updated.cmp(&a.updated),
        Ordering::Created => b.created.cmp(&a.created),
        Ordering::Attention => a.status.attention().cmp(&b.status.attention()).then_with(|| b.updated.cmp(&a.updated)),
    }
}

struct Bucket {
    key: String,
    kind: Kind,
    id: String,
    server: Option<String>,
    rows: Vec<usize>,
}

fn build(rows: &[Row], top: &[usize], with_children: &dyn Fn(usize) -> Vec<usize>, state: &ViewState) -> Vec<Group> {
    let leaf = |b: Bucket| leaf(rows, b, with_children, state.collapsed);
    match state.grouping {
        Grouping::None => vec![leaf(Bucket { key: "all".into(), kind: Kind::All, id: "all".into(), server: None, rows: top.to_vec() })],
        Grouping::Project => project_buckets(rows, top, "", None).into_iter().map(leaf).collect(),
        Grouping::Server => by_server(rows, top).into_iter().map(|(server, list)| leaf(Bucket { key: format!("server:{server}"), kind: Kind::Server, id: server.clone(), server: Some(server), rows: list })).collect(),
        Grouping::ServerProject => by_server(rows, top)
            .into_iter()
            .map(|(server, list)| {
                let key = format!("server:{server}");
                let nested: Vec<Group> = project_buckets(rows, &list, &format!("{key}/"), Some(&server)).into_iter().map(leaf).collect();
                Group {
                    collapsed: state.collapsed.get(&key).copied().unwrap_or(false),
                    open: nested.iter().map(|g| g.open).sum(),
                    key,
                    kind: Kind::Server,
                    id: server.clone(),
                    server: Some(server),
                    rows: vec![],
                    spans_servers: false,
                    children: Some(nested),
                }
            })
            .collect(),
    }
}

/// Rows by server, in first-row order (`Map.groupBy`).
fn by_server(rows: &[Row], top: &[usize]) -> Vec<(String, Vec<usize>)> {
    let mut out: Vec<(String, Vec<usize>)> = vec![];
    for &i in top {
        match out.iter_mut().find(|(s, _)| *s == rows[i].server) {
            Some((_, list)) => list.push(i),
            None => out.push((rows[i].server.clone(), vec![i])),
        }
    }
    out
}

/// Project buckets in first-row order, rows without a project in a trailing "other" bucket.
fn project_buckets(rows: &[Row], top: &[usize], prefix: &str, server: Option<&str>) -> Vec<Bucket> {
    let mut out: Vec<(Option<String>, Vec<usize>)> = vec![];
    for &i in top {
        match out.iter_mut().find(|(p, _)| *p == rows[i].project) {
            Some((_, list)) => list.push(i),
            None => out.push((rows[i].project.clone(), vec![i])),
        }
    }
    out.sort_by_key(|(p, _)| p.is_none());
    out.into_iter()
        .map(|(project, list)| {
            let id = project.unwrap_or_else(|| "other".into());
            Bucket { key: format!("{prefix}project:{id}"), kind: Kind::Project, id, server: server.map(str::to_owned), rows: list }
        })
        .collect()
}

fn leaf(rows: &[Row], b: Bucket, with_children: &dyn Fn(usize) -> Vec<usize>, collapsed: &BTreeMap<String, bool>) -> Group {
    let open = b.rows.iter().filter(|&&i| rows[i].open).count();
    let list: Vec<usize> = b.rows.iter().flat_map(|&i| with_children(i)).collect();
    let spans_servers = b.kind == Kind::Project && list.iter().map(|&i| rows[i].server.as_str()).collect::<HashSet<_>>().len() > 1;
    Group { collapsed: collapsed.get(&b.key).copied().unwrap_or(false), open, rows: list, spans_servers, children: None, key: b.key, kind: b.kind, id: b.id, server: b.server }
}

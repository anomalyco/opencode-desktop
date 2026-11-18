//! The `@` popover's rows for a query, as Desktop's `contextList` builds them
//! (`composer/editor/interaction.ts:290-334`) over `composer/model.ts`'s sources: references, skills
//! and subagents (`context`, `:226-259`), sessions (`searchSessions`, `:360-407`), the files open in
//! the session's side tabs (`recent`, `:87-92`) and the server's file search (`searchContextFiles`,
//! `:408-415`). fuzzysort filters the first three and the open files by their label; sessions and
//! searched files are already matched and keep their order after them (`skipFilter`); then the rows
//! group as skills, agents, sessions, references, resources, open files and files
//! (`sortGroupsBy`). A query with a space lists only sessions. A frame asking for the same rows
//! again gets the list it built.

use std::collections::HashMap;

use opencode_kit::model::SessionInfo;
use opencode_search::file_rank::file_tab_path;

use crate::fuzzysort::{self, Target, rank_session_title};
use crate::wire::session_id_ok;
use crate::{FILE_LIMIT, Item, Kind, MentionsHost, SESSIONS, shown_path, title_of};

/// Host-supplied cache invalidation stamp for loaded sessions, projects, and open tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionsStamp {
    /// Identity and version of the host's sessions collection (e.g. `(Tracked::id, Tracked::version)`).
    pub sessions: (u32, u64),
    /// Identity and version of the host's projects collection.
    pub projects: (u32, u64),
    /// Version of the host's open tabs collection.
    pub tabs_version: u64,
    /// Number of open tabs.
    pub tabs_len: usize,
}

#[derive(Default)]
pub(crate) struct Cache {
    sessions: Option<Sessions>,
    items: Option<(ItemsKey, Vec<Item>)>,
}

#[derive(Clone, PartialEq, Eq)]
struct SessionsKey {
    route: Option<String>,
    directory: String,
    counts: (usize, usize, usize),
    /// Moves when a session is added, renamed or updated: the sessions' and projects' cells (a
    /// window holds another server's when it moves to it) and versions, and the tabs' version.
    stamp: ((u32, u64), (u32, u64), u64),
}

struct Session {
    id: String,
    title: String,
    description: String,
    local: bool,
    open: bool,
    updated: i64,
}

struct Sessions {
    key: SessionsKey,
    rows: Vec<Session>,
}

#[derive(Clone, PartialEq, Eq)]
struct ItemsKey {
    query: String,
    sessions: SessionsKey,
    /// The file search's answer (its query and length), or the fixture's file count.
    files: (String, usize),
    catalog: (usize, usize, usize),
    recent: Vec<String>,
}

/// `sortGroupsBy`'s order (`interaction.ts:329-333`).
fn group(item: &Item) -> u8 {
    match item.kind {
        Kind::Skill => 0,
        Kind::Agent => 1,
        Kind::Session => 2,
        Kind::Reference => 3,
        Kind::File if item.recent => 5,
        Kind::File => 6,
    }
}

/// Loaded root sessions and `extra` server matches, newest metadata winning, newest first.
pub fn merged_roots<'a>(sessions: &'a [SessionInfo], extra: &'a [SessionInfo]) -> Vec<&'a SessionInfo> {
    let mut by_id: HashMap<&str, &SessionInfo> = HashMap::new();
    for s in sessions.iter().filter(|s| s.parent.is_none()).chain(extra) {
        if by_id.get(s.id.as_str()).is_none_or(|old| s.updated >= old.updated) {
            by_id.insert(&s.id, s);
        }
    }
    let mut all: Vec<&SessionInfo> = by_id.into_values().collect();
    // `compareSessionTime` (`shell/layout/helpers.ts`): newest `updated ?? created` first,
    // then by id, a code unit at a time.
    all.sort_by(|a, b| b.active_ms().cmp(&a.active_ms()).then_with(|| a.id.encode_utf16().cmp(b.id.encode_utf16())));
    all
}

/// Sessions a mention can name: loaded roots and what the server found, newest first, not
/// the open one.
pub fn mention_sessions(host: &dyn MentionsHost) -> Vec<&SessionInfo> {
    let current = host.active_session_id();
    merged_roots(host.sessions(), &host.mentions().found)
        .into_iter()
        .filter(|s| Some(s.id.as_str()) != current && session_id_ok(&s.id))
        .collect()
}

/// Extracts open file paths from side-region tab keys, deduplicated in strip order with the
/// active file first (`plugins.files.opened()`, `host-apis.tsx:928-932, 1228-1231`).
pub fn recent_files_from_tabs(strip: &[String], active: Option<&str>) -> Vec<String> {
    let file = |key: &str| key.strip_prefix("file:").and_then(file_tab_path);
    let mut all: Vec<String> = Vec::new();
    for path in strip.iter().filter_map(|key| file(key)) {
        if !all.contains(&path) {
            all.push(path);
        }
    }
    match active.and_then(file) {
        Some(active) => std::iter::once(active.clone()).chain(all.into_iter().filter(|p| *p != active)).collect(),
        None => all,
    }
}

fn sessions_key(host: &dyn MentionsHost) -> SessionsKey {
    let stamp = host.sessions_stamp();
    SessionsKey {
        route: host.active_session_id().map(str::to_owned),
        directory: host.active_directory(),
        counts: (host.sessions().len(), host.mentions().found.len(), stamp.tabs_len),
        stamp: (stamp.sessions, stamp.projects, stamp.tabs_version),
    }
}

fn build_sessions(host: &dyn MentionsHost, key: SessionsKey) -> Sessions {
    let project = host.project_id_at(&key.directory);
    let mut labels = HashMap::new();
    let rows = mention_sessions(host)
        .into_iter()
        .map(|s| Session {
            id: s.id.clone(),
            description: labels.entry((s.project.as_str(), s.directory.as_str())).or_insert_with(|| host.project_label(s)).clone(),
            local: project.as_ref().is_none_or(|id| *id == s.project) || s.directory == key.directory,
            open: host.is_session_tab_open(&s.id),
            updated: s.updated,
            title: title_of(s),
        })
        .collect();
    Sessions { key, rows }
}

/// `plugins.files.opened()` with the selected one first: the files the routed session's side
/// tabs show, in strip order (`host-apis.tsx:928-932, 1228-1231`).
fn recent_files(host: &dyn MentionsHost) -> Vec<String> {
    let Some((strip, active)) = host.side_tab_keys() else { return Vec::new() };
    recent_files_from_tabs(&strip, active.as_deref())
}

/// The server's latest file search, which stays listed until the next answers; in the fixture,
/// its files as a search would find them (fuzzysort standing in for the server's), and none
/// for a query the live search skips (`search::next_files`).
fn searched_files(host: &dyn MentionsHost, query: &str) -> Vec<String> {
    if host.is_live() {
        return host.mentions().search.files().iter().map(|p| shown_path(p)).collect();
    }
    if query.trim().is_empty() || query.chars().any(char::is_whitespace) {
        return Vec::new();
    }
    host.mentions().local_paths(query, FILE_LIMIT)
}

/// A row of the list as it last showed: a pick lands after the list has closed.
pub fn shown_mention(host: &dyn MentionsHost, key: &str) -> Option<Item> {
    host.mentions().cache.borrow().items.as_ref()?.1.iter().find(|i| i.key == key).cloned()
}

/// The popover's rows (`contextList`).
pub fn mention_items(host: &dyn MentionsHost) -> Vec<Item> {
    let Some(query) = host.mention_query() else { return Vec::new() };
    let sessions_key = sessions_key(host);
    let recent = recent_files(host);
    let m = host.mentions();
    let files = if host.is_live() {
        (m.search.files_query().to_owned(), m.search.files().len())
    } else {
        (m.paths_version().to_string(), m.file_index.len())
    };
    let key = ItemsKey {
        query: query.to_owned(),
        sessions: sessions_key.clone(),
        files,
        catalog: (m.references.len(), m.skills.len(), m.agents.len()),
        recent,
    };
    let mut cache_ref = m.cache.borrow_mut();
    let cache = &mut *cache_ref;
    if cache.sessions.as_ref().is_none_or(|s| s.key != sessions_key) {
        cache.sessions = Some(build_sessions(host, sessions_key));
        cache.items = None;
    }
    if let Some((k, items)) = &cache.items
        && *k == key
    {
        return items.clone();
    }
    let rows = cache.sessions.as_ref().map_or(&[][..], |s| &s.rows[..]);
    let sessions = search_sessions(rows, &key.query);
    // Only session titles hold spaces: matches elsewhere would keep the list over prose.
    let items = if key.query.chars().any(char::is_whitespace) { sessions } else { context_items(host, &key, sessions) };
    cache.items = Some((key, items.clone()));
    items
}

/// Everything but the sessions, filtered and grouped (`useFilteredList`'s `filterItems`).
fn context_items(host: &dyn MentionsHost, key: &ItemsKey, sessions: Vec<Item>) -> Vec<Item> {
    let m = host.mentions();
    let references = m.references.iter().map(|r| Item {
        key: format!("reference:{}", r.path),
        kind: Kind::Reference,
        label: format!("@{}", r.name),
        description: r.description.clone(),
        content: format!("@{}", r.name),
        recent: false,
    });
    let skills = m.skills.iter().map(|s| Item {
        key: format!("skill:{}", s.id),
        kind: Kind::Skill,
        label: format!("@{}", s.id),
        description: s.description.split_whitespace().collect::<Vec<_>>().join(" "),
        content: format!("@{}", s.id),
        recent: false,
    });
    let agents = m.agents.iter().map(|(name, _)| Item {
        key: format!("agent:{name}"),
        kind: Kind::Agent,
        label: format!("@{name}"),
        description: String::new(),
        content: format!("@{name}"),
        recent: false,
    });
    let file = |path: &str, recent: bool| Item {
        key: format!("file:{path}"),
        kind: Kind::File,
        label: path.to_owned(),
        description: String::new(),
        content: format!("@{path}"),
        recent,
    };
    let recent = key.recent.iter().map(|p| file(p, true));
    let searched: Vec<Item> = searched_files(host, &key.query).into_iter().filter(|p| !key.recent.contains(p)).map(|p| file(&p, false)).collect();
    let all: Vec<Item> = references.chain(skills).chain(agents).chain(sessions).chain(recent).chain(searched).collect();
    let needle = key.query.to_lowercase();
    let mut items = if needle.is_empty() {
        all
    } else {
        let skip = |i: &Item| i.kind == Kind::Session || (i.kind == Kind::File && !i.recent);
        let (skipped, filterable): (Vec<Item>, Vec<Item>) = all.into_iter().partition(skip);
        let targets: Vec<[Target; 1]> = filterable.iter().map(|i| [Target::new(&i.label)]).collect();
        let objects: Vec<&[Target]> = targets.iter().map(|t| &t[..]).collect();
        let found = fuzzysort::go(&needle, &objects);
        let mut filterable: Vec<Option<Item>> = filterable.into_iter().map(Some).collect();
        found.into_iter().filter_map(|i| filterable[i].take()).chain(skipped).collect()
    };
    // `groupItems` keeps each group's order; the groups sort stably by kind.
    items.sort_by_key(group);
    items
}

/// `searchSessions`: not the open session; a bare `@` offers this project's, a query any whose
/// title words it starts; this project's first, then the closer match, those in a tab, the
/// newest; at most 5.
fn search_sessions(rows: &[Session], query: &str) -> Vec<Item> {
    let mut found: Vec<(&Session, u8)> = rows
        .iter()
        .filter(|s| !query.is_empty() || s.local)
        .filter_map(|s| Some((s, if query.is_empty() { 0 } else { rank_session_title(&s.title, query)? })))
        .collect();
    found.sort_by(|(a, x), (b, y)| b.local.cmp(&a.local).then(x.cmp(y)).then(b.open.cmp(&a.open)).then(b.updated.cmp(&a.updated)));
    found
        .into_iter()
        .take(SESSIONS)
        .map(|(s, _)| Item {
            key: format!("session:{}", s.id),
            kind: Kind::Session,
            content: format!("@{}", s.title),
            label: s.title.clone(),
            description: s.description.clone(),
            recent: false,
        })
        .collect()
}

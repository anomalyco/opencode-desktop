//! The `@` popover's rows for a query, as Desktop's `contextList` builds them
//! (`composer/editor/interaction.ts:290-334`) over `composer/model.ts`'s sources: references, skills
//! and subagents (`context`, `:226-259`), sessions (`searchSessions`, `:360-407`), the files open in
//! the session's side tabs (`recent`, `:87-92`) and the server's file search (`searchContextFiles`,
//! `:408-415`). fuzzysort filters the first three and the open files by their label; sessions and
//! searched files are already matched and keep their order after them (`skipFilter`); then the rows
//! group as skills, agents, sessions, references, resources, open files and files
//! (`sortGroupsBy`). A query with a space lists only sessions. A frame asking for the same rows
//! again gets the list it built.

use super::fuzzysort::{self, Target, rank_session_title};
use super::{FILE_LIMIT, Item, Kind, SESSIONS, shown_path};
use crate::view::title_of;
use crate::{App, Route, Source};

#[derive(Default)]
pub(crate) struct Cache {
    sessions: Option<Sessions>,
    items: Option<(ItemsKey, Vec<Item>)>,
}

#[derive(Clone, PartialEq)]
struct SessionsKey {
    route: Option<String>,
    directory: String,
    counts: (usize, usize, usize),
    /// Moves when a session is added, renamed or updated.
    stamp: i64,
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

#[derive(Clone, PartialEq)]
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

impl App {
    fn sessions_key(&self) -> SessionsKey {
        SessionsKey {
            route: match &self.route {
                Route::Session(id) => Some(id.clone()),
                _ => None,
            },
            directory: self.composer_directory(),
            counts: (self.sessions.len(), self.mentions.found.len(), self.tabs.len()),
            stamp: self.sessions.iter().chain(&self.mentions.found).fold(0i64, |sum, s| sum.wrapping_add(s.updated)),
        }
    }

    fn build_sessions(&self, key: SessionsKey) -> Sessions {
        let project = self.project_at(&key.directory).map(|p| p.id.clone());
        let mut labels = std::collections::HashMap::new();
        let rows = self
            .mention_sessions()
            .into_iter()
            .map(|s| Session {
                id: s.id.clone(),
                description: labels.entry((&s.project, &s.directory)).or_insert_with(|| self.project_label(s)).clone(),
                local: project.as_ref().is_none_or(|id| *id == s.project) || s.directory == key.directory,
                open: self.tabs.iter().any(|t| t.id == s.id),
                updated: s.updated,
                title: title_of(s),
            })
            .collect();
        Sessions { key, rows }
    }

    /// `plugins.files.opened()` with the selected one first: the files the routed session's side
    /// tabs show, in strip order (`host-apis.tsx:928-932, 1228-1231`).
    fn recent_files(&self) -> Vec<String> {
        let Some(region) = self.resolve_side_region() else { return Vec::new() };
        let file = |key: &str| key.strip_prefix("file:").and_then(crate::files::data::file_tab_path);
        let mut all: Vec<String> = Vec::new();
        for path in region.strip.iter().filter_map(|e| file(&e.key)) {
            if !all.contains(&path) {
                all.push(path);
            }
        }
        match region.active.as_deref().and_then(file) {
            Some(active) => std::iter::once(active.clone()).chain(all.into_iter().filter(|p| *p != active)).collect(),
            None => all,
        }
    }

    /// The server's latest file search, which stays listed until the next answers; in the fixture,
    /// its files as a search would find them (fuzzysort standing in for the server's).
    fn searched_files(&self, query: &str) -> Vec<String> {
        match &self.source {
            Source::Live { .. } => self.mentions.search.files().iter().map(|p| shown_path(p)).collect(),
            _ => {
                let index = &self.mentions.file_index;
                if query.is_empty() {
                    return index.iter().take(FILE_LIMIT).map(|p| shown_path(p)).collect();
                }
                let targets: Vec<[Target; 1]> = index.iter().map(|p| [Target::new(&shown_path(p))]).collect();
                let objects: Vec<&[Target]> = targets.iter().map(|t| &t[..]).collect();
                fuzzysort::go(&query.to_lowercase(), &objects).into_iter().take(FILE_LIMIT).map(|i| shown_path(&index[i])).collect()
            }
        }
    }

    /// A row of the list as it last showed: a pick lands after the list has closed.
    pub(crate) fn shown_mention(&self, key: &str) -> Option<Item> {
        self.mentions.cache.borrow().items.as_ref()?.1.iter().find(|i| i.key == key).cloned()
    }

    /// The popover's rows (`contextList`).
    pub fn mention_items(&self) -> Vec<Item> {
        let Some(query) = self.mention_query() else { return Vec::new() };
        let sessions_key = self.sessions_key();
        let recent = self.recent_files();
        let files = match &self.source {
            Source::Live { .. } => (self.mentions.search.files_query().to_owned(), self.mentions.search.files().len()),
            _ => (String::new(), self.mentions.file_index.len()),
        };
        let m = &self.mentions;
        let key = ItemsKey { query, sessions: sessions_key.clone(), files, catalog: (m.references.len(), m.skills.len(), m.agents.len()), recent };
        let mut cache = self.mentions.cache.borrow_mut();
        if cache.sessions.as_ref().is_none_or(|s| s.key != sessions_key) {
            cache.sessions = Some(self.build_sessions(sessions_key));
            cache.items = None;
        }
        if let Some((k, items)) = &cache.items
            && *k == key
        {
            return items.clone();
        }
        let sessions = self.search_sessions(&cache.sessions.as_ref().expect("built above").rows, &key.query);
        // Only session titles hold spaces: matches elsewhere would keep the list over prose.
        let items = if key.query.chars().any(char::is_whitespace) { sessions } else { self.context_items(&key, sessions) };
        cache.items = Some((key, items.clone()));
        items
    }

    /// Everything but the sessions, filtered and grouped (`useFilteredList`'s `filterItems`).
    fn context_items(&self, key: &ItemsKey, sessions: Vec<Item>) -> Vec<Item> {
        let m = &self.mentions;
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
        let file = |path: &str, recent: bool| Item { key: format!("file:{path}"), kind: Kind::File, label: path.to_owned(), description: String::new(), content: format!("@{path}"), recent };
        let recent = key.recent.iter().map(|p| file(p, true));
        let searched: Vec<Item> = self.searched_files(&key.query).into_iter().filter(|p| !key.recent.contains(p)).map(|p| file(&p, false)).collect();
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
    fn search_sessions(&self, rows: &[Session], query: &str) -> Vec<Item> {
        let mut found: Vec<(&Session, u8)> = rows
            .iter()
            .filter(|s| !query.is_empty() || s.local)
            .filter_map(|s| Some((s, if query.is_empty() { 0 } else { rank_session_title(&s.title, query)? })))
            .collect();
        found.sort_by(|(a, x), (b, y)| b.local.cmp(&a.local).then(x.cmp(y)).then(b.open.cmp(&a.open)).then(b.updated.cmp(&a.updated)));
        found
            .into_iter()
            .take(SESSIONS)
            .map(|(s, _)| Item { key: format!("session:{}", s.id), kind: Kind::Session, content: format!("@{}", s.title), label: s.title.clone(), description: s.description.clone(), recent: false })
            .collect()
    }
}

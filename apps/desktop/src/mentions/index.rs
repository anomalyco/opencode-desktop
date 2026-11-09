//! The `@` popover's rows for a query. Sessions and files are prepared for matching once (and
//! again when what they come from changes), each key narrows the matches of the query it
//! extends, and a frame asking for the rows again gets the same list.

use super::fuzzy::{self, Hit, Needle, Prepared, Tier};
use super::{FILE_LIMIT, Item, Kind, SESSIONS, shown_path};
use crate::view::title_of;
use crate::{App, Route, Source};

#[derive(Default)]
pub(crate) struct Cache {
    sessions: Option<Sessions>,
    files: Option<Files>,
    items: Option<(String, u64, Vec<Item>)>,
}

/// What each query typed so far finds in a list, shortest first: typing on narrows the last
/// one's matches, and deleting goes back to one already found.
#[derive(Default)]
struct Narrowed(Vec<(Needle, Vec<u32>, Vec<(usize, Hit)>)>);

impl Narrowed {
    /// The matches among `names`, scored, in list order.
    fn find<'a>(&mut self, needle: &Needle, names: impl Fn(usize) -> &'a Prepared, len: usize) -> Vec<(usize, Hit)> {
        while self.0.last().is_some_and(|(typed, ..)| !needle.extends(typed)) {
            self.0.pop();
        }
        if self.0.last().is_none_or(|(typed, ..)| typed != needle) {
            let kept: Vec<u32> = match self.0.last() {
                Some((_, kept, _)) => kept.iter().copied().filter(|&i| fuzzy::contains(needle, names(i as usize))).collect(),
                None => (0..len as u32).filter(|&i| fuzzy::contains(needle, names(i as usize))).collect(),
            };
            let hits = kept.iter().filter_map(|&i| Some((i as usize, fuzzy::score(needle, names(i as usize))?))).collect();
            self.0.push((needle.clone(), kept, hits));
        }
        self.0.last().expect("pushed above").2.clone()
    }
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
    name: Prepared,
}

struct Sessions {
    key: SessionsKey,
    /// Newest first, the open one left out.
    rows: Vec<Session>,
    narrowed: Narrowed,
}

#[derive(Clone, PartialEq)]
struct FilesKey {
    answer: String,
    count: usize,
}

struct Files {
    key: FilesKey,
    rows: Vec<(String, String, Prepared)>,
    narrowed: Narrowed,
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
            .map(|s| {
                let title = title_of(s);
                Session {
                    id: s.id.clone(),
                    description: labels.entry((&s.project, &s.directory)).or_insert_with(|| self.project_label(s)).clone(),
                    local: project.as_ref().is_none_or(|id| *id == s.project) || s.directory == key.directory,
                    open: self.tabs.iter().any(|t| t.id == s.id),
                    name: Prepared::new(&title),
                    title,
                }
            })
            .collect();
        Sessions { key, rows, narrowed: Narrowed::default() }
    }

    /// The fixture's files, or the server's latest answer, which stays until the next arrives.
    fn files_key(&self) -> FilesKey {
        match &self.source {
            Source::Live { .. } => FilesKey { answer: self.mentions.search.files_query().to_owned(), count: self.mentions.search.files().len() },
            _ => FilesKey { answer: String::new(), count: self.mentions.file_index.len() },
        }
    }

    fn build_files(&self, key: FilesKey) -> Files {
        let paths = match &self.source {
            Source::Live { .. } => self.mentions.search.files(),
            _ => &self.mentions.file_index,
        };
        let rows = paths
            .iter()
            .map(|p| {
                let shown = shown_path(p);
                let name = Prepared::new(&shown);
                (p.clone(), shown, name)
            })
            .collect();
        Files { key, rows, narrowed: Narrowed::default() }
    }

    /// The popover's rows. A bare `@` lists skills, agents and this project's newest sessions.
    /// A query lists what it finds best first: by tier, and within a tier skills, agents,
    /// sessions (this project's first, then the closest, then those in a tab, then the newest),
    /// then files.
    pub fn mention_items(&self) -> Vec<Item> {
        let Some(query) = self.mention_query() else { return Vec::new() };
        let sessions_key = self.sessions_key();
        let files_key = self.files_key();
        let mut cache = self.mentions.cache.borrow_mut();
        if cache.sessions.as_ref().is_none_or(|s| s.key != sessions_key) {
            cache.sessions = Some(self.build_sessions(sessions_key));
            cache.items = None;
        }
        if cache.files.as_ref().is_none_or(|f| f.key != files_key) {
            cache.files = Some(self.build_files(files_key));
            cache.items = None;
        }
        let catalog = (self.mentions.skills.len() + self.mentions.agents.len()) as u64;
        if let Some((q, c, items)) = &cache.items
            && *q == query
            && *c == catalog
        {
            return items.clone();
        }
        let needle = Needle::new(&query);
        let skills = self.mentions.skills.iter().map(|s| Item {
            key: format!("skill:{}", s.id),
            kind: Kind::Skill,
            label: format!("@{}", s.id),
            description: s.description.split_whitespace().collect::<Vec<_>>().join(" "),
            content: format!("@{}", s.id),
        });
        let agents = self.mentions.agents.iter().map(|(name, _)| Item {
            key: format!("agent:{name}"),
            kind: Kind::Agent,
            label: format!("@{name}"),
            description: String::new(),
            content: format!("@{name}"),
        });
        let cache = &mut *cache;
        let sessions = cache.sessions.as_mut().expect("built above");
        // Only session titles hold spaces: matches elsewhere would keep the list over prose
        // (`interaction.ts` `contextList`).
        let words = query.chars().any(char::is_whitespace);
        let items: Vec<Item> = if words {
            let rows = &sessions.rows;
            let mut found = sessions.narrowed.find(&needle, |i| &rows[i].name, rows.len());
            found.sort_by(|(a, x), (b, y)| y.tier.cmp(&x.tier).then(rows[*b].local.cmp(&rows[*a].local)).then(y.quality.cmp(&x.quality)).then(rows[*b].open.cmp(&rows[*a].open)));
            found.into_iter().take(SESSIONS).map(|(i, _)| session_item(&rows[i])).collect()
        } else if needle.is_empty() {
            let local = sessions.rows.iter().filter(|s| s.local).take(SESSIONS).map(session_item);
            skills.chain(agents).chain(local).collect()
        } else {
            let mut ranked: Vec<(Tier, u8, Item)> = Vec::new();
            for (group, list) in [(0, skills.collect::<Vec<_>>()), (1, agents.collect())] {
                let mut scored: Vec<(Hit, Item)> = list.into_iter().filter_map(|i| Some((fuzzy::score(&needle, &Prepared::new(&i.label))?, i))).collect();
                scored.sort_by(|a, b| b.0.cmp(&a.0));
                ranked.extend(scored.into_iter().map(|(h, i)| (h.tier, group, i)));
            }
            let rows = &sessions.rows;
            let mut found = sessions.narrowed.find(&needle, |i| &rows[i].name, rows.len());
            // Stable on the newest-first order the rows keep.
            found.sort_by(|(a, x), (b, y)| y.tier.cmp(&x.tier).then(rows[*b].local.cmp(&rows[*a].local)).then(y.quality.cmp(&x.quality)).then(rows[*b].open.cmp(&rows[*a].open)));
            ranked.extend(found.into_iter().take(SESSIONS).map(|(i, h)| (h.tier, 2, session_item(&rows[i]))));
            let files = cache.files.as_mut().expect("built above");
            let rows = &files.rows;
            let mut found = files.narrowed.find(&needle, |i| &rows[i].2, rows.len());
            if found.len() > FILE_LIMIT {
                found.select_nth_unstable_by(FILE_LIMIT, |(a, x), (b, y)| y.cmp(x).then(rows[*a].1.len().cmp(&rows[*b].1.len())).then(a.cmp(b)));
                found.truncate(FILE_LIMIT);
            }
            found.sort_by(|(a, x), (b, y)| y.cmp(x).then(rows[*a].1.len().cmp(&rows[*b].1.len())).then(a.cmp(b)));
            ranked.extend(found.into_iter().map(|(i, h)| {
                let (path, shown, _) = &rows[i];
                (h.tier, 3, Item { key: format!("file:{path}"), kind: Kind::File, content: format!("@{shown}"), label: shown.clone(), description: String::new() })
            }));
            ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            ranked.into_iter().map(|(_, _, i)| i).collect()
        };
        cache.items = Some((query, catalog, items.clone()));
        items
    }
}

fn session_item(s: &Session) -> Item {
    Item { key: format!("session:{}", s.id), kind: Kind::Session, content: format!("@{}", s.title), label: s.title.clone(), description: s.description.clone() }
}

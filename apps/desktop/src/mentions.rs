//! `@` in the composer: Desktop's context popover (`composer/editor/interaction.ts`,
//! `suggestions/machine.ts`, `composer/model.ts`). Typing `@` after a space or at the start
//! opens a list of skills, subagents, sessions and the server's file search; picking one puts
//! an atomic pill in the text, and sending turns the pills into the prompt's `agents`, `skills`
//! and `files` and session tokens (`composer/request.ts`, `prompt-parts.ts`).

use std::time::Duration;

use rune::{Cx, El, Key, KeyEvent, Latest, Tx, Waker, icon};
use rune_theme_opencode::icons as ic;
use serde_json::Value;

use opencode_kit::client::Server;
use opencode_kit::model::{self, SessionInfo};

use crate::palette::Pal;
use crate::view::title_of;
use crate::{App, Route, Source};
use list::{Highlight, Row, Spec, data_list, nav_step, popup, step_key, upsert_sessions};

pub mod badge;
pub mod list;
mod wire;

pub use wire::{Wire, wire};
use wire::session_id_ok;
#[cfg(test)]
use opencode_kit::client::{AgentInput, FileInput, Mention, SkillInput};
#[cfg(test)]
use wire::{absolute, encode_path, filename};

/// Session rows: Desktop shows at most 5 (`composer/model.ts:405`).
const SESSIONS: usize = 5;
/// Desktop waits this long after the last keystroke before searching files.
const FILE_DEBOUNCE: Duration = Duration::from_millis(150);
/// And before searching sessions.
const SESSION_DEBOUNCE: Duration = Duration::from_millis(100);
/// The server's default `limit` for `/api/fs/find`.
const FILE_LIMIT: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Skill,
    Agent,
    Session,
    File,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// `kind:value`, also the pill's id: `skill:<id>`, `agent:<name>`, `session:<id>`,
    /// `file:<path>`.
    pub key: String,
    pub kind: Kind,
    pub label: String,
    pub description: String,
    /// The pill's text.
    pub content: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
}

type Catalog = (String, Vec<(String, String)>, Vec<Skill>);

pub struct Mentions {
    /// The popover is open; its query is the text between the `@` and the caret.
    pub open: bool,
    /// The composer's text and caret when last looked at.
    seen: (String, usize),
    /// The highlighted row and the query it was picked under.
    active: Highlight,
    /// Subagents (`mode != primary`, not hidden) by name, with their descriptions.
    pub agents: Vec<(String, String)>,
    pub skills: Vec<Skill>,
    /// The directory `agents` and `skills` belong to, once asked for.
    catalog: Option<String>,
    catalog_fetch: Latest<Catalog>,
    /// Sessions the server found for a query's first word.
    found: Vec<SessionInfo>,
    searched: Option<String>,
    sessions_fetch: Latest<Vec<SessionInfo>>,
    /// The last file search's query and paths, and the query asked for since.
    files: (String, Vec<String>),
    files_asked: Option<String>,
    files_fetch: Latest<(String, Vec<String>)>,
    /// The fixture's files, searched locally.
    pub file_index: Vec<String>,
}

impl Default for Mentions {
    fn default() -> Self {
        Mentions {
            open: false,
            seen: (String::new(), 0),
            active: Highlight::default(),
            agents: Vec::new(),
            skills: Vec::new(),
            catalog: None,
            catalog_fetch: Latest::default(),
            found: Vec::new(),
            searched: None,
            sessions_fetch: Latest::default().debounce(SESSION_DEBOUNCE),
            files: (String::new(), Vec::new()),
            files_asked: None,
            files_fetch: Latest::default().debounce(FILE_DEBOUNCE),
            file_index: Vec::new(),
        }
    }
}

impl Mentions {
    pub fn set_waker(&mut self, waker: Waker) {
        self.catalog_fetch.set_waker(waker.clone());
        self.files_fetch.set_waker(waker.clone());
        self.sessions_fetch.set_waker(waker);
    }

    /// A session the `@` search found.
    pub fn found_session(&self, id: &str) -> Option<&SessionInfo> {
        self.found.iter().find(|s| s.id == id)
    }

    /// Fixture data in the server's shapes: `agent_list` as `/api/agent`, `skills` as `/api/skill`.
    pub fn load(&mut self, fixture: &Value) {
        self.agents = subagents(&fixture["agent_list"]);
        self.skills = skills(&fixture["skills"]);
        self.file_index = found_paths(&fixture["files"]);
    }

    /// Takes in what the fetch workers returned; whether any changed state.
    fn drain(&mut self) -> bool {
        let mut changed = false;
        if let Some((directory, agents, skills)) = self.catalog_fetch.take()
            && self.catalog.as_deref() == Some(directory.as_str())
        {
            self.agents = agents;
            self.skills = skills;
            changed = true;
        }
        if let Some((query, paths)) = self.files_fetch.take()
            && self.files_asked.as_deref() == Some(query.as_str())
        {
            self.files = (query, paths);
            changed = true;
        }
        if let Some(list) = self.sessions_fetch.take() {
            upsert_sessions(&mut self.found, list);
            changed = true;
        }
        changed
    }

    /// The directory's subagents and skills, once.
    fn ask_catalog(&mut self, server: &Server, directory: String) {
        if self.catalog.as_deref() == Some(directory.as_str()) {
            return;
        }
        self.catalog = Some(directory.clone());
        self.files = (String::new(), Vec::new());
        self.files_asked = None;
        self.files_fetch.cancel();
        let server = server.clone();
        self.catalog_fetch.request(directory, move |directory| {
            let at = format!("location%5Bdirectory%5D={}", crate::switcher::encode(&directory));
            let agents = server.get(&format!("/api/agent?{at}")).map(|v| subagents(&v)).unwrap_or_default();
            let skills = server.get(&format!("/api/skill?{at}")).map(|v| skills(&v)).unwrap_or_default();
            (directory, agents, skills)
        });
    }

    /// Files for a one-word query, after a pause for more typing.
    fn ask_files(&mut self, server: &Server, directory: String, query: &str) {
        if query.trim().is_empty() || query.contains(char::is_whitespace) {
            if self.files_asked.take().is_some() {
                self.files_fetch.cancel();
            }
            return;
        }
        if self.files_asked.as_deref() == Some(query) {
            return;
        }
        self.files_asked = Some(query.to_owned());
        let server = server.clone();
        self.files_fetch.request((directory, query.to_owned()), move |(directory, query)| {
            let path = format!(
                "/api/fs/find?location%5Bdirectory%5D={}&query={}",
                crate::switcher::encode(&directory),
                crate::switcher::encode(&query)
            );
            let paths = server.get(&path).map(|v| found_paths(&v)).unwrap_or_default();
            (query, paths)
        });
    }

    /// Sessions matching the query's first word, after a pause for more typing.
    fn ask_sessions(&mut self, server: &Server, query: &str) {
        let word = query.split(' ').next().unwrap_or("");
        if word.is_empty() {
            if self.searched.take().is_some() {
                self.sessions_fetch.cancel();
            }
            return;
        }
        if self.searched.as_deref() == Some(word) {
            return;
        }
        self.searched = Some(word.to_owned());
        let server = server.clone();
        self.sessions_fetch.request_opt(word.to_owned(), move |word| {
            let path = format!("/api/session?parentID=null&archived=false&limit=50&search={}", crate::switcher::encode(&word));
            server.get(&path).ok().map(|v| model::parse_sessions(&v))
        });
    }
}

/// `/api/agent`'s subagents: Desktop offers agents that aren't hidden or primary.
fn subagents(v: &Value) -> Vec<(String, String)> {
    data_list(v)
        .filter(|a| a["hidden"].as_bool() != Some(true) && a["mode"].as_str() != Some("primary"))
        .filter_map(|a| Some((a["id"].as_str().or(a["name"].as_str())?.to_owned(), a["description"].as_str().unwrap_or("").to_owned())))
        .collect()
}

/// A found path as a row and its pill say it: the server's relative path with `/` between its
/// parts, and a folder's trailing `/`. On Windows `/api/fs/find` ends a folder with `\` after
/// `/`-joined parts (`src/util\`).
fn shown_path(path: &str) -> String {
    use opencode_kit::path::{SEPARATORS, is_absolute, trim};
    if is_absolute(path) {
        return path.to_owned();
    }
    let joined = trim(path).split(SEPARATORS).collect::<Vec<_>>().join("/");
    if path.ends_with(SEPARATORS) && !joined.is_empty() { format!("{joined}/") } else { joined }
}

/// `/api/fs/find`'s paths.
fn found_paths(v: &Value) -> Vec<String> {
    data_list(v).filter_map(|f| f["path"].as_str().or(f.as_str()).map(str::to_owned)).collect()
}

fn skills(v: &Value) -> Vec<Skill> {
    data_list(v)
        .filter_map(|s| {
            let id = s["id"].as_str()?.to_owned();
            Some(Skill { name: s["name"].as_str().unwrap_or(&id).to_owned(), description: s["description"].as_str().unwrap_or("").to_owned(), id })
        })
        .collect()
}

/// The `@` before the caret and the query after it. Closed, the query is one run of non-space
/// characters; open, single spaces join words (and one may trail), so multi-word session titles
/// stay searchable. The `@` starts the text or follows whitespace.
pub fn trigger(before: &str, open: bool) -> Option<(usize, &str)> {
    let at = before.rfind('@')?;
    if before[..at].chars().next_back().is_some_and(|c| !c.is_whitespace()) {
        return None;
    }
    let query = &before[at + 1..];
    let ok = if open {
        !query.starts_with(' ') && !query.contains("  ") && query.chars().all(|c| c == ' ' || !c.is_whitespace())
    } else {
        !query.chars().any(char::is_whitespace)
    };
    ok.then_some((at, query))
}

/// `matchesSessionTitle`: each query word starts consecutive title words, ignoring case. A
/// trailing space asks for one more title word.
#[cfg(test)]
pub fn matches_session_title(title: &str, query: &str) -> bool {
    rank_session_title(title, query).is_some()
}

/// `rankSessionTitle` (`composer/session-match.ts:6-18`): 0 for the whole title, 1 when the
/// query's words start the title's first words, 2 when they start later ones; none otherwise.
pub fn rank_session_title(title: &str, query: &str) -> Option<u8> {
    if title.to_lowercase() == query.to_lowercase() {
        return Some(0);
    }
    let words: Vec<String> = query.to_lowercase().split(' ').map(str::to_owned).collect();
    let title = title.to_lowercase();
    let titles: Vec<&str> = title.split_whitespace().collect();
    let start = (0..titles.len()).find(|start| words.iter().enumerate().all(|(i, w)| titles.get(start + i).is_some_and(|t| t.starts_with(w.as_str()))))?;
    Some(if start == 0 { 1 } else { 2 })
}

/// A fuzzy match of `needle` (lowercase) in `hay`, higher is better: a substring beats a
/// scattered match, and earlier and tighter beat later and looser.
pub fn fuzzy(needle: &str, hay: &str) -> Option<i64> {
    let hay = hay.to_lowercase();
    if needle.is_empty() {
        return Some(0);
    }
    if let Some(at) = hay.find(needle) {
        return Some(10_000 - at as i64 * 10 - (hay.len() - needle.len()) as i64);
    }
    let mut chars = hay.char_indices();
    let (mut first, mut last) = (None, 0);
    for n in needle.chars() {
        let (i, _) = chars.find(|&(_, c)| c == n)?;
        first.get_or_insert(i);
        last = i;
    }
    Some(5_000 - (last - first.unwrap_or(0)) as i64 * 10 - hay.len() as i64)
}

/// Filters and ranks `items` by [`fuzzy`] score against `query` (best first, ties in input order).
pub fn fuzzy_rank<T>(query: &str, items: impl IntoIterator<Item = T>, label: impl Fn(&T) -> &str) -> Vec<T> {
    let needle = query.to_lowercase();
    let mut scored: Vec<(i64, T)> = items.into_iter().filter_map(|i| Some((fuzzy(&needle, label(&i))?, i))).collect();
    scored.sort_by_key(|(s, _)| -s);
    scored.into_iter().map(|(_, i)| i).collect()
}

impl App {
    /// The composer's directory: the open session's, or where a new one starts.
    pub fn composer_directory(&self) -> String {
        match &self.route {
            Route::Session(id) => self.session(id).map(|s| s.directory.clone()).unwrap_or_default(),
            _ => self.draft_dir().to_owned(),
        }
    }

    fn caret(&self) -> usize {
        self.composer.focus.min(self.composer.text.len())
    }

    /// Re-reads the composer after an edit: a new `@` opens the popover, and one the text or
    /// caret has left closes it. Escape stays closed until the text changes again.
    pub fn sync_mentions(&mut self) {
        let caret = self.caret();
        if self.mentions.seen.0 == self.composer.text && self.mentions.seen.1 == caret {
            if !self.composer.focused {
                self.mentions.open = false;
            }
            return;
        }
        let edited = self.mentions.seen.0 != self.composer.text;
        self.mentions.seen = (self.composer.text.clone(), caret);
        if !self.composer.focused || (!edited && !self.mentions.open) {
            self.mentions.open = false;
            return;
        }
        let open = self.mentions.open;
        self.mentions.open = self.mention_trigger(open).is_some();
    }

    /// After the composer's text changes: the popover opens, closes and refilters with this frame.
    pub fn mentions_changed(&mut self, tx: &mut Tx) {
        let was = self.mentions.open;
        self.sync_mentions();
        if was || self.mentions.open {
            tx.instant();
        }
    }

    /// The `@`'s byte offset and the query, when the text before the caret has one that isn't
    /// already a pill's.
    fn mention_trigger(&self, open: bool) -> Option<(usize, String)> {
        if self.add.shell {
            return None;
        }
        let text = self.composer.text.get(..self.caret())?;
        let (at, query) = trigger(text, open)?;
        let in_pill = self.composer.pills().any(|(r, _)| r.start <= at && at < r.end);
        (!in_pill).then(|| (at, query.to_owned()))
    }

    pub fn mention_query(&self) -> Option<String> {
        self.mentions.open.then(|| self.mention_trigger(true).map(|(_, q)| q)).flatten()
    }

    /// Sessions a mention can name: loaded roots and what the server found, newest first, not
    /// the open one.
    fn mention_sessions(&self) -> Vec<&SessionInfo> {
        let current = match &self.route {
            Route::Session(id) => Some(id.as_str()),
            _ => None,
        };
        self.merged_roots(&self.mentions.found)
            .into_iter()
            .filter(|s| Some(s.id.as_str()) != current && session_id_ok(&s.id))
            .collect()
    }

    /// The popover's rows: skills, then agents (both fuzzy on their labels), then sessions whose
    /// titles match. A query with a space only lists sessions.
    pub fn mention_items(&self) -> Vec<Item> {
        let Some(query) = self.mention_query() else { return Vec::new() };
        // `searchSessions` (`composer/model.ts:360-407`): this project's sessions first, then by
        // how well the title matches, then those open in a tab, then the newest. A bare `@`
        // offers only this project's.
        let project = self.project_at(&self.composer_directory()).map(|p| p.id.clone());
        let directory = self.composer_directory();
        let mut ranked: Vec<(bool, u8, bool, &SessionInfo)> = self
            .mention_sessions()
            .into_iter()
            .filter_map(|s| {
                let local = project.as_ref().is_none_or(|id| *id == s.project) || s.directory == directory;
                if query.is_empty() && !local {
                    return None;
                }
                let rank = if query.is_empty() { 0 } else { rank_session_title(&title_of(s), &query)? };
                Some((local, rank, self.tabs.iter().any(|t| t.id == s.id), s))
            })
            .collect();
        // Stable on the newest-first order `mention_sessions` keeps.
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(b.2.cmp(&a.2)));
        let sessions: Vec<Item> = ranked
            .into_iter()
            .take(SESSIONS)
            .map(|(_, _, _, s)| {
                let title = title_of(s);
                Item {
                    key: format!("session:{}", s.id),
                    kind: Kind::Session,
                    content: format!("@{title}"),
                    label: title,
                    description: self.project_label(s),
                }
            })
            .collect();
        if query.contains(char::is_whitespace) {
            return sessions;
        }
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
        let mut out = fuzzy_rank(&query, skills, |i| &i.label);
        out.extend(fuzzy_rank(&query, agents, |i| &i.label));
        out.extend(sessions);
        out.extend(self.mention_files(&query).into_iter().map(|path| {
            let shown = shown_path(&path);
            Item { key: format!("file:{path}"), kind: Kind::File, content: format!("@{shown}"), label: shown, description: String::new() }
        }));
        out
    }

    /// Files for a query: the fixture's, searched here, or the server's latest answer. Until
    /// the server answers a new query, the last answer stays, narrowed to what still matches.
    fn mention_files(&self, query: &str) -> Vec<String> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let narrow = |paths: &[String]| -> Vec<String> {
            fuzzy_rank(query, paths.iter().cloned(), String::as_str).into_iter().take(FILE_LIMIT).collect()
        };
        match &self.source {
            Source::Live { .. } if self.mentions.files.0 == query => self.mentions.files.1.clone(),
            Source::Live { .. } => narrow(&self.mentions.files.1),
            _ => narrow(&self.mentions.file_index),
        }
    }

    /// The highlighted row: the one picked under this query, else the first.
    pub fn mention_active(&self, items: &[Item]) -> Option<String> {
        self.mentions.active.resolve(&self.mention_query().unwrap_or_default(), items.iter().map(|i| i.key.as_str()))
    }

    pub fn mention_activate(&mut self, key: String) {
        let query = self.mention_query().unwrap_or_default();
        self.mentions.active.set(key, query);
    }

    /// Replaces the `@query` with the item's pill and a space, caret after it.
    pub fn pick_mention(&mut self, key: &str) {
        let items = self.mention_items();
        let Some(item) = items.iter().find(|i| i.key == key) else { return };
        let Some((at, _)) = self.mention_trigger(true) else { return };
        let caret = self.caret();
        self.composer.replace_with_pill(at..caret, item.key.clone(), &item.content);
        self.composer.insert(" ");
        self.mentions_settle();
    }

    /// After the app sets the composer's text itself (a recalled prompt): not an edit, so no popover.
    pub fn mentions_settle(&mut self) {
        self.close_mentions();
        self.mentions.seen = (self.composer.text.clone(), self.caret());
    }

    pub fn close_mentions(&mut self) {
        self.mentions.open = false;
        self.mentions.active.clear();
    }

    /// The composer's keys while the popover is open: arrows and Ctrl-N/P move and wrap, Tab
    /// and Enter pick, Escape closes.
    pub fn mention_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        self.sync_mentions();
        if !self.mentions.open {
            return false;
        }
        if let Some(step) = nav_step(ev, false) {
            let items = self.mention_items();
            let active = self.mention_active(&items);
            let keys = items.iter().map(|i| i.key.as_str());
            if let Some(key) = step_key(keys, active.as_deref(), step, tx, "mentions-scroll", "mention-") {
                self.mention_activate(key);
            }
            return true;
        }
        match ev.key {
            Key::Escape => {
                self.close_mentions();
                true
            }
            Key::Tab | Key::Enter if self.composer.preedit.is_none() => {
                let items = self.mention_items();
                if let Some(key) = self.mention_active(&items) {
                    self.pick_mention(&key);
                }
                true
            }
            _ => false,
        }
    }

    /// Takes in fetched lists, and asks the server for what the open popover needs: the
    /// directory's agents and skills once, and sessions by the query's first word.
    /// Fetched rows land with this frame: no fading in or sliding.
    pub fn poll_mentions(&mut self, tx: &mut Tx) -> bool {
        let was = self.mentions.open;
        let changed = self.take_mentions();
        if changed && (was || self.mentions.open) {
            tx.instant();
        }
        changed
    }

    fn take_mentions(&mut self) -> bool {
        let mut changed = self.mentions.drain();
        let was = self.mentions.open;
        self.sync_mentions();
        changed |= was != self.mentions.open;
        let Some(query) = self.mention_query() else {
            self.mentions.searched = None;
            self.mentions.files_asked = None;
            self.mentions.sessions_fetch.cancel();
            self.mentions.files_fetch.cancel();
            return changed;
        };
        let Source::Live { server, .. } = &self.source else { return changed };
        let server = server.clone();
        let directory = self.composer_directory();
        self.mentions.ask_catalog(&server, directory.clone());
        self.mentions.ask_files(&server, directory, &query);
        self.mentions.ask_sessions(&server, &query);
        changed
    }

    /// The prompt's text and mentions for what's in the composer.
    pub fn mention_wire(&self) -> Wire {
        let pills: Vec<_> = self.composer.pills().collect();
        wire(&self.composer.text, &pills, &self.composer_directory(), |id| self.mentions.skills.iter().find(|s| s.id == id).map_or_else(|| id.to_owned(), |s| s.name.clone()))
    }
}

// ---------------------------------------------------------------------------------------------
// View

use session_view::user_parts::SESSION as SPEECH_BUBBLE;

/// A row's mark (`editor.tsx:1671-1686`): the small (14 pt) speech bubble, brain and skill
/// icons, which take the row button's own ink (`buttontext`: black, or white in dark), the
/// brain in `icon-info-active`; a file's 16 pt colour badge.
fn row_icon(p: &Pal, kind: Kind, label: &str) -> El {
    let ink = if p.dark { rune::Color::WHITE } else { rune::Color::BLACK };
    let info = rune::color::hex(if p.dark { "#fabbff" } else { "#912a9a" }).unwrap_or(p.info);
    match kind {
        Kind::Session => icon(&SPEECH_BUBBLE, 14.0).color(ink),
        Kind::Agent => icon(&ic::BRAIN, 14.0).color(info),
        Kind::Skill => icon(&ic::POST_SKILL, 14.0).color(ink),
        Kind::File => icon(rune_theme_opencode::file_icons::file(label), 16.0).color(p.text),
    }
}

/// The popover over the composer, full width, 8 points up: rows of icon, label and a muted
/// description. It appears and goes at once.
pub fn view(s: &App, p: &Pal, cx: &mut Cx<App>) -> Option<El> {
    if !s.mentions.open || s.mention_query().is_none() {
        return None;
    }
    let width = cx.layout_size("composer").map_or(0.0, |[w, _]| w);
    let items = s.mention_items();
    let active = s.mention_active(&items);
    let rows = items
        .iter()
        .map(|i| Row { id: i.key.clone(), key: format!("mention-{}", i.key), label: i.label.clone(), lead: Some(row_icon(p, i.kind, &i.label)), description: i.description.clone(), trail: None })
        .collect();
    let spec = Spec { key: "mentions", label: "Mentions", empty: "No matching items", width, boundary: list::boundary(cx) };
    Some(popup(p, spec, rows, active.as_deref(), |s, key| s.mention_activate(key.to_owned()), |s, key, _| s.pick_mention(key)))
}

#[cfg(test)]
pub(crate) mod tests;

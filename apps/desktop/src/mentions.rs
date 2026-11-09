//! `@` in the composer: Desktop's context popover (`composer/editor/interaction.ts`,
//! `suggestions/machine.ts`, `composer/model.ts`). Typing `@` after a space or at the start
//! opens a list of skills, subagents, sessions and the server's file search; picking one puts
//! an atomic pill in the text, and sending turns the pills into the prompt's `agents`, `skills`
//! and `files` and session tokens (`composer/request.ts`, `prompt-parts.ts`). Whether the list
//! is open, and its query and highlight, are the suggestion machine's (`crate::suggestions`);
//! the server's searches for it are [`search`]'s.

use rune::machine::{Attempt, Seq};
use rune::{Cx, El, Latest, Tx, Waker, icon};
use rune_theme_opencode::icons as ic;
use serde_json::Value;

use opencode_kit::model::{self, SessionInfo};

use crate::palette::Pal;
use crate::{App, Route, Source};
use list::{Row, Spec, data_list, popup, upsert_sessions};

pub mod badge;
pub mod fuzzy;
mod index;
mod chip_stories;
pub mod list;
pub mod refs;
pub mod search;
mod wire;

pub use wire::{Wire, context_input, wire};
use wire::session_id_ok;
#[cfg(test)]
use opencode_kit::client::{AgentInput, FileInput, Mention, SkillInput};
#[cfg(test)]
use wire::{absolute, encode_path, filename};

/// Session rows: Desktop shows at most 5 (`composer/model.ts:405`).
const SESSIONS: usize = 5;
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

/// A directory's subagents and skills, each `None` when its read failed.
type Catalog = (Attempt, Option<Vec<(String, String)>>, Option<Vec<Skill>>);

pub struct Mentions {
    /// Subagents (`mode != primary`, not hidden) by name, with their descriptions.
    pub agents: Vec<(String, String)>,
    pub skills: Vec<Skill>,
    /// Whose `agents` and `skills` these are.
    catalog: search::Catalog,
    catalog_fetch: Latest<Catalog>,
    /// The open list's file and session searches.
    pub search: search::Search,
    pub(crate) seq: Seq,
    /// Sessions the server found for a query's first word.
    found: Vec<SessionInfo>,
    files_fetch: Latest<(Attempt, Option<Vec<String>>)>,
    sessions_fetch: Latest<(Attempt, Option<Vec<SessionInfo>>)>,
    /// The fixture's files, searched locally.
    pub file_index: Vec<String>,
    /// What the rows are matched against, kept until what they come from changes.
    cache: std::cell::RefCell<index::Cache>,
}

impl Default for Mentions {
    fn default() -> Self {
        Mentions {
            agents: Vec::new(),
            skills: Vec::new(),
            catalog: search::Catalog::default(),
            catalog_fetch: Latest::default(),
            search: search::Search::default(),
            seq: Seq::default(),
            found: Vec::new(),
            files_fetch: Latest::default(),
            sessions_fetch: Latest::default(),
            file_index: Vec::new(),
            cache: Default::default(),
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

    /// There's a server to search; the fixture's files and sessions are searched here.
    pub(crate) fn mention_server(&self) -> bool {
        matches!(self.source, Source::Live { .. })
    }

    /// The open `@` list's query.
    pub fn mention_query(&self) -> Option<String> {
        self.suggest.state.context().map(|l| l.query.clone())
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

    /// The highlighted row.
    pub fn mention_active(&self) -> Option<String> {
        self.suggest.state.context().and_then(|l| l.active.id()).map(str::to_owned)
    }

    /// The row's pill replaces the `@query` before the caret, with a space after it
    /// (`addMention`): from the last `@` before the caret that isn't a pill's, or at the caret
    /// when there's none.
    pub fn pick_mention(&mut self, key: &str) {
        let Some(item) = self.mention_items().into_iter().find(|i| i.key == key) else { return };
        let caret = self.caret();
        let pills: Vec<_> = self.composer.pills().map(|(r, _)| r).collect();
        let at = self.composer.text[..caret].rmatch_indices('@').map(|(i, _)| i).find(|i| !pills.iter().any(|r| r.start <= *i && *i < r.end)).unwrap_or(caret);
        self.composer.replace_with_pill(at..caret, item.key.clone(), &item.content);
        self.composer.insert(" ");
    }

    /// The open list's searches follow its query (`context.changed`, `context.closed`), and the
    /// directory's catalog is read once it opens.
    pub(crate) fn sync_mention_search(&mut self, tx: &mut Tx) {
        let now = tx.clock();
        let server = self.mention_server();
        let m = &mut self.mentions;
        let effects = match self.suggest.state.context() {
            Some(list) => search::step(&mut m.seq, &mut m.search, search::Event::Changed { query: &list.query, server, now }),
            None => search::step(&mut m.seq, &mut m.search, search::Event::Closed),
        };
        self.run_mention_search(effects, tx);
        let Source::Live { server, .. } = &self.source else { return };
        if self.suggest.state.context().is_none() {
            return;
        }
        let (server, directory) = (server.clone(), self.composer_directory());
        let opened = matches!(self.mentions.catalog, search::Catalog::Unread | search::Catalog::Failed { .. });
        let m = &mut self.mentions;
        let Some((attempt, directory)) = search::step_catalog(&mut m.seq, &mut m.catalog, search::CatalogEvent::Wanted { directory: &directory, opened }) else { return };
        // `/api/skill` answers 503 until the server's plugins load, which `get_ready` waits out.
        m.catalog_fetch.request(directory, move |directory| {
            let at = format!("location%5Bdirectory%5D={}", crate::switcher::encode(&directory));
            let agents = server.get(&format!("/api/agent?{at}")).ok().map(|v| subagents(&v));
            let skills = server.get_ready(&format!("/api/skill?{at}")).ok().map(|v| skills(&v));
            (attempt, agents, skills)
        });
    }

    fn run_mention_search(&mut self, effects: impl IntoIterator<Item = search::Effect>, tx: &mut Tx) {
        let Source::Live { server, .. } = &self.source else {
            for effect in effects {
                if let search::Effect::WakeAt(due) = effect {
                    tx.wake_at(due);
                }
            }
            return;
        };
        let server = server.clone();
        let directory = self.composer_directory();
        for effect in effects {
            match effect {
                search::Effect::WakeAt(due) => tx.wake_at(due),
                search::Effect::SearchFiles { attempt, query } => {
                    let (server, directory) = (server.clone(), directory.clone());
                    self.mentions.files_fetch.request(query, move |query| {
                        let path = format!(
                            "/api/fs/find?location%5Bdirectory%5D={}&query={}",
                            crate::switcher::encode(&directory),
                            crate::switcher::encode(&query)
                        );
                        (attempt, server.get(&path).ok().map(|v| found_paths(&v)))
                    });
                }
                search::Effect::SearchSessions { attempt, word } => {
                    let server = server.clone();
                    self.mentions.sessions_fetch.request(word, move |word| {
                        let path = format!("/api/session?parentID=null&archived=false&limit=50&search={}", crate::switcher::encode(&word));
                        (attempt, server.get(&path).ok().map(|v| model::parse_sessions(&v)))
                    });
                }
            }
        }
    }

    /// Takes in what the searches answered and the timers that came due; the open list's rows
    /// land with this frame, without fading in or sliding.
    pub fn poll_mentions(&mut self, tx: &mut Tx) -> bool {
        let now = tx.clock();
        let m = &mut self.mentions;
        let mut changed = false;
        if let Some((attempt, agents, skills)) = m.catalog_fetch.take() {
            // A failed read keeps what was there, as Desktop's location data does, rather than
            // settling on an empty list.
            let failed = agents.is_none() || skills.is_none();
            if matches!(&m.catalog, search::Catalog::Reading { attempt: a, .. } if *a == attempt) {
                m.agents = agents.unwrap_or(std::mem::take(&mut m.agents));
                m.skills = skills.unwrap_or(std::mem::take(&mut m.skills));
                changed = true;
            }
            search::step_catalog(&mut m.seq, &mut m.catalog, search::CatalogEvent::Answered { attempt, failed });
        }
        let mut effects = Vec::new();
        if let Some((attempt, paths)) = m.files_fetch.take() {
            let before = m.search.clone();
            effects.extend(search::step(&mut m.seq, &mut m.search, search::Event::Files { attempt, paths }));
            changed |= m.search != before;
        }
        if let Some((attempt, list)) = m.sessions_fetch.take() {
            if let Some(list) = list {
                upsert_sessions(&mut m.found, list);
                changed = true;
            }
            effects.extend(search::step(&mut m.seq, &mut m.search, search::Event::Sessions { attempt }));
        }
        if m.search.next_due().is_some_and(|due| now + 1e-9 >= due) {
            let before = m.search.clone();
            effects.extend(search::step(&mut m.seq, &mut m.search, search::Event::Due { now }));
            changed |= m.search != before;
        }
        self.run_mention_search(effects, tx);
        if changed && self.suggest.state.context().is_some() {
            tx.instant();
            self.refresh_suggestions(tx);
        }
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
    s.suggest.state.context()?;
    let width = cx.layout_size("composer").map_or(0.0, |[w, _]| w);
    let items = s.mention_items();
    let active = s.mention_active();
    let rows = items
        .iter()
        .map(|i| Row { id: i.key.clone(), key: format!("mention-{}", i.key), label: i.label.clone(), lead: Some(row_icon(p, i.kind, &i.label)), description: i.description.clone(), trail: None })
        .collect();
    let spec = Spec { key: "mentions", label: opencode_i18n::t!("rune.composer.mentions"), empty: opencode_i18n::t!("ui.promptInput.noMatchingItems"), width, boundary: list::boundary(cx), search: None };
    Some(popup(p, spec, rows, active.as_deref(), |s, key, tx| s.suggest_pointed(key, tx), |s, key, tx| s.suggest_pick(key, tx)))
}

#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
mod bench_tests;

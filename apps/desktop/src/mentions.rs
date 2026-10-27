//! `@` in the composer: Desktop's context popover (`composer/editor/interaction.ts`,
//! `suggestions/machine.ts`, `composer/model.ts`). Typing `@` after a space or at the start
//! opens a list of skills, subagents, sessions and the server's file search; picking one puts
//! an atomic pill in the text, and sending turns the pills into the prompt's `agents`, `skills`
//! and `files` and session tokens (`composer/request.ts`, `prompt-parts.ts`).

use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rune::{Color, El, Icon, Key, KeyEvent, PillStyle, Tx, col, icon, row, text};
use rune_theme_opencode::icons as ic;
use serde_json::Value;

use opencode_kit::client::{AgentInput, FileInput, Mention, SkillInput};
use opencode_kit::model::{self, SessionInfo};

use crate::palette::Pal;
use crate::view::{UI, title_of};
use crate::{App, Route, Source};

/// Session rows: Desktop shows 8 for a query and 5 without.
const SESSIONS_QUERY: usize = 8;
const SESSIONS_EMPTY: usize = 5;
/// `composerSuggestionMaxHeight` at full height: 8 px of padding, whole 28 px rows, and an
/// 18 px peek of the next.
const ROW_H: f32 = 28.0;
const PAD: f32 = 6.0;
const MAX_H: f32 = 306.0;
/// Desktop waits this long after the last keystroke before searching files.
const FILE_DEBOUNCE: Duration = Duration::from_millis(150);
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

enum Fetched {
    /// A directory's subagents (name, description) and skills.
    Catalog { directory: String, agents: Vec<(String, String)>, skills: Vec<Skill> },
    Sessions(Vec<SessionInfo>),
    /// `/api/fs/find`'s paths (relative to the directory, directories end in `/`) for a query.
    Files { query: String, paths: Vec<String> },
}

pub struct Mentions {
    /// The popover is open; its query is the text between the `@` and the caret.
    pub open: bool,
    /// The composer's text and caret when last looked at.
    seen: (String, usize),
    /// The highlighted row and the query it was picked under.
    active: Option<(String, String)>,
    /// Subagents (`mode != primary`, not hidden) by name, with their descriptions.
    pub agents: Vec<(String, String)>,
    pub skills: Vec<Skill>,
    /// The directory `agents` and `skills` belong to, once asked for.
    catalog: Option<String>,
    /// Sessions the server found for a query's first word.
    found: Vec<SessionInfo>,
    searched: Option<String>,
    generation: Arc<AtomicU64>,
    /// The last file search's query and paths, and the query asked for since.
    files: (String, Vec<String>),
    files_asked: Option<String>,
    files_generation: Arc<AtomicU64>,
    /// The fixture's files, searched locally.
    pub file_index: Vec<String>,
    fetched_tx: Sender<Fetched>,
    fetched: Receiver<Fetched>,
}

impl Default for Mentions {
    fn default() -> Self {
        let (fetched_tx, fetched) = channel();
        Mentions {
            open: false,
            seen: (String::new(), 0),
            active: None,
            agents: Vec::new(),
            skills: Vec::new(),
            catalog: None,
            found: Vec::new(),
            searched: None,
            generation: Arc::new(AtomicU64::new(0)),
            files: (String::new(), Vec::new()),
            files_asked: None,
            files_generation: Arc::new(AtomicU64::new(0)),
            file_index: Vec::new(),
            fetched_tx,
            fetched,
        }
    }
}

impl Mentions {
    /// Fixture data in the server's shapes: `agent_list` as `/api/agent`, `skills` as `/api/skill`.
    pub fn load(&mut self, fixture: &Value) {
        self.agents = subagents(&fixture["agent_list"]);
        self.skills = skills(&fixture["skills"]);
        self.file_index = found_paths(&fixture["files"]);
    }
}

/// `/api/agent`'s subagents: Desktop offers agents that aren't hidden or primary.
fn subagents(v: &Value) -> Vec<(String, String)> {
    let list = v.get("data").unwrap_or(v);
    list.as_array()
        .into_iter()
        .flatten()
        .filter(|a| a["hidden"].as_bool() != Some(true) && a["mode"].as_str() != Some("primary"))
        .filter_map(|a| Some((a["id"].as_str().or(a["name"].as_str())?.to_owned(), a["description"].as_str().unwrap_or("").to_owned())))
        .collect()
}

/// `/api/fs/find`'s paths.
fn found_paths(v: &Value) -> Vec<String> {
    let list = v.get("data").unwrap_or(v);
    list.as_array().into_iter().flatten().filter_map(|f| f["path"].as_str().or(f.as_str()).map(str::to_owned)).collect()
}

fn skills(v: &Value) -> Vec<Skill> {
    let list = v.get("data").unwrap_or(v);
    list.as_array()
        .into_iter()
        .flatten()
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
pub fn matches_session_title(title: &str, query: &str) -> bool {
    let words: Vec<String> = query.to_lowercase().split(' ').map(str::to_owned).collect();
    let title = title.to_lowercase();
    let titles: Vec<&str> = title.split_whitespace().collect();
    (0..titles.len()).any(|start| words.iter().enumerate().all(|(i, w)| titles.get(start + i).is_some_and(|t| t.starts_with(w.as_str()))))
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

/// `parseSessionMention`: an id the server turns back into a session.
fn session_id_ok(id: &str) -> bool {
    id.strip_prefix("ses_").is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// `path` under `directory` unless it's already absolute.
fn absolute(directory: &str, path: &str) -> String {
    let windows = path.as_bytes().get(1) == Some(&b':') && path.as_bytes()[0].is_ascii_alphabetic();
    if path.starts_with('/') || path.starts_with("\\\\") || windows {
        return path.to_owned();
    }
    format!("{}/{path}", directory.trim_end_matches(['/', '\\']))
}

/// `encodeFilePath`: each segment percent-encoded as `encodeURIComponent` does, `/` kept, and
/// a Windows drive rooted (`/C:/…`).
fn encode_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let drive = |s: &str| s.len() == 2 && s.as_bytes()[0].is_ascii_alphabetic() && s.as_bytes()[1] == b':';
    let rooted = if normalized.get(..2).is_some_and(drive) { format!("/{normalized}") } else { normalized };
    rooted
        .split('/')
        .enumerate()
        .map(|(i, seg)| {
            if i == 1 && drive(seg) {
                return seg.to_owned();
            }
            seg.bytes()
                .map(|b| match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => (b as char).to_string(),
                    _ => format!("%{b:02X}"),
                })
                .collect()
        })
        .collect::<Vec<String>>()
        .join("/")
}

/// `getFilename`: the last segment, ignoring trailing separators.
fn filename(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed)
}

/// What a prompt sends for its pills.
#[derive(Debug, Default, PartialEq)]
pub struct Wire {
    pub text: String,
    /// The text as typed (session pills show titles).
    pub display: String,
    pub agents: Vec<AgentInput>,
    pub skills: Vec<SkillInput>,
    /// File mentions as `file://` references the server reads.
    pub files: Vec<FileInput>,
}

/// The prompt for `text` (trimmed) and its pills (byte range, id). Session pills become
/// `@session:<id>`, moving the mentions after them; offsets count UTF-16 units. File paths
/// resolve against `directory`.
pub fn wire(text: &str, pills: &[(Range<usize>, String)], directory: &str, skill_name: impl Fn(&str) -> String) -> Wire {
    let trimmed = text.trim();
    let lead = text.len() - text.trim_start().len();
    let end = lead + trimmed.len();
    let mut pills: Vec<&(Range<usize>, String)> = pills.iter().filter(|(r, _)| r.start >= lead && r.end <= end && r.start < r.end).collect();
    pills.sort_by_key(|(r, _)| r.start);
    let mut out = Wire { display: trimmed.to_owned(), ..Default::default() };
    let utf16 = |s: &str| s.encode_utf16().count();
    let mut at = lead;
    for (range, id) in pills {
        if range.start < at {
            continue;
        }
        out.text.push_str(&text[at..range.start]);
        let shown = &text[range.clone()];
        let (kind, value) = id.split_once(':').unwrap_or(("", id));
        let content = match kind {
            "session" if session_id_ok(value) => format!("@session:{value}"),
            _ => shown.to_owned(),
        };
        let start = utf16(&out.text);
        out.text.push_str(&content);
        let mention = Some(Mention { start, end: start + utf16(&content), text: content.clone() });
        match kind {
            "agent" => out.agents.push(AgentInput { name: value.to_owned(), mention }),
            "skill" => out.skills.push(SkillInput { id: value.to_owned(), name: skill_name(value), mention }),
            "file" => out.files.push(FileInput {
                uri: format!("file://{}", encode_path(&absolute(directory, value))),
                name: filename(value).to_owned(),
                mime: Some("text/plain".into()),
                mention,
            }),
            _ => {}
        }
        at = range.end;
    }
    out.text.push_str(&text[at..end]);
    out
}

impl App {
    /// The composer's directory: the open session's, or where a new one starts.
    pub fn composer_directory(&self) -> String {
        match &self.route {
            Route::Session(id) => self.session(id).map(|s| s.directory.clone()).unwrap_or_default(),
            _ => self.draft_dir.clone(),
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

    /// After the composer's text changes: the popover opens or closes with this frame.
    pub fn mentions_changed(&mut self, tx: &mut Tx) {
        let was = self.mentions.open;
        self.sync_mentions();
        if was != self.mentions.open {
            tx.instant();
        }
    }

    /// The `@`'s byte offset and the query, when the text before the caret has one that isn't
    /// already a pill's.
    fn mention_trigger(&self, open: bool) -> Option<(usize, String)> {
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
        let mut all: Vec<&SessionInfo> = Vec::new();
        for s in self.sessions.iter().filter(|s| s.parent.is_none()).chain(&self.mentions.found) {
            if Some(s.id.as_str()) == current || !session_id_ok(&s.id) {
                continue;
            }
            match all.iter_mut().find(|x| x.id == s.id) {
                Some(old) if s.updated > old.updated => *old = s,
                Some(_) => {}
                None => all.push(s),
            }
        }
        all.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
        all
    }

    /// The popover's rows: skills, then agents (both fuzzy on their labels), then sessions whose
    /// titles match. A query with a space only lists sessions.
    pub fn mention_items(&self) -> Vec<Item> {
        let Some(query) = self.mention_query() else { return Vec::new() };
        let sessions: Vec<Item> = self
            .mention_sessions()
            .into_iter()
            .filter(|s| query.is_empty() || matches_session_title(&title_of(s), &query))
            .take(if query.is_empty() { SESSIONS_EMPTY } else { SESSIONS_QUERY })
            .map(|s| {
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
        let needle = query.to_lowercase();
        let ranked = |items: Vec<Item>| -> Vec<Item> {
            let mut scored: Vec<(i64, Item)> = items.into_iter().filter_map(|i| Some((fuzzy(&needle, &i.label)?, i))).collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0));
            scored.into_iter().map(|(_, i)| i).collect()
        };
        let skills = self.mentions.skills.iter().map(|s| Item {
            key: format!("skill:{}", s.id),
            kind: Kind::Skill,
            label: format!("@{}", s.id),
            description: s.description.clone(),
            content: format!("@{}", s.id),
        });
        let agents = self.mentions.agents.iter().map(|(name, _)| Item {
            key: format!("agent:{name}"),
            kind: Kind::Agent,
            label: format!("@{name}"),
            description: String::new(),
            content: format!("@{name}"),
        });
        let mut out = ranked(skills.collect());
        out.extend(ranked(agents.collect()));
        out.extend(sessions);
        out.extend(self.mention_files(&query).into_iter().map(|path| Item {
            key: format!("file:{path}"),
            kind: Kind::File,
            label: path.clone(),
            description: String::new(),
            content: format!("@{path}"),
        }));
        out
    }

    /// Files for a query: the fixture's, searched here, or the server's latest answer. Until
    /// the server answers a new query, the last answer stays, narrowed to what still matches.
    fn mention_files(&self, query: &str) -> Vec<String> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let needle = query.to_lowercase();
        let narrow = |paths: &[String]| -> Vec<String> {
            let mut scored: Vec<(i64, &String)> = paths.iter().filter_map(|p| Some((fuzzy(&needle, p)?, p))).collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0));
            scored.into_iter().take(FILE_LIMIT).map(|(_, p)| p.clone()).collect()
        };
        match &self.source {
            Source::Live { .. } if self.mentions.files.0 == query => self.mentions.files.1.clone(),
            Source::Live { .. } => narrow(&self.mentions.files.1),
            _ => narrow(&self.mentions.file_index),
        }
    }

    /// The highlighted row: the one picked under this query, else the first.
    pub fn mention_active(&self, items: &[Item]) -> Option<String> {
        let query = self.mention_query().unwrap_or_default();
        let picked = self.mentions.active.as_ref().filter(|(_, q)| *q == query).map(|(k, _)| k);
        picked.filter(|k| items.iter().any(|i| &i.key == *k)).cloned().or_else(|| items.first().map(|i| i.key.clone()))
    }

    pub fn mention_activate(&mut self, key: String) {
        self.mentions.active = Some((key, self.mention_query().unwrap_or_default()));
    }

    /// Replaces the `@query` with the item's pill and a space, caret after it.
    pub fn pick_mention(&mut self, key: &str) {
        let items = self.mention_items();
        let Some(item) = items.iter().find(|i| i.key == key) else { return };
        let Some((at, _)) = self.mention_trigger(true) else { return };
        let caret = self.caret();
        self.composer.replace_with_pill(at..caret, item.key.clone(), &item.content);
        self.composer.insert(" ");
        self.mentions.open = false;
        self.mentions.active = None;
        self.mentions.seen = (self.composer.text.clone(), self.caret());
    }

    pub fn close_mentions(&mut self) {
        self.mentions.open = false;
        self.mentions.active = None;
    }

    /// The composer's keys while the popover is open: arrows and Ctrl-N/P move and wrap, Tab
    /// and Enter pick, Escape closes.
    pub fn mention_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        self.sync_mentions();
        if !self.mentions.open {
            return false;
        }
        let ctrl = |c: &str| ev.mods.ctrl && !ev.mods.cmd && !ev.mods.alt && matches!(&ev.key, Key::Text(t) if t.eq_ignore_ascii_case(c));
        let step = match &ev.key {
            Key::Down if !ev.mods.alt && !ev.mods.cmd && !ev.mods.shift => Some(1),
            Key::Up if !ev.mods.alt && !ev.mods.cmd && !ev.mods.shift => Some(-1),
            _ if ctrl("n") => Some(1),
            _ if ctrl("p") => Some(-1),
            _ => None,
        };
        if let Some(step) = step {
            let items = self.mention_items();
            if items.is_empty() {
                return true;
            }
            let active = self.mention_active(&items);
            let i = items.iter().position(|it| Some(&it.key) == active.as_ref()).unwrap_or(0) as i64;
            let n = items.len() as i64;
            let key = items[(i + step).rem_euclid(n) as usize].key.clone();
            tx.instant();
            tx.scroll_into_view_in("mentions-scroll", format!("mention-{key}"));
            self.mention_activate(key);
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
    pub fn poll_mentions(&mut self) -> bool {
        let mut changed = false;
        for f in self.mentions.fetched.try_iter() {
            match f {
                Fetched::Catalog { directory, agents, skills } => {
                    if self.mentions.catalog.as_deref() == Some(directory.as_str()) {
                        self.mentions.agents = agents;
                        self.mentions.skills = skills;
                    }
                }
                Fetched::Files { query, paths } => {
                    if self.mentions.files_asked.as_deref() == Some(query.as_str()) {
                        self.mentions.files = (query, paths);
                    }
                }
                Fetched::Sessions(list) => {
                    for s in list {
                        match self.mentions.found.iter_mut().find(|x| x.id == s.id) {
                            Some(old) => *old = s,
                            None => self.mentions.found.push(s),
                        }
                    }
                }
            }
            changed = true;
        }
        let was = self.mentions.open;
        self.sync_mentions();
        changed |= was != self.mentions.open;
        let Some(query) = self.mention_query() else {
            self.mentions.searched = None;
            return changed;
        };
        let Source::Live { server, .. } = &self.source else { return changed };
        let (server, wake) = (server.clone(), self.waker.clone());
        let directory = self.composer_directory();
        if self.mentions.catalog.as_deref() != Some(directory.as_str()) {
            self.mentions.catalog = Some(directory.clone());
            self.mentions.files = (String::new(), Vec::new());
            self.mentions.files_asked = None;
            let out = self.mentions.fetched_tx.clone();
            let (server, wake, directory) = (server.clone(), wake.clone(), directory.clone());
            std::thread::spawn(move || {
                let at = format!("location%5Bdirectory%5D={}", crate::switcher::encode(&directory));
                let agents = server.get(&format!("/api/agent?{at}")).map(|v| subagents(&v)).unwrap_or_default();
                let skills = server.get(&format!("/api/skill?{at}")).map(|v| skills(&v)).unwrap_or_default();
                let _ = out.send(Fetched::Catalog { directory, agents, skills });
                if let Some(w) = wake {
                    w();
                }
            });
        }
        if !query.trim().is_empty() && !query.contains(char::is_whitespace) && self.mentions.files_asked.as_deref() != Some(query.as_str()) {
            self.mentions.files_asked = Some(query.clone());
            let generation = self.mentions.files_generation.fetch_add(1, Ordering::Relaxed) + 1;
            let (out, current) = (self.mentions.fetched_tx.clone(), self.mentions.files_generation.clone());
            let (server, wake, directory, query) = (server.clone(), wake.clone(), directory.clone(), query.clone());
            std::thread::spawn(move || {
                std::thread::sleep(FILE_DEBOUNCE);
                if current.load(Ordering::Relaxed) != generation {
                    return;
                }
                let path = format!(
                    "/api/fs/find?location%5Bdirectory%5D={}&query={}",
                    crate::switcher::encode(&directory),
                    crate::switcher::encode(&query)
                );
                let paths = server.get(&path).map(|v| found_paths(&v)).unwrap_or_default();
                if current.load(Ordering::Relaxed) != generation {
                    return;
                }
                let _ = out.send(Fetched::Files { query, paths });
                if let Some(w) = wake {
                    w();
                }
            });
        }
        let word = query.split(' ').next().unwrap_or("").to_owned();
        if word.is_empty() || self.mentions.searched.as_deref() == Some(word.as_str()) {
            return changed;
        }
        self.mentions.searched = Some(word.clone());
        let generation = self.mentions.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let (out, current) = (self.mentions.fetched_tx.clone(), self.mentions.generation.clone());
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            if current.load(Ordering::Relaxed) != generation {
                return;
            }
            let path = format!("/api/session?parentID=null&archived=false&limit=50&search={}", crate::switcher::encode(&word));
            if let Ok(v) = server.get(&path) {
                let _ = out.send(Fetched::Sessions(model::parse_sessions(&v)));
                if let Some(w) = wake {
                    w();
                }
            }
        });
        changed
    }

    /// The prompt's text and mentions for what's in the composer.
    pub fn mention_wire(&self) -> Wire {
        let pills: Vec<(Range<usize>, String)> = self.composer.pills().map(|(r, id)| (r, id.to_owned())).collect();
        wire(&self.composer.text, &pills, &self.composer_directory(), |id| self.mentions.skills.iter().find(|s| s.id == id).map_or_else(|| id.to_owned(), |s| s.name.clone()))
    }
}

// ---------------------------------------------------------------------------------------------
// View

/// Desktop's `speech-bubble` and `post-skill` (`ui/src/icons/icon/additional-icons.ts`).
static SPEECH_BUBBLE: Icon = Icon {
    name: "speech-bubble",
    view: 20.0,
    body: r#"<path d="M18.3334 10.0003C18.3334 5.57324 15.0927 2.91699 10.0001 2.91699C4.90749 2.91699 1.66675 5.57324 1.66675 10.0003C1.66675 11.1497 2.45578 13.1016 2.5771 13.3949C2.5878 13.4207 2.59839 13.4444 2.60802 13.4706C2.69194 13.6996 3.04282 14.9364 1.66675 16.7684C3.5186 17.6538 5.48526 16.1982 5.48526 16.1982C6.84592 16.9202 8.46491 17.0837 10.0001 17.0837C15.0927 17.0837 18.3334 14.4274 18.3334 10.0003Z" stroke="currentColor" stroke-linecap="square"/>"#,
};
static POST_SKILL: Icon = Icon {
    name: "post-skill",
    view: 20.0,
    body: r#"<rect x="2.5" y="3.5" width="15" height="13" rx="1.5" stroke="currentColor"/><path d="M5.5 7.5H10.5M5.5 10.5H14.5" stroke="currentColor"/>"#,
};

static FILE_DOCUMENT: Icon = Icon {
    name: "file-document",
    view: 14.0,
    body: r#"<path d="M7.58331 2.33333V5.25H10.5M2.91664 1.75H7.58331L11.0833 5.25V12.25H2.91664V1.75Z" stroke="currentColor" stroke-linecap="square"/>"#,
};

/// Colours the shared palette doesn't carry: `syntax-type` for agents, `syntax-property` for
/// files, `icon-info-active`.
struct Ink {
    agent: Color,
    file: Color,
    info: Color,
}

fn ink(dark: bool) -> &'static Ink {
    static DARK: OnceLock<Ink> = OnceLock::new();
    static LIGHT: OnceLock<Ink> = OnceLock::new();
    let build = || {
        let hex = |s: &str| rune::color::hex(s).unwrap_or(Color::BLACK);
        let theme = rune::Theme::from_json(include_str!("../assets/theme.json"), dark).unwrap_or_else(|e| panic!("{e}"));
        Ink {
            agent: if dark { hex("#ecf58c") } else { hex("#596600") },
            file: if dark { hex("#ff9ae2") } else { hex("#ed6dc8") },
            info: theme.color("state-fg-info"),
        }
    };
    if dark { DARK.get_or_init(build) } else { LIGHT.get_or_init(build) }
}

/// A pill's look by its id. Desktop colours agent and file mentions' text and leaves skills
/// plain, with no chip; a session gets a raised chip.
pub fn pill_style(p: &Pal) -> impl Fn(&str) -> PillStyle + 'static {
    let (dark, text, layer2) = (p.dark, p.text, p.layer2);
    move |id: &str| {
        let k = ink(dark);
        match id.split_once(':').map(|(kind, _)| kind) {
            Some("agent") => PillStyle::new().bg(Color::TRANSPARENT).fg(k.agent),
            Some("file") => PillStyle::new().bg(Color::TRANSPARENT).fg(k.file),
            Some("session") => PillStyle::new().bg(layer2).fg(text),
            _ => PillStyle::new().bg(Color::TRANSPARENT).fg(text),
        }
    }
}

fn row_icon(p: &Pal, kind: Kind) -> El {
    match kind {
        Kind::Session => icon(&SPEECH_BUBBLE, 16.0).color(p.icon),
        Kind::Agent => icon(&ic::BRAIN, 16.0).color(ink(p.dark).info),
        Kind::Skill => icon(&POST_SKILL, 16.0).color(p.icon),
        Kind::File => icon(&FILE_DOCUMENT, 16.0).color(p.icon),
    }
}

/// The popover over the composer, full width, 8 points up: rows of icon, label and a muted
/// description. It appears and goes at once.
pub fn view(s: &App, p: &Pal, width: f32) -> Option<El> {
    if !s.mentions.open || s.mention_query().is_none() {
        return None;
    }
    let items = s.mention_items();
    let active = s.mention_active(&items);
    let mut list = col().key("mentions-list").w_full();
    for item in &items {
        let key = item.key.clone();
        let hover = item.key.clone();
        let mut words = row().key("words").grow(1.0).shrink().min_w(0.0).gap(8.0).center_y().child(row_icon(p, item.kind))
            .child(text(item.label.clone()).font(UI).color(p.text));
        if !item.description.is_empty() {
            words = words.child(text(item.description.clone()).font(UI).color(p.muted).truncate().shrink().min_w(0.0));
        }
        list = list.child(
            row()
                .key(format!("mention-{}", item.key))
                .role("option")
                .label(item.label.clone())
                .w_full()
                .h(ROW_H)
                .px(8.0)
                .center_y()
                .radius(6.0)
                .fill(if active.as_deref() == Some(item.key.as_str()) { p.hover } else { Color::TRANSPARENT })
                .on_hover(move |s: &mut App| s.mention_activate(hover.clone()))
                .on_press(move |s: &mut App| {
                    s.pick_mention(&key);
                    s.composer.focused = true;
                })
                .child(words),
        );
    }
    let body = if items.is_empty() {
        col().key("mentions-scroll").w_full().child(text("No matching items").font(UI).color(p.muted).px(8.0).py(4.0))
    } else {
        let h = (items.len() as f32 * ROW_H).min(MAX_H - 2.0 * PAD);
        col().key("mentions-scroll").w_full().h(h).scroll(rune::ScrollSpec { stick_bottom: false }).child(list)
    };
    Some(
        col()
            .key("mentions")
            .role("listbox")
            .label("Mentions")
            .above("composer", 8.0)
            .w(width)
            .padding(PAD)
            .radius(12.0)
            .fill(p.bg_base)
            .border(0.5, p.ring)
            .shadow(4.0, 12.0, p.shadow)
            .transition(rune::Transition::opacity().springs(crate::view::no_motion()))
            .on_press(|_: &mut App| {})
            .child(body),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime};

    #[test]
    fn the_trigger_follows_desktops_patterns() {
        assert_eq!(trigger("@", false), Some((0, "")));
        assert_eq!(trigger("ask @exp", false), Some((4, "exp")));
        assert_eq!(trigger("mail@exp", false), None);
        assert_eq!(trigger("@fix the", false), None);
        assert_eq!(trigger("@fix the", true), Some((0, "fix the")));
        assert_eq!(trigger("@fix the ", true), Some((0, "fix the ")));
        assert_eq!(trigger("@fix  the", true), None);
        assert_eq!(trigger("@ fix", true), None);
        assert_eq!(trigger("a\n@x", false), Some((2, "x")));
    }

    #[test]
    fn session_titles_match_word_starts() {
        assert!(matches_session_title("Fix flaky reconnect", "fla rec"));
        assert!(matches_session_title("Fix flaky reconnect", "FIX"));
        assert!(!matches_session_title("Fix flaky reconnect", "fix rec"));
        assert!(!matches_session_title("HOME", "home "));
        assert!(matches_session_title("HOME base", "home "));
    }

    #[test]
    fn fuzzy_prefers_substrings() {
        assert!(fuzzy("exp", "@explore").unwrap() > fuzzy("epl", "@explore").unwrap());
        assert!(fuzzy("xyz", "@explore").is_none());
    }

    #[test]
    fn the_wire_rewrites_sessions_and_counts_utf16() {
        let text = "  é @explore then @Fix it and @opencode  ";
        let a = text.find("@explore").unwrap();
        let s = text.find("@Fix it").unwrap();
        let k = text.find("@opencode").unwrap();
        let pills = vec![
            (s..s + "@Fix it".len(), "session:ses_abc123".to_owned()),
            (a..a + "@explore".len(), "agent:explore".to_owned()),
            (k..k + "@opencode".len(), "skill:opencode".to_owned()),
        ];
        let w = wire(text, &pills, "/repo", |id| if id == "opencode" { "OpenCode".into() } else { id.into() });
        assert_eq!(w.text, "é @explore then @session:ses_abc123 and @opencode");
        assert_eq!(w.display, "é @explore then @Fix it and @opencode");
        let m = |start, end, t: &str| Some(Mention { start, end, text: t.into() });
        assert_eq!(w.agents, vec![AgentInput { name: "explore".into(), mention: m(2, 10, "@explore") }]);
        let k16 = "é @explore then @session:ses_abc123 and ".encode_utf16().count();
        assert_eq!(w.skills, vec![SkillInput { id: "opencode".into(), name: "OpenCode".into(), mention: m(k16, k16 + 9, "@opencode") }]);
    }

    #[test]
    fn file_mentions_go_out_as_file_urls() {
        let text = "read @src/a b.rs and @/etc/hosts";
        let a = text.find("@src").unwrap();
        let e = text.find("@/etc").unwrap();
        let pills = vec![
            (a..a + "@src/a b.rs".len(), "file:src/a b.rs".to_owned()),
            (e..text.len(), "file:/etc/hosts".to_owned()),
        ];
        let w = wire(text, &pills, "/repo/", |id| id.into());
        let m = |start, end, t: &str| Some(Mention { start, end, text: t.into() });
        assert_eq!(
            w.files,
            vec![
                FileInput { uri: "file:///repo/src/a%20b.rs".into(), name: "a b.rs".into(), mime: Some("text/plain".into()), mention: m(5, 16, "@src/a b.rs") },
                FileInput { uri: "file:///etc/hosts".into(), name: "hosts".into(), mime: Some("text/plain".into()), mention: m(21, 32, "@/etc/hosts") },
            ]
        );
        assert_eq!(w.text, text);
        assert_eq!(encode_path("C:\\x\\é#.rs"), "/C:/x/%C3%A9%23.rs");
        assert_eq!(filename("src/stream/"), "stream");
        assert_eq!(absolute("/repo", "C:/x"), "C:/x");
    }

    fn runtime() -> Runtime<App> {
        let mut app = App::empty();
        app.load_fixture(crate::FIXTURE);
        crate::open_scene(&mut app);
        app.finish_stream();
        let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
        rt.on_key = Some(crate::on_key);
        rt.poll = Some(crate::poll);
        rt.frame(0.0);
        rt
    }
    fn key(rt: &mut Runtime<App>, key: Key) {
        rt.key(KeyEvent { key, mods: Default::default() });
        rt.frame(1.0 / 60.0);
    }
    /// One keystroke a frame, as a person types.
    fn typed(rt: &mut Runtime<App>, s: &str) {
        for c in s.chars() {
            key(rt, Key::Text(c.to_string()));
        }
    }
    fn tap(rt: &mut Runtime<App>, target: &str) {
        let (x, y) = rt.find(target).unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }
    fn keys(rt: &Runtime<App>) -> Vec<String> {
        rt.state.mention_items().into_iter().map(|i| i.key).collect()
    }
    fn shown(rt: &Runtime<App>, key: &str) -> Option<rune::ElementInfo> {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key))
    }

    #[test]
    fn at_lists_skills_agents_and_sessions_and_filters() {
        let mut rt = runtime();
        tap(&mut rt, "prompt");
        typed(&mut rt, "ask @");
        let popover = shown(&rt, "mentions").expect("the popover opens on @");
        assert!((popover.opacity - 1.0).abs() < 1e-3, "no fade");
        let composer = shown(&rt, "composer").unwrap();
        assert!((popover.target[1] + popover.target[3] + 8.0 - composer.target[1]).abs() < 0.1, "8 points above");
        assert!((popover.target[2] - composer.target[2]).abs() < 0.1, "full width");
        let all = keys(&rt);
        assert_eq!(&all[..3], ["skill:opencode", "agent:general", "agent:explore"]);
        // The open session isn't offered; the five newest others are.
        assert_eq!(all.iter().filter(|k| k.starts_with("session:")).count(), 5);
        assert!(!all.contains(&"session:ses_fx0001aaaaaaaaaaaaaaaaaa".to_owned()));
        assert_eq!(rt.state.mention_active(&rt.state.mention_items()).as_deref(), Some("skill:opencode"));

        typed(&mut rt, "exp");
        let found = keys(&rt);
        assert_eq!(found[0], "agent:explore");
        assert!(found[1..].iter().all(|k| k.starts_with("session:")), "{found:?}");
        assert!(found.contains(&"session:ses_fx0008aaaaaaaaaaaaaaaaaa".to_owned()), "\"Explain …\" starts with exp");
        typed(&mut rt, "zzz");
        assert!(keys(&rt).is_empty());
        assert!(shown(&rt, "mentions").is_some(), "an empty list still shows its label");
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.composer.text, "ask @expzzz", "Enter with nothing to pick neither picks nor sends");
        assert_eq!(rt.state.sends, 0);
        key(&mut rt, Key::Escape);
        assert!(shown(&rt, "mentions").is_none());
        assert!(rt.state.composer.focused);
    }

    #[test]
    fn multi_word_queries_list_sessions_only() {
        let mut rt = runtime();
        tap(&mut rt, "prompt");
        typed(&mut rt, "@prof");
        assert_eq!(keys(&rt), ["session:ses_fx0005aaaaaaaaaaaaaaaaaa"]);
        typed(&mut rt, " scr");
        assert_eq!(keys(&rt), ["session:ses_fx0005aaaaaaaaaaaaaaaaaa"]);
        typed(&mut rt, "x");
        assert!(keys(&rt).is_empty());
        // Closed, a space ends the mention.
        key(&mut rt, Key::Escape);
        typed(&mut rt, " ");
        assert!(!rt.state.mentions.open);
    }

    #[test]
    fn keys_move_wrap_and_pick_into_a_pill() {
        let mut rt = runtime();
        tap(&mut rt, "prompt");
        typed(&mut rt, "hi @");
        let all = keys(&rt);
        key(&mut rt, Key::Up);
        assert_eq!(rt.state.mention_active(&rt.state.mention_items()).as_deref(), Some(all.last().unwrap().as_str()), "up wraps");
        key(&mut rt, Key::Down);
        rt.key(KeyEvent { key: Key::Text("n".into()), mods: rune::Mods { ctrl: true, ..Default::default() } });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.mention_active(&rt.state.mention_items()).as_deref(), Some("agent:general"));
        key(&mut rt, Key::Tab);
        assert_eq!(rt.state.composer.text, "hi @general ");
        assert!(!rt.state.mentions.open);
        assert_eq!(rt.state.composer.pills().map(|(r, id)| (r, id.to_owned())).collect::<Vec<_>>(), [(3..11, "agent:general".to_owned())]);
        assert_eq!(rt.state.composer.focus, 12);
        assert!(rt.state.pickers.toolbar_focus.is_none(), "Tab picked instead of moving to the toolbar");

        // Backspace takes the space, then the whole pill, and never reopens the popover on it.
        key(&mut rt, Key::Backspace);
        assert!(!rt.state.mentions.open);
        key(&mut rt, Key::Backspace);
        assert_eq!(rt.state.composer.text, "hi ");

        typed(&mut rt, "@fix foo");
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.composer.text, "hi @Fix footnote numbering in exported PDFs ");
        assert_eq!(rt.state.composer.pills().next().map(|(_, id)| id.to_owned()).as_deref(), Some("session:ses_fx0010aaaaaaaaaaaaaaaaaa"));
    }

    #[test]
    fn files_follow_sessions_and_pick_into_a_pill() {
        let mut rt = runtime();
        tap(&mut rt, "prompt");
        typed(&mut rt, "@");
        assert!(keys(&rt).iter().all(|k| !k.starts_with("file:")), "no file search without a query");
        typed(&mut rt, "rec");
        let found = keys(&rt);
        let first_file = found.iter().position(|k| k.starts_with("file:")).expect("files listed");
        assert!(found[..first_file].iter().all(|k| !k.starts_with("file:")));
        assert!(found[first_file..].iter().all(|k| k.starts_with("file:")), "{found:?}");
        assert_eq!(found[first_file], "file:src/stream/reconnect.rs");
        typed(&mut rt, "onnect.rs");
        assert_eq!(keys(&rt), ["file:src/stream/reconnect.rs"]);
        let row = shown(&rt, "mention-file:src/stream/reconnect.rs").expect("the row shows");
        assert!(row.target[3] > 0.0);
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.composer.text, "@src/stream/reconnect.rs ");
        let w = rt.state.mention_wire();
        assert_eq!(
            w.files,
            vec![FileInput {
                uri: "file:///Users/demo/code/lumen/src/stream/reconnect.rs".into(),
                name: "reconnect.rs".into(),
                mime: Some("text/plain".into()),
                mention: Some(Mention { start: 0, end: 24, text: "@src/stream/reconnect.rs".into() }),
            }]
        );
    }

    #[test]
    fn sending_carries_the_mentions() {
        let mut rt = runtime();
        rt.state.route = Route::Session("ses_fx0003aaaaaaaaaaaaaaaaaa".into());
        rt.frame(1.0 / 60.0);
        tap(&mut rt, "prompt");
        typed(&mut rt, "@exp");
        key(&mut rt, Key::Enter);
        typed(&mut rt, "look at @opencode");
        key(&mut rt, Key::Enter);
        typed(&mut rt, "and @fix flaky");
        key(&mut rt, Key::Enter);
        let w = rt.state.mention_wire();
        assert_eq!(w.text, "@explore look at @opencode and @session:ses_fx0001aaaaaaaaaaaaaaaaaa");
        assert_eq!(w.display, "@explore look at @opencode and @Fix flaky reconnect in the event stream");
        assert_eq!(w.agents, vec![AgentInput { name: "explore".into(), mention: Some(Mention { start: 0, end: 8, text: "@explore".into() }) }]);
        assert_eq!(w.skills, vec![SkillInput { id: "opencode".into(), name: "OpenCode".into(), mention: Some(Mention { start: 17, end: 26, text: "@opencode".into() }) }]);
        key(&mut rt, Key::Enter);
        assert_eq!(rt.state.sends, 1);
        assert!(rt.state.composer.text.is_empty());
        let t = &rt.state.transcripts["ses_fx0003aaaaaaaaaaaaaaaaaa"];
        assert!(matches!(t.messages.last(), Some(opencode_kit::model::Message::User { text, .. }) if text.ends_with("@Fix flaky reconnect in the event stream")), "the echo shows what was typed");
    }
}

//! ⌘K, ⇧⌘P and ⌘P, Desktop's "Search everything" palette (`shell/commands/{dialog,palette,search}.ts(x)`,
//! `ui/forms/search-list`): commands, files and sessions in one list.
//!
//! In a session it is `DialogCommandPalette`: empty, Desktop's preferred commands, the files open
//! in the side panel and the five newest sessions; typed, the matching commands, sessions and
//! files the server finds. On Home it is `HomeCommandPalette`: the first commands and the
//! sessions, no files. Search commands opens it on commands alone (`DialogCommandSearch`), ⇧⌘F
//! on sessions (`DialogSessionSearch`); the commands are `commands.rs`'s list.
//!
//! Sessions match on title, project and id, and the server finds title matches past the loaded
//! list (`/api/session?search=`, after a 150 ms pause, `PALETTE_SESSION_DEBOUNCE_MS`). A typed
//! query ranks commands first, then sessions (open tabs, then the newest), then files
//! (`rankCommandPaletteEntries`).

use crate::pickers::PickerHost as _;
use std::collections::HashMap;
use std::time::Duration;

use rune::{Key, KeyEvent, Latest, TextEdit, Tx, Waker};

use opencode_kit::model::{self, SessionInfo};

use crate::files::data::{fetch_live_find, file_tab_path, parse_path_and_line};
use crate::keymap::Bind;
use crate::mentions::list::{Highlight, cycle, upsert_sessions};
use crate::view::title_of;
use crate::{App, Route, Source};

mod view;
pub use view::{dialog, view};

/// `ENTRY_LIMIT`: the empty query's commands, files and recent sessions; `DialogSessionSearch`'s
/// recent sessions (`RECENT_LIMIT`); and a query's local session matches (`ENTRY_LIMIT * 10`).
const RECENT: usize = 5;
const RECENT_SESSIONS: usize = 10;
const MATCHES: usize = 50;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SwitcherMode {
    #[default]
    All,
    Sessions,
    /// Search commands: commands only.
    Commands,
    /// The Inbox's Archived button: archived sessions, each restored as it opens
    /// (`DialogSessionSearch archived`).
    Archived,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Session(String),
    /// A command's id in `commands.rs`.
    Command(&'static str),
    /// A workspace file's path, from the session's folder.
    File(String),
    /// A Settings row or page, by its settings search id (settings/palette.ts).
    Setting(String),
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub key: String,
    pub action: Action,
    pub title: String,
    /// A session's project id, for its icon.
    pub project: String,
    /// A session's project, a command's description.
    pub description: String,
    /// A command's keymap entry, whose chord the row shows.
    pub hint: Option<crate::keymap::Bind>,
    /// A plugin command's chord, which has no keymap entry.
    pub chord: Option<rune::Chord>,
    pub group: &'static str,
    pub updated: Option<i64>,
    /// The session is open as a tab.
    pub open: bool,
}

/// A server search's answer.
struct Found {
    query: String,
    sessions: Vec<SessionInfo>,
    failed: bool,
}

pub struct Switcher {
    pub open: bool,
    pub mode: SwitcherMode,
    pub search: TextEdit,
    /// The highlighted row and the query it was picked under; a new query starts from its first row.
    active: Highlight,
    /// The row the pointer took and the query it took it under, and the pointer's last point on
    /// a row (`SearchList`'s `pointed`): late results leave the list where it is for that row.
    pointed: Option<(String, String)>,
    pointer: Option<[f32; 2]>,
    restore_composer: bool,
    /// Sessions the server found while this was open, kept for later keystrokes.
    pub(crate) loaded: Vec<SessionInfo>,
    /// The query last sent to the server and the one that failed.
    searched: String,
    failed: Option<String>,
    fetch: Latest<Found>,
    /// The archived search has asked for its first list.
    listed_archived: bool,
    /// The files the server found for a query, and that query (`file.searchFiles`).
    pub(crate) files: (String, Vec<String>),
    /// The file query last sent.
    files_asked: Option<String>,
    files_fetch: Latest<(String, Vec<String>)>,
}

impl Default for Switcher {
    fn default() -> Self {
        Switcher {
            open: false,
            mode: SwitcherMode::All,
            search: TextEdit::new(""),
            active: Highlight::default(),
            pointed: None,
            pointer: None,
            restore_composer: false,
            loaded: Vec::new(),
            searched: String::new(),
            failed: None,
            fetch: Latest::default().debounce(Duration::from_millis(150)),
            listed_archived: false,
            files: (String::new(), Vec::new()),
            files_asked: None,
            files_fetch: Latest::default(),
        }
    }
}

impl Switcher {
    pub fn set_waker(&mut self, waker: Waker) {
        self.fetch = Latest::new(waker.clone()).debounce(Duration::from_millis(150));
        self.files_fetch = Latest::new(waker);
    }

    /// The keys highlight row `key`.
    pub fn activate(&mut self, key: String) {
        self.pointed = None;
        self.active.set(key, self.search.text.clone());
    }

    /// The pointer moved on row `key` to `at`, in window points (`SearchList`'s `onMouseMove`):
    /// only a moving pointer takes the row, so rows the keys, the wheel or late results carry
    /// under a still one leave the highlight alone. True when the highlight changed.
    pub fn point(&mut self, key: &str, at: [f32; 2]) -> bool {
        if self.pointer.replace(at) == Some(at) || self.pointed(key) {
            return false;
        }
        self.active.set(key.to_owned(), self.search.text.clone());
        self.pointed = Some((key.to_owned(), self.search.text.clone()));
        true
    }

    /// The pointer took row `key` under this query, and the keys haven't moved on since.
    fn pointed(&self, key: &str) -> bool {
        self.pointed.as_ref().is_some_and(|(k, q)| k == key && *q == self.search.text)
    }

    pub fn query(&self) -> &str {
        self.search.text.trim()
    }

    pub(crate) fn restore_composer(&self) -> bool {
        self.restore_composer
    }

    /// Still waiting on the server for this query: Desktop's "Loading" while nothing matches.
    pub(crate) fn loading(&self) -> bool {
        self.fetch.is_pending() || self.files_fetch.is_pending()
    }

    pub(crate) fn failed(&self) -> Option<&str> {
        self.failed.as_deref()
    }
}

/// `matchesCommandPaletteEntry`: a case-insensitive substring of the title, description, id or,
/// for commands, the group name (the server matches sessions by title, so "Session" must not
/// match every session).
pub fn matches(e: &Entry, q: &str) -> bool {
    let q = q.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    let (id, group) = match &e.action {
        Action::Session(id) => (id.as_str(), ""),
        Action::Command(_) => ("", e.group),
        Action::File(path) => (path.as_str(), e.group),
        Action::Setting(_) => ("", e.group),
    };
    [e.title.as_str(), e.description.as_str(), id, group].iter().any(|t| t.to_lowercase().contains(&q))
}

/// `rankCommandPaletteEntries`: commands lead, then sessions (open tabs first, then the newest),
/// then files.
pub fn rank(mut entries: Vec<Entry>) -> Vec<Entry> {
    let kind = |e: &Entry| match e.action {
        Action::Command(_) => 0,
        Action::Session(_) => 1,
        Action::Setting(_) => 2,
        Action::File(_) => 3,
    };
    entries.sort_by(|a, b| {
        kind(a).cmp(&kind(b)).then_with(|| match (&a.action, &b.action) {
            (Action::Session(_), Action::Session(_)) => b.open.cmp(&a.open).then(b.updated.cmp(&a.updated)),
            _ => std::cmp::Ordering::Equal,
        })
    });
    entries
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Move {
    Next,
    Previous,
    NextSection,
    PreviousSection,
    First,
    Last,
}

/// `searchListMove`: arrows step and wrap, ⌘ or ⌥ with them jump sections, Home and End go to
/// the ends. Shift and Ctrl leave the key to the field.
pub fn move_for(ev: &KeyEvent) -> Option<Move> {
    if ev.mods.shift || ev.mods.ctrl {
        return None;
    }
    let section = ev.mods.cmd || ev.mods.alt;
    match ev.key {
        Key::Down => Some(if section { Move::NextSection } else { Move::Next }),
        Key::Up => Some(if section { Move::PreviousSection } else { Move::Previous }),
        Key::Home if !section => Some(Move::First),
        Key::End if !section => Some(Move::Last),
        _ => None,
    }
}

/// `searchListTarget`: the row a move lands on, given each row's section. A section move lands
/// on a section's first row; up goes to the start of the current section first.
pub fn target(sections: &[&str], index: usize, mv: Move) -> Option<usize> {
    let count = sections.len();
    if count == 0 {
        return None;
    }
    let index = index.min(count - 1);
    let start = |at: usize| (0..=at).rev().find(|&i| i == 0 || sections[i - 1] != sections[i]).unwrap_or(0);
    Some(match mv {
        Move::First => 0,
        Move::Last => count - 1,
        Move::Next => cycle(count, Some(index), 1),
        Move::Previous => cycle(count, Some(index), -1),
        Move::PreviousSection => {
            let current = start(index);
            if index > current {
                current
            } else if current == 0 {
                start(count - 1)
            } else {
                start(current - 1)
            }
        }
        Move::NextSection => {
            let current = start(index);
            (current + 1..count).find(|&i| sections[i] != sections[current]).unwrap_or(0)
        }
    })
}

impl App {
    /// The project name a row shows: the project's, or the directory's last part.
    pub(crate) fn project_label(&self, s: &SessionInfo) -> String {
        crate::view::project_or_dir(self.project_for_session(s), &s.directory)
    }

    /// Loaded root sessions and `extra` server matches, newest metadata winning, newest first.
    pub(crate) fn merged_roots<'a>(&'a self, extra: &'a [SessionInfo]) -> Vec<&'a SessionInfo> {
        let mut by_id: HashMap<&str, &SessionInfo> = HashMap::new();
        for s in self.sessions.iter().filter(|s| s.parent.is_none()).chain(extra) {
            if by_id.get(s.id.as_str()).is_none_or(|old| s.updated >= old.updated) {
                by_id.insert(&s.id, s);
            }
        }
        let mut all: Vec<&SessionInfo> = by_id.into_values().collect();
        all.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
        all
    }

    /// The session whose folder the palette searches for files: Desktop lists files only where
    /// the palette opens over a session (`DialogCommandPalette`), not on Home.
    pub(crate) fn switcher_files_session(&self) -> Option<&str> {
        match (&self.route, self.switcher.mode) {
            (Route::Session(_), SwitcherMode::All) => self.active_session_id(),
            _ => None,
        }
    }

    pub fn switcher_entries(&self) -> Vec<Entry> {
        let q = self.switcher.query();
        let all = self.palette_commands();
        let command = |c: &crate::commands::Command| Entry {
            key: format!("command:{}", c.id),
            action: Action::Command(c.id),
            title: c.title.into(),
            project: String::new(),
            description: c.description.into(),
            hint: c.keys,
            chord: c.chord,
            group: opencode_i18n::t!("palette.group.commands"),
            updated: None,
            open: false,
        };
        if self.switcher.mode == SwitcherMode::Commands {
            return all.iter().filter(|c| c.id != crate::commands::SEARCH).map(command).filter(|e| matches(e, q)).collect();
        }
        let archived = self.switcher.mode == SwitcherMode::Archived;
        let group = match () {
            _ if archived => opencode_i18n::t!("sidebar.inbox.archived.title"),
            _ if q.is_empty() => opencode_i18n::t!("palette.group.recentSessions"),
            _ => opencode_i18n::t!("command.category.session"),
        };
        // A project's tasks live in its inbox.
        let roots = self.merged_roots(&self.switcher.loaded).into_iter().filter(|s| s.archived.is_some() == archived);
        let sessions = roots.map(|s| Entry {
            key: format!("session:{}", s.id),
            action: Action::Session(s.id.clone()),
            title: title_of(s),
            project: s.project.clone(),
            description: self.project_label(s),
            hint: None,
            chord: None,
            group,
            updated: Some(s.updated),
            open: self.tabs.iter().any(|t| t.id == s.id),
        });
        if matches!(self.switcher.mode, SwitcherMode::Sessions | SwitcherMode::Archived) {
            if q.is_empty() {
                return sessions.take(RECENT_SESSIONS).collect();
            }
            return rank(sessions.filter(|e| matches(e, q)).take(MATCHES).collect());
        }
        if q.is_empty() {
            // `preferredCommandEntries` over a session; Home's first five (`HomeCommandPalette`).
            let ids: &[&str] = if self.route == Route::Home { &crate::commands::HOME_COMMON } else { &crate::commands::COMMON };
            let common: Vec<Entry> = ids.iter().filter_map(|id| all.iter().find(|c| c.id == *id)).map(command).collect();
            return common.into_iter().chain(self.switcher_recent_files()).chain(sessions.take(RECENT)).collect();
        }
        let found: Vec<Entry> = all
            .iter()
            .map(command)
            .filter(|e| matches(e, q))
            .chain(sessions.filter(|e| matches(e, q)).take(MATCHES))
            .chain(self.switcher_settings(q))
            .chain(self.switcher_found_files(q))
            .collect();
        rank(found)
    }

    /// Settings' rows for the query (`createSettingsPaletteEntries`): the settings search's best
    /// five, servers and projects left out, a title starting with the query first.
    fn switcher_settings(&self, q: &str) -> Vec<Entry> {
        let value = q.trim().to_lowercase();
        let mut found: Vec<_> = crate::settings::search::palette(self, q, usize::MAX).into_iter().filter(|e| !e.entity).collect();
        found.sort_by_key(|e| !e.title.to_lowercase().starts_with(&value));
        found.into_iter().take(5).map(|e| Entry {
            key: format!("setting:{}", e.id),
            action: Action::Setting(e.id.clone()),
            description: if e.where_ == e.title { String::new() } else { e.where_.clone() },
            title: e.title,
            project: String::new(),
            hint: None,
            chord: None,
            group: opencode_i18n::t!("palette.group.settings"),
            updated: None,
            open: false,
        }).collect()
    }

    /// `recentFileEntries`: the files open in the side panel, the shown one first.
    fn switcher_recent_files(&self) -> Vec<Entry> {
        let Some(sid) = self.switcher_files_session() else { return Vec::new() };
        let Some(tabs) = self.side.sessions.get(sid).map(|s| &s.tab_state.tabs) else { return Vec::new() };
        let path = |key: &str| key.strip_prefix("file:").and_then(file_tab_path);
        let active = tabs.active.as_deref().and_then(path);
        let mut paths: Vec<String> = active.clone().into_iter().collect();
        paths.extend(tabs.all.iter().filter_map(|k| path(k)).filter(|p| Some(p) != active.as_ref()));
        paths.into_iter().take(RECENT).map(file_entry).collect()
    }

    /// The server's files for the query (`file.searchFiles`). Until it answers this query, the
    /// files it found before and the folder's known files that still match (`search.ts`'s
    /// settled and seen items), so rows don't blink out between keystrokes.
    fn switcher_found_files(&self, q: &str) -> Vec<Entry> {
        let Some(sid) = self.switcher_files_session() else { return Vec::new() };
        let clean = parse_path_and_line(q).0;
        let (asked, found) = &self.switcher.files;
        if *asked == clean {
            return found.iter().cloned().map(file_entry).collect();
        }
        let needle = clean.to_lowercase();
        let known = self.files.sessions.get(sid).into_iter().flat_map(|w| w.known_files.iter());
        let mut out: Vec<String> = Vec::new();
        for p in found.iter().chain(known) {
            if p.to_lowercase().contains(&needle) && !out.contains(p) {
                out.push(p.clone());
            }
        }
        out.into_iter().take(MATCHES).map(file_entry).collect()
    }

    /// The highlighted row's key: the one picked under this query, else the first.
    pub fn switcher_active(&self, entries: &[Entry]) -> Option<String> {
        self.switcher.active.resolve(&self.switcher.search.text, entries.iter().map(|e| e.key.as_str()))
    }

    pub fn open_switcher(&mut self) {
        self.open_switcher_mode(SwitcherMode::All);
    }

    pub fn open_session_search(&mut self, tx: &mut Tx) {
        tx.instant();
        self.open_switcher_mode(SwitcherMode::Sessions);
    }

    /// Search commands: the palette on commands alone.
    pub fn open_command_search(&mut self, tx: &mut Tx) {
        tx.instant();
        self.open_switcher_mode(SwitcherMode::Commands);
    }

    /// ⌘P, `file.open`: the same palette ⌘K opens over a session (`use-session-commands.tsx`).
    pub fn open_file_search(&mut self, tx: &mut Tx) {
        tx.instant();
        if let Some(sid) = self.active_session_id().map(str::to_owned) {
            self.ensure_workspace_files(&sid);
        }
        self.open_switcher_mode(SwitcherMode::All);
    }

    /// The Inbox's Archived button: session search over archived sessions.
    pub fn open_archived_search(&mut self, tx: &mut Tx) {
        tx.instant();
        self.open_switcher_mode(SwitcherMode::Archived);
    }

    fn open_switcher_mode(&mut self, mode: SwitcherMode) {
        if self.switcher.open && self.switcher.mode == mode {
            return;
        }
        let restore = if self.switcher.open {
            self.switcher.restore_composer
        } else {
            self.composer.focused || (self.project_switcher.open && self.project_switcher.restore_composer())
        };
        self.close_picker(false);
        self.close_project_switcher(false);
        self.project_state.home_menu_open = false;
        self.menus.edit.focused = false;
        self.switcher.restore_composer = restore;
        self.composer.focused = false;
        self.search.focused = false;
        self.find.input.focused = false;
        self.switcher.open = true;
        self.switcher.mode = mode;
        self.switcher.search = TextEdit::new("");
        self.switcher.search.focused = true;
        self.switcher.active.clear();
        self.switcher.pointed = None;
        self.switcher.pointer = None;
        self.switcher.loaded.clear();
        self.switcher.searched.clear();
        self.switcher.fetch.cancel();
        self.switcher.failed = None;
        self.switcher.listed_archived = false;
        self.switcher.files = (String::new(), Vec::new());
        self.switcher.files_asked = None;
        self.switcher.files_fetch.cancel();
    }

    /// Closes it; `restore` gives the composer back its focus if it had it.
    pub fn close_switcher(&mut self, restore: bool) {
        if !self.switcher.open {
            return;
        }
        self.switcher.open = false;
        self.switcher.search.focused = false;
        self.switcher.search.preedit = None;
        self.switcher.fetch.cancel();
        self.switcher.files_fetch.cancel();
        self.composer.focused = restore && self.switcher.restore_composer;
    }

    pub fn switcher_choose(&mut self, key: &str, tx: &mut Tx) {
        let Some(entry) = self.switcher_entries().into_iter().find(|e| e.key == key) else { return };
        let restore = self.switcher.mode == SwitcherMode::Archived;
        let line = parse_path_and_line(self.switcher.query()).1;
        self.close_switcher(false);
        match entry.action {
            // An archived session is restored as it opens.
            Action::Session(id) if restore => {
                if self.session(&id).is_none()
                    && let Some(s) = self.switcher.loaded.iter().find(|s| s.id == id).cloned()
                {
                    self.sessions.push(s);
                }
                self.unarchive_session(&id);
                self.open_inbox_session(&id, tx);
            }
            Action::Session(id) => {
                // A session only the search knows joins the list, so its tab has a title.
                if self.session(&id).is_none()
                    && let Some(s) = self.switcher.loaded.iter().find(|s| s.id == id).cloned()
                {
                    self.sessions.push(s);
                }
                self.open(&id, tx);
            }
            Action::Command(id) => self.run_command(id, tx),
            // `path:line` (or `path#L12`) opens the file at that line.
            Action::File(path) => self.open_workspace_file(&path, false, line, tx),
            Action::Setting(id) => {
                if let Some(e) = crate::settings::search::index(self).into_iter().find(|e| e.id == id) {
                    crate::settings::search::reveal(self, &e, tx);
                }
            }
        }
    }

    /// ⌘K and ⇧⌘P toggle "Search everything", ⇧⌘F "Search sessions" and ⌘P,
    /// over a session, "Search everything" again; while open, the list's keys.
    pub fn switcher_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if Bind::CommandSearch.matches_tx(ev, tx) {
            if self.switcher.open && self.switcher.mode == SwitcherMode::Commands {
                self.close_switcher(true);
            } else {
                self.open_command_search(tx);
            }
            return true;
        }
        if Bind::FileOpen.matches_tx(ev, tx) && matches!(self.route, Route::Session(_)) && !self.settings.open {
            if self.switcher.open && self.switcher.mode == SwitcherMode::All {
                tx.instant();
                self.close_switcher(true);
            } else {
                self.open_file_search(tx);
            }
            return true;
        }
        for (bind, mode) in [(Bind::Palette, SwitcherMode::All), (Bind::SessionSearch, SwitcherMode::Sessions)] {
            if !bind.matches_tx(ev, tx) {
                continue;
            }
            if self.switcher.open && self.switcher.mode == mode {
                self.close_switcher(true);
            } else if mode == SwitcherMode::All {
                self.open_file_search(tx);
            } else {
                tx.instant();
                self.open_switcher_mode(mode);
            }
            return true;
        }
        self.switcher.open && self.switcher_nav(ev, tx)
    }

    /// The search field's keys: moves, Enter picks, Escape closes.
    pub fn switcher_nav(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if self.switcher.search.preedit.is_some() {
            return false;
        }
        if let Some(mv) = move_for(ev) {
            let entries = self.switcher_entries();
            let active = self.switcher_active(&entries);
            let from = entries.iter().position(|e| Some(&e.key) == active.as_ref()).unwrap_or(0);
            let sections: Vec<&str> = entries.iter().map(|e| e.group).collect();
            if let Some(i) = target(&sections, from, mv) {
                let key = entries[i].key.clone();
                tx.instant();
                if i == 0 {
                    tx.scroll_to_y("switcher-scroll", 0.0);
                } else {
                    tx.scroll_into_view_in("switcher-scroll", format!("sw-{key}"));
                }
                self.switcher.activate(key);
                if let Action::Session(id) = &entries[i].action { self.prefetch_session(id); }
            }
            return true;
        }
        match ev.key {
            Key::Enter if !ev.mods.shift => {
                let entries = self.switcher_entries();
                if let Some(key) = self.switcher_active(&entries) {
                    self.switcher_choose(&key, tx);
                }
                true
            }
            Key::Escape => {
                tx.instant();
                self.close_switcher(true);
                true
            }
            _ => false,
        }
    }

    /// Takes in server results, and sends a changed query to the server.
    pub fn poll_switcher(&mut self, tx: &mut Tx) -> bool {
        let mut changed = false;
        if let Some(f) = self.switcher.fetch.take() {
            upsert_sessions(&mut self.switcher.loaded, f.sessions);
            if self.switcher.searched == f.query {
                self.switcher.failed = f.failed.then_some(f.query);
            }
            changed = true;
        }
        if let Some(found) = self.switcher.files_fetch.take() {
            if self.switcher.files_asked.as_deref() == Some(found.0.as_str()) {
                self.switcher.files = found;
            }
            changed = true;
        }
        if !self.switcher.open {
            return changed;
        }
        let entries = self.switcher_entries();
        let active = self.switcher_active(&entries);
        if let Some(active) = &active
            && let Some(Entry { action: Action::Session(id), .. }) = entries.iter().find(|e| &e.key == active)
        { self.prefetch_session(id); }
        // Results land at once: no row fades in or slides to its place.
        if changed {
            tx.instant();
            // They bring a row the keys chose back into view (`SearchList`), but leave the list
            // where the wheel put it while the first row is the highlight or the pointer took it.
            if let Some(key) = active.filter(|k| entries.first().is_some_and(|e| &e.key != k) && !self.switcher.pointed(k)) {
                tx.scroll_into_view_in("switcher-scroll", format!("sw-{key}"));
            }
        }
        changed |= self.request_switcher_files();
        let q = self.switcher.query().to_owned();
        let archived = self.switcher.mode == SwitcherMode::Archived;
        if q == self.switcher.searched && !(archived && !self.switcher.listed_archived) {
            return changed;
        }
        self.switcher.listed_archived |= archived;
        self.switcher.searched = q.clone();
        self.switcher.failed = None;
        tx.scroll_to_y("switcher-scroll", 0.0);
        self.switcher.fetch.cancel();
        let Source::Live { server, .. } = &self.source else { return true };
        // Archived sessions aren't loaded, so the archived search lists the newest with no query.
        if q.is_empty() && !archived {
            return true;
        }
        // Typed searches wait for a pause, and a newer keystroke cancels this one.
        let server = server.clone();
        self.switcher.fetch.request(q, move |q| {
            let path = format!("/api/session?parentID=null&archived={archived}&limit=50&search={}", encode(&q));
            let listed = server.get(&path);
            // `looksLikeSessionID`: a pasted id finds its session even when no title matches.
            let exact = (q.len() > 20 && q.starts_with("ses_"))
                .then(|| server.get(&format!("/api/session/{q}")).ok())
                .flatten()
                .map(|v| model::parse_session(v.get("data").unwrap_or(&v)));
            let failed = listed.is_err() && exact.is_none();
            let mut sessions: Vec<SessionInfo> = exact.into_iter().collect();
            if let Ok(v) = &listed {
                sessions.extend(model::parse_sessions(v));
            }
            Found { query: q, sessions, failed }
        });
        true
    }

    /// Asks the server for the files matching a new query, in the session's folder (a typed
    /// `:line` left out); a newer query cancels the last.
    fn request_switcher_files(&mut self) -> bool {
        let Some(sid) = self.switcher_files_session().map(str::to_owned) else { return false };
        let clean = parse_path_and_line(self.switcher.query()).0;
        if clean.is_empty() || self.switcher.files_asked.as_deref() == Some(clean.as_str()) {
            return false;
        }
        self.switcher.files_asked = Some(clean.clone());
        self.switcher.files_fetch.cancel();
        let Source::Live { server, .. } = &self.source else {
            // The fixture's folder is all known: its files are the answer.
            let known = self.files.sessions.get(&sid).map(|w| w.known_files.iter().cloned().collect::<Vec<_>>()).unwrap_or_default();
            let found = crate::files::data::rank_file_query(&known, &clean).into_iter().filter(|p| p.to_lowercase().contains(&clean.to_lowercase())).collect();
            self.switcher.files = (clean, found);
            return true;
        };
        let (server, dir) = (server.clone(), self.session(&sid).map(|s| s.directory.clone()).unwrap_or_default());
        self.switcher.files_fetch.request(clean, move |q| {
            let found = fetch_live_find(&server, &dir, &q);
            (q, found)
        });
        true
    }
}

fn file_entry(path: String) -> Entry {
    Entry {
        key: format!("file:{path}"),
        action: Action::File(path.clone()),
        title: path,
        project: String::new(),
        description: String::new(),
        hint: None,
        chord: None,
        group: opencode_i18n::t!("palette.group.files"),
        updated: None,
        open: false,
    }
}

/// Percent-encodes a query parameter's value.
pub(crate) fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests;

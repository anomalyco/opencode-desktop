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

use rune::machine::Seq;
use rune::{Key, KeyEvent, Latest, TextEdit, Tx, Waker};

use opencode_kit::model::SessionInfo;

use crate::files::data::{file_tab_path, parse_path_and_line};
use crate::keymap::Bind;
use crate::mentions::list::{Highlight, cycle};
use crate::popover::HighlightEvent;
use crate::view::title_of;
use crate::{App, Route, Source};

mod poll;
mod search;
mod servers;
mod view;
pub(crate) use view::{highlighted, relative_time};
mod band;
pub(crate) mod search_index;
mod session_index;
mod shown;
pub use view::{dialog, view};
use shown::{Event, Shown};
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
    /// A session on the server the window works against.
    Session(String),
    /// A session on another server (`⇧⌘F` searches them all).
    RemoteSession { server: opencode_kit::servers::ServerKey, id: String },
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

/// `SESSION_RESTORE_TIMEOUT_MS`: how long the dialog waits for a restore before it gives
/// control back (the request itself keeps running).
const RESTORE_TIMEOUT: f64 = 15.0;

/// The session search's pick (`SessionSearchState`): an archived session restores before it
/// opens.
#[derive(Clone, Debug, Default, PartialEq)]
enum Restore {
    #[default]
    Idle,
    Restoring { session: String, due: f64 },
}

/// A source's answer to the request that carried `attempt`; `Err` when it failed.
struct Reply<T> {
    attempt: rune::machine::Attempt,
    items: Result<Vec<T>, ()>,
}

pub struct Switcher {
    pub open: bool,
    pub mode: SwitcherMode,
    pub search: TextEdit,
    /// The lit row: the first until the keys or the pointer choose one, and the first again
    /// for each new query (`SearchList`).
    active: Highlight,
    /// The pointer's last point on a row: only a pointer that moved takes a row (`onMouseMove`'s
    /// movement check).
    pointer: Option<[f32; 2]>,
    restore_composer: bool,
    /// Sessions the server found while this was open, kept for later keystrokes
    /// (`SessionSourceState.loaded`).
    pub(crate) loaded: Vec<SessionInfo>,
    /// Which rows the dialog builds (`band`).
    pub(crate) band: band::Band,
    seq: Seq,
    /// The server's sessions for the query, and the folder's files (`file.searchFiles`), each a
    /// source machine (`search.rs`).
    sessions: search::Source<()>,
    pub(crate) files: search::Source<String>,
    fetch: Latest<Reply<SessionInfo>>,
    files_fetch: Latest<Reply<String>>,
    /// The fixture's answer to the file query and when it lands (its `find_ms`, as a server's).
    fixture_find: Option<(f64, Reply<String>)>,
    /// What the list shows while its searches run (`shown.rs`).
    shown: Shown,
    restore: Restore,
    /// The other servers' session sources while it searches sessions (`servers.rs`).
    pub(crate) remotes: Vec<servers::Remote>,
}

impl Default for Switcher {
    fn default() -> Self {
        Switcher {
            open: false,
            mode: SwitcherMode::All,
            search: TextEdit::new(""),
            active: Highlight::default(),
            pointer: None,
            restore_composer: false,
            loaded: Vec::new(),
            band: Default::default(),
            seq: Seq::default(),
            sessions: search::Source::default(),
            files: search::Source::default(),
            fetch: Latest::default(),
            files_fetch: Latest::default(),
            fixture_find: None,
            shown: Shown::default(),
            restore: Restore::Idle,
            remotes: Vec::new(),
        }
    }
}

impl Switcher {
    pub fn set_waker(&mut self, waker: Waker) {
        self.fetch = Latest::new(waker.clone());
        self.files_fetch = Latest::new(waker);
    }

    /// The keys highlight row `key`.
    pub fn activate(&mut self, key: String) {
        self.active.set(key);
    }

    /// The pointer moved on row `key` to `at`, in window points (`SearchList`'s `onMouseMove`):
    /// only a moving pointer takes the row, so rows the keys, the wheel or late results carry
    /// under a still one leave the highlight alone. True when the highlight changed.
    pub fn point(&mut self, key: &str, at: [f32; 2]) -> bool {
        if self.pointer.replace(at) == Some(at) || self.active.pointed(key) {
            return false;
        }
        self.active.step(HighlightEvent::Pointer(key.to_owned()));
        true
    }

    /// The field's text changed: the list starts again from its first row.
    pub fn query_changed(&mut self) {
        self.active.step(HighlightEvent::QueryChanged);
    }

    pub fn query(&self) -> &str {
        self.search.text.trim()
    }

    pub(crate) fn restore_composer(&self) -> bool {
        self.restore_composer
    }

    /// The typed query's session search failed (`selectPaletteSearchFailed`).
    fn failed_now(&self) -> bool {
        self.sessions.failed() && self.sessions.query() == Some(self.query())
    }
}

/// `needle`'s letters appear in `hay` in order, as the server's fuzzy file search matches them.
fn subsequence(hay: &str, needle: &str) -> bool {
    let mut letters = hay.chars();
    needle.chars().all(|c| letters.any(|h| h == c))
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
        Action::Session(id) | Action::RemoteSession { id, .. } => (id.as_str(), ""),
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
        Action::Session(_) | Action::RemoteSession { .. } => 1,
        Action::Setting(_) => 2,
        Action::File(_) => 3,
    };
    entries.sort_by(|a, b| {
        kind(a).cmp(&kind(b)).then_with(|| match (&a.action, &b.action) {
            (Action::Session(_) | Action::RemoteSession { .. }, Action::Session(_) | Action::RemoteSession { .. }) => b.open.cmp(&a.open).then(b.updated.cmp(&a.updated)),
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
        // `compareSessionTime` (`shell/layout/helpers.ts`): newest `updated ?? created` first,
        // then by id, a code unit at a time.
        all.sort_by(|a, b| b.active_ms().cmp(&a.active_ms()).then_with(|| a.id.encode_utf16().cmp(b.id.encode_utf16())));
        all
    }

    /// The session whose folder the palette searches for files: Desktop lists files only where
    /// the palette opens over a session (`DialogCommandPalette`), not on Home.
    pub(crate) fn switcher_files_session(&self) -> Option<&str> {
        match (&*self.route, self.switcher.mode) {
            (Route::Session { .. }, SwitcherMode::All) => self.active_session_id(),
            _ => None,
        }
    }

    /// The list: the typed query's matches; while it's searched with none yet, the rows on screen
    /// before it (`shown.rs`).
    pub fn switcher_entries(&self) -> Vec<Entry> {
        let q = self.switcher.query();
        let rows = self.switcher_matches(q);
        match self.switcher.shown.held(q) {
            Some(was) if rows.is_empty() && self.switcher_searching() => self.switcher_matches(was),
            _ => rows,
        }
    }

    /// What shows where the rows would be when there are none.
    pub(crate) fn switcher_empty(&self) -> shown::Empty {
        self.switcher.shown.empty(self.switcher.query(), self.switcher_searching(), self.switcher.failed_now())
    }

    /// A search for the typed query is still out: the server's sessions (live, after its pause)
    /// or the folder's files.
    pub(crate) fn switcher_searching(&self) -> bool {
        let sw = &self.switcher;
        let q = sw.query();
        // A source that hasn't heard the typed query yet is about to search it.
        let sessions = self.switcher_asks_sessions(q) && (sw.sessions.query() != Some(q) || sw.sessions.loading());
        // A large list's index is still building for a typed query.
        let index = self.switcher.mode != SwitcherMode::Commands && !q.trim().is_empty() && self.session_search_building();
        let files = self.switcher_files_session().is_some() && {
            let clean = parse_path_and_line(q).0;
            !clean.is_empty() && (sw.files.query() != Some(clean.as_str()) || sw.files.loading())
        };
        sessions || index || files || self.remotes_searching(q)
    }

    /// Whether the server's session search asks about `q`: live, not on commands alone, and
    /// typed or the archived list (archived sessions aren't loaded, so it lists the newest with
    /// no query).
    fn switcher_asks_sessions(&self, q: &str) -> bool {
        self.switcher.mode != SwitcherMode::Commands
            && matches!(*self.source, Source::Live { .. })
            && (!q.is_empty() || self.switcher.mode == SwitcherMode::Archived)
    }

    /// Everything matching `q`: commands, sessions, settings and files.
    fn switcher_matches(&self, q: &str) -> Vec<Entry> {
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
        let sessions = self.session_matches(q, group, archived);
        if matches!(self.switcher.mode, SwitcherMode::Sessions | SwitcherMode::Archived) {
            if !self.switcher.remotes.is_empty() {
                // Every server's sessions, newest first (`session-search.tsx:73-79`).
                let tag = servers::own_tag(self);
                let mut all: Vec<Entry> = sessions.into_iter().map(|e| Entry { description: servers::located(&e.description, tag.as_deref()), ..e }).chain(self.remote_matches(q, group, archived)).collect();
                all.sort_by(|a, b| b.updated.cmp(&a.updated));
                return all.into_iter().take(if q.is_empty() { RECENT_SESSIONS } else { usize::MAX }).collect();
            }
            if q.is_empty() {
                return sessions.into_iter().take(RECENT_SESSIONS).collect();
            }
            return rank(sessions);
        }
        if q.is_empty() {
            // `preferredCommandEntries` over a session; Home's first five (`HomeCommandPalette`).
            let ids: &[&str] = if self.route == Route::Home { &crate::commands::HOME_COMMON } else { &crate::commands::COMMON };
            let common: Vec<Entry> = ids.iter().filter_map(|id| all.iter().find(|c| c.id == *id)).map(command).collect();
            return common.into_iter().chain(self.switcher_recent_files()).chain(sessions.into_iter().take(RECENT)).collect();
        }
        let found: Vec<Entry> = all
            .iter()
            .map(command)
            .filter(|e| matches(e, q))
            .chain(sessions)
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
    /// files it found before whose letters still come in order (its search is fuzzy), in its
    /// order, then the folder's known files that contain the query, so rows hold their places
    /// between keystrokes.
    fn switcher_found_files(&self, q: &str) -> Vec<Entry> {
        let Some(sid) = self.switcher_files_session() else { return Vec::new() };
        let clean = parse_path_and_line(q).0;
        let found = self.switcher.files.settled();
        if found.is_some_and(|f| f.query == clean) {
            return found.into_iter().flat_map(|f| f.items.iter().cloned()).map(file_entry).collect();
        }
        let needle = clean.to_lowercase();
        let found = found.into_iter().flat_map(|f| f.items.iter()).filter(|p| subsequence(&p.to_lowercase(), &needle));
        let known = self.files.sessions.get(sid).map(|w| w.local_files(&clean, MATCHES)).unwrap_or_default();
        let mut out: Vec<String> = Vec::new();
        for p in found.chain(known.iter()) {
            if !out.contains(p) {
                out.push(p.clone());
            }
        }
        out.into_iter().take(MATCHES).map(file_entry).collect()
    }

    /// The highlighted row's key: the one picked under this query, else the first.
    pub fn switcher_active(&self, entries: &[Entry]) -> Option<String> {
        self.switcher.active.resolve(entries.iter().map(|e| e.key.as_str()))
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
        // The palette lists the commands a plugin offers as their `enabled` and `hidden` say.
        crate::plugin_host::dormant::wake_kind(self, "commands", &["enabled", "hidden"]);
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
        self.switcher.pointer = None;
        self.switcher.loaded.clear();
        self.switcher.fetch.cancel();
        self.switcher.files_fetch.cancel();
        self.switcher.fixture_find = None;
        self.switcher.sessions = search::Source::default();
        self.switcher.files = search::Source::default();
        self.switcher.restore = Restore::Idle;
        self.switcher.remotes = servers::open(self);
        self.switcher.shown.step(Event::Opened { now: *self.clock });
    }

    /// Closes it; `restore` gives the composer back its focus if it had it.
    pub fn close_switcher(&mut self, restore: bool) {
        if !self.switcher.open {
            return;
        }
        self.switcher.open = false;
        self.switcher.search.focused = false;
        self.switcher.search.cancel_composition();
        self.switcher.fetch.cancel();
        self.switcher.files_fetch.cancel();
        self.switcher.fixture_find = None;
        self.switcher.sessions = search::Source::default();
        self.switcher.files = search::Source::default();
        self.switcher.restore = Restore::Idle;
        self.switcher.remotes.clear();
        self.composer.focused = restore && self.switcher.restore_composer;
    }

    fn switcher_run(&mut self, action: Action, tx: &mut Tx) {
        if !self.switcher.open { return; }
        // One pick at a time: Enter or a click while a session restores does nothing.
        if matches!(self.switcher.restore, Restore::Restoring { .. }) { return; }
        // Another server's session: the window works against its server from here on, and it
        // opens (or restores) there as one of its own.
        if let Action::RemoteSession { server, id } = action {
            let found = self.switcher.remotes.iter().find(|r| r.server == server).and_then(|r| r.session(&id).cloned());
            crate::servers::runtime::switch(self, &server);
            if let Some(info) = found.filter(|_| self.session(&id).is_none()) {
                self.switcher.loaded.push(info);
            }
            return self.switcher_run(Action::Session(id), tx);
        }
        let restore = self.switcher.mode == SwitcherMode::Archived;
        let line = parse_path_and_line(self.switcher.query()).1;
        // An archived session is restored first, and opens once the server has
        // (`session-search-machine.ts`): the dialog waits for it.
        if let (true, Action::Session(id)) = (restore, &action) {
            let due = tx.clock() + RESTORE_TIMEOUT;
            self.switcher.restore = Restore::Restoring { session: id.clone(), due };
            tx.wake_at(due);
            if !self.write(opencode_kit::client::Write::Unarchive(id.clone())) || self.stage.is_some() {
                let ok = self.stage.is_some();
                self.switcher_restore_replied(id.clone(), ok, "", tx);
            }
            return;
        }
        self.close_switcher(false);
        match action {
            Action::Session(_) if restore => {}
            Action::Session(id) => {
                // A session only the search knows joins the list, so its tab has a title.
                if self.session(&id).is_none()
                    && let Some(s) = self.switcher.loaded.iter().find(|s| s.id == id).cloned()
                {
                    self.sessions.push(s);
                }
                self.open(&id, tx);
                // The palette is gone: show the chosen shell, not the previous transcript,
                // while the existing page request finishes. Keep its retry/back state.
                if let Some(open) = &mut *self.opening { open.step(crate::session_pages::OpenEvent::Expired); }
            }
            Action::RemoteSession { .. } => {}
            Action::Command(id) => self.run_command(id, tx),
            // `path:line` (or `path#L12`) opens the file at that line.
            // Keyboard focus follows into the file, onto its line, so a reader lands in what
            // opened rather than on the window.
            Action::File(path) => {
                self.open_workspace_file(&path, false, line, tx);
                tx.focus(format!("cl{}", line.unwrap_or(1).max(1)), tx.focus_visible());
            }
            Action::Setting(id) => {
                if let Some(e) = crate::settings::search::index(self).into_iter().find(|e| e.id == id) {
                    crate::settings::search::reveal(self, &e, tx);
                }
            }
        }
    }

    /// The server answered a restore of archived session `id` (or it timed out, `ok` false).
    pub(crate) fn switcher_restore_replied(&mut self, id: String, ok: bool, why: &str, tx: &mut Tx) {
        let ours = matches!(&self.switcher.restore, Restore::Restoring { session, .. } if *session == id);
        if !ours {
            // Restored after the dialog stopped waiting: it shows as restored, and no tab opens.
            match ok {
                true => self.unarchive_session_locally(&id),
                // An Unarchive from the Inbox says it failed as any write does (`session_write_replied`).
                false => {}
            }
            return;
        }
        self.switcher.restore = Restore::Idle;
        if !ok {
            let title = opencode_i18n::t!("common.requestFailed");
            crate::status_toast::fail(self, title, if why.is_empty() { title } else { why });
            return;
        }
        if self.session(&id).is_none()
            && let Some(s) = self.switcher.loaded.iter().find(|s| s.id == id).cloned()
        {
            self.sessions.push(s);
        }
        self.unarchive_session_locally(&id);
        self.close_switcher(false);
        self.open_inbox_session(&id, tx);
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
        if Bind::FileOpen.matches_tx(ev, tx) && matches!(*self.route, Route::Session { .. }) && !self.settings.open {
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
        if self.switcher.search.is_composing() {
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
                if let Some(key) = self.switcher_active(&entries)
                    && let Some(entry) = entries.into_iter().find(|e| e.key == key) {
                    self.switcher_run(entry.action, tx);
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

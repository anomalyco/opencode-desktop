//! Palette entry construction, ranking, filtering, and keyboard list navigation.

use std::collections::HashMap;

use opencode_kit::model::SessionInfo;
use opencode_kit::servers::ServerKey;
use opencode_search::file_rank::{file_tab_path, parse_path_and_line};
use opencode_search::query::cycle;
use opencode_ui::keymap::Bind;
use rune::{Chord, Key, KeyEvent};

use crate::{PaletteCommand, SwitcherHost, SwitcherMode, project_or_dir, quick_open, search_index, servers, session_index, shown};

/// `ENTRY_LIMIT`: the empty query's commands, files and recent sessions; `DialogSessionSearch`'s
/// recent sessions (`RECENT_LIMIT`); and a query's local session matches (`ENTRY_LIMIT * 10`).
pub const RECENT: usize = 5;
pub const RECENT_SESSIONS: usize = 10;
pub const MATCHES: usize = 50;

/// The palette's opening rows over a session, in this order (`palette.ts:57-60,160-165`): New
/// session, Previous and Next session, then every `featured` command, then Toggle review.
pub const COMMON: [&str; 3] = ["session.new", "previous-tab", "next-tab"];
pub const COMMON_VIEW: [&str; 1] = ["side"];

/// The opening rows on Home (`HomeCommandPalette`).
pub const HOME_COMMON: [&str; 5] = ["settings", "previous-tab", "next-tab", "previous-unread", "next-unread"];

/// Search commands' own entry: the commands-only palette doesn't list itself.
pub const COMMAND_SEARCH_ID: &str = "commands";

#[derive(Clone, Debug, PartialEq, Hash)]
pub enum Action {
    /// A session on the server the window works against.
    Session(String),
    /// A session on another server (`⇧⌘F` searches them all).
    RemoteSession { server: ServerKey, id: String },
    /// A project's folder on a server, where Quick Open starts a new session.
    Project { server: ServerKey, directory: String },
    /// A command's id in the host command catalog.
    Command(&'static str),
    /// A workspace file's path, from the session's folder.
    File(String),
    /// A Settings row or page, by its settings search id (`settings/palette.ts`).
    Setting(String),
}

#[derive(Clone, Debug, Hash)]
pub struct Entry {
    pub key: String,
    pub action: Action,
    pub title: String,
    /// A session's project id, for its icon.
    pub project: String,
    /// A session's project, a command's description.
    pub description: String,
    /// A command's keymap entry, whose chord the row shows.
    pub hint: Option<Bind>,
    /// A plugin command's chord, which has no keymap entry.
    pub chord: Option<Chord>,
    pub group: &'static str,
    pub updated: Option<i64>,
    /// The session is open as a tab.
    pub open: bool,
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
        Action::Project { .. } => ("", ""),
    };
    [e.title.as_str(), e.description.as_str(), id, group].iter().any(|t| t.to_lowercase().contains(&q))
}

/// `rankCommandPaletteEntries`: commands lead, then projects, sessions (open tabs first, then the
/// newest), settings and files.
pub fn rank(mut entries: Vec<Entry>) -> Vec<Entry> {
    let kind = |e: &Entry| match e.action {
        Action::Command(_) => 0,
        Action::Project { .. } => 1,
        Action::Session(_) | Action::RemoteSession { .. } => 2,
        Action::Setting(_) => 3,
        Action::File(_) => 4,
    };
    entries.sort_by(|a, b| {
        kind(a).cmp(&kind(b)).then_with(|| match (&a.action, &b.action) {
            (Action::Session(_) | Action::RemoteSession { .. }, Action::Session(_) | Action::RemoteSession { .. }) => b.open.cmp(&a.open).then(b.updated.cmp(&a.updated)),
            _ => std::cmp::Ordering::Equal,
        })
    });
    entries
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

/// The project name a row shows: the project's, or the directory's last part.
pub fn project_label(host: &dyn SwitcherHost, s: &SessionInfo) -> String {
    project_or_dir(host.project_for_session(s), &s.directory)
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

/// Convenience wrapper over [`merged_roots`] reading the host's loaded sessions and ordering by
/// the host's recency timestamp (`SwitcherHost::session_active_ms`).
pub fn merged_roots_for<'a>(host: &'a dyn SwitcherHost, extra: &'a [SessionInfo]) -> Vec<&'a SessionInfo> {
    let mut all = merged_roots(host.sessions(), extra);
    all.sort_by(|a, b| {
        host.session_active_ms(b)
            .cmp(&host.session_active_ms(a))
            .then_with(|| a.id.encode_utf16().cmp(b.id.encode_utf16()))
    });
    all
}

/// The session whose folder the palette searches for files: Desktop lists files only where
/// the palette opens over a session (`DialogCommandPalette`), not on Home.
pub(crate) fn files_session(host: &dyn SwitcherHost) -> Option<&str> {
    match host.switcher().mode {
        SwitcherMode::All => host.active_session_id(),
        _ => None,
    }
}

/// The list: the typed query's matches; while it's searched with none yet, the rows on screen
/// before it (`shown.rs`).
pub fn entries(host: &dyn SwitcherHost) -> Vec<Entry> {
    let sw = host.switcher();
    let q = sw.query();
    let rows = matches_for(host, q);
    if let Some(was) = sw.shown.held(q).filter(|_| rows.is_empty() && searching(host)) {
        return matches_for(host, was);
    }
    sw.order.borrow_mut().present(q, rows, &sw.active)
}

/// The highlighted row's key: the one picked under this query, else the first.
pub fn active(host: &dyn SwitcherHost, entries: &[Entry]) -> Option<String> {
    host.switcher().active.resolve(entries.iter().map(|e| e.key.as_str()))
}

/// What shows where the rows would be when there are none.
pub(crate) fn empty(host: &dyn SwitcherHost) -> shown::Empty {
    let sw = host.switcher();
    sw.shown.empty(sw.query(), searching(host), sw.failed_now())
}

/// A search for the typed query is still out: the server's sessions (live, after its pause)
/// or the folder's files.
pub(crate) fn searching(host: &dyn SwitcherHost) -> bool {
    let sw = host.switcher();
    let q = sw.query();
    // A source that hasn't heard the typed query yet is about to search it.
    let sessions = asks_sessions(host, q) && (sw.sessions.query() != Some(q) || sw.sessions.loading());
    // A large list's index is still building for a typed query.
    let index = sw.mode != SwitcherMode::Commands && !q.trim().is_empty() && search_index::is_building(host);
    let files = files_session(host).is_some() && {
        let clean = parse_path_and_line(q).0;
        !clean.is_empty() && (sw.files.query() != Some(clean.as_str()) || sw.files.loading())
    };
    sessions || index || files || servers::remotes_searching(host, q)
}

/// Whether the server's session search asks about `q`: live, not on commands alone, and
/// typed or the archived list (archived sessions aren't loaded, so it lists the newest with
/// no query).
pub(crate) fn asks_sessions(host: &dyn SwitcherHost, q: &str) -> bool {
    let mode = host.switcher().mode;
    mode != SwitcherMode::Commands && host.live_server().is_some() && (!q.is_empty() || mode == SwitcherMode::Archived)
}

/// Everything matching `q`: commands, sessions, settings and files.
pub(crate) fn matches_for(host: &dyn SwitcherHost, q: &str) -> Vec<Entry> {
    let sw = host.switcher();
    let all = host.palette_commands();
    let command = |c: &PaletteCommand| Entry {
        key: format!("command:{}", c.id),
        action: Action::Command(c.id),
        title: c.title.clone(),
        project: String::new(),
        description: c.description.clone(),
        hint: c.hint,
        chord: c.chord,
        group: opencode_i18n::t!("palette.group.commands"),
        updated: None,
        open: false,
    };
    if sw.mode == SwitcherMode::Commands {
        return all.iter().filter(|c| c.id != COMMAND_SEARCH_ID).map(command).filter(|e| matches(e, q)).collect();
    }
    let archived = sw.mode == SwitcherMode::Archived;
    let group = match () {
        _ if archived => opencode_i18n::t!("sidebar.inbox.section.archived"),
        _ if q.is_empty() => opencode_i18n::t!("palette.group.recentSessions"),
        _ => opencode_i18n::t!("command.category.session"),
    };
    // A project's tasks live in its inbox.
    let sessions = session_index::session_matches(host, q, group, archived);
    match sw.mode {
        SwitcherMode::Sessions | SwitcherMode::Archived => return searched_sessions(host, q, group, archived, sessions),
        // Projects ahead of the sessions (`DialogSessionSearch`'s `items`).
        SwitcherMode::QuickOpen => return quick_open::projects(host, q).into_iter().chain(searched_sessions(host, q, group, archived, sessions)).collect(),
        SwitcherMode::All | SwitcherMode::Commands => {}
    }
    if q.is_empty() {
        // `preferredCommandEntries` over a session; Home's first five (`HomeCommandPalette`).
        let common: Vec<Entry> = if host.is_home() {
            HOME_COMMON.iter().filter_map(|id| all.iter().find(|c| c.id == *id)).map(command).collect()
        } else {
            let pick = |ids: &[&str]| ids.iter().filter_map(|id| all.iter().find(|c| c.id == *id)).collect::<Vec<_>>();
            pick(&COMMON).into_iter().chain(all.iter().filter(|c| c.featured)).chain(pick(&COMMON_VIEW)).map(command).collect()
        };
        return common.into_iter().chain(recent_files(host)).chain(sessions.into_iter().take(RECENT)).collect();
    }
    let found: Vec<Entry> = all
        .iter()
        .map(command)
        .filter(|e| matches(e, q))
        .chain(sessions)
        .chain(settings_entries(host, q))
        .chain(found_files(host, q))
        .collect();
    rank(found)
}

/// The session search's rows: every server's, newest first, or the first few with no query.
fn searched_sessions(host: &dyn SwitcherHost, q: &str, group: &'static str, archived: bool, sessions: Vec<Entry>) -> Vec<Entry> {
    if !host.switcher().remotes.is_empty() {
        // Every server's sessions, newest first (`session-search.tsx:73-79`).
        let tag = servers::own_tag(host);
        let mut all_rows: Vec<Entry> = sessions
            .into_iter()
            .map(|e| Entry { description: servers::located(&e.description, tag.as_deref()), ..e })
            .chain(servers::remote_matches(host, q, group, archived))
            .collect();
        all_rows.sort_by_key(|a| std::cmp::Reverse(a.updated));
        return all_rows.into_iter().take(if q.is_empty() { RECENT_SESSIONS } else { usize::MAX }).collect();
    }
    if q.is_empty() {
        return sessions.into_iter().take(RECENT_SESSIONS).collect();
    }
    rank(sessions)
}

/// Settings' rows for the query (`createSettingsPaletteEntries`): the settings search's best
/// five, servers and projects left out, a title starting with the query first.
fn settings_entries(host: &dyn SwitcherHost, q: &str) -> Vec<Entry> {
    let value = q.trim().to_lowercase();
    let mut found = host.palette_settings(q);
    found.sort_by_key(|e| !e.title.to_lowercase().starts_with(&value));
    found
        .into_iter()
        .take(5)
        .map(|e| Entry {
            key: format!("setting:{}", e.id),
            action: Action::Setting(e.id),
            description: if e.where_ == e.title { String::new() } else { e.where_ },
            title: e.title,
            project: String::new(),
            hint: None,
            chord: None,
            group: opencode_i18n::t!("palette.group.settings"),
            updated: None,
            open: false,
        })
        .collect()
}

/// `recentFileEntries`: the files open in the side panel, the shown one first.
fn recent_files(host: &dyn SwitcherHost) -> Vec<Entry> {
    let Some(sid) = files_session(host) else { return Vec::new() };
    let Some((active_key, all_keys)) = host.side_tab_keys(sid) else { return Vec::new() };
    let path = |key: &str| key.strip_prefix("file:").and_then(file_tab_path);
    let active = active_key.and_then(path);
    let mut paths: Vec<String> = active.clone().into_iter().collect();
    paths.extend(all_keys.iter().filter_map(|k| path(k)).filter(|p| Some(p) != active.as_ref()));
    paths.into_iter().take(RECENT).map(file_entry).collect()
}

/// The server's files for the query (`file.searchFiles`). Until it answers this query, the
/// files it found before whose letters still come in order (its search is fuzzy), in its
/// order, then the folder's known files that contain the query, so rows hold their places
/// between keystrokes.
fn found_files(host: &dyn SwitcherHost, q: &str) -> Vec<Entry> {
    let Some(sid) = files_session(host) else { return Vec::new() };
    let clean = parse_path_and_line(q).0;
    let found = host.switcher().files.settled();
    if found.is_some_and(|f| f.query == clean) {
        return found.into_iter().flat_map(|f| f.items.iter().cloned()).map(file_entry).collect();
    }
    let needle = clean.to_lowercase();
    let found = found.into_iter().flat_map(|f| f.items.iter()).filter(|p| subsequence(&p.to_lowercase(), &needle));
    let known = host.local_workspace_files(sid, &clean, MATCHES);
    let mut out: Vec<String> = Vec::new();
    for p in found.chain(known.iter()) {
        if !out.contains(p) {
            out.push(p.clone());
        }
    }
    out.into_iter().take(MATCHES).map(file_entry).collect()
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

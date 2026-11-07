//! ⌘K, Desktop's "Search everything" palette as the home surface opens it
//! (`home/start/command-palette.tsx`, `shell/commands/{palette,search,dialog}.ts(x)`,
//! `ui/forms/search-list`): commands and sessions, opening a session as a tab.
//!
//! ⇧⌘P opens it on commands alone (`commands.search`); the commands are `commands.rs`'s list.
//!
//! Empty, it lists a few commands, then the five newest sessions. Typed, it filters on title,
//! project and session id, asks the server for title matches past the loaded list
//! (`/api/session?search=`, after a 100 ms pause), and ranks sessions first (open tabs, then
//! the newest) ahead of commands.

use crate::pickers::PickerHost as _;
use std::collections::HashMap;
use std::time::Duration;

use rune::{Color, Cx, El, Key, KeyEvent, Latest, ScrollSpec, Span, TextEdit, Transition, Tx, Waker, col, el, icon, row, spacer, text, text_input};
use rune_theme_opencode::icons as ic;

use opencode_kit::model::{self, SessionInfo};

use crate::keymap::Bind;
use crate::mentions::list::{Highlight, cycle, upsert_sessions};
use crate::palette::Pal;
use crate::view::{UI, UI_MED, title_of};
use crate::{App, Source};

/// Rows each list shows: the empty query's commands and recent sessions (`ENTRY_LIMIT`),
/// `DialogSessionSearch`'s recent sessions (`RECENT_LIMIT`), and a query's local session matches.
const RECENT: usize = 5;
const RECENT_SESSIONS: usize = 10;
const MATCHES: usize = 50;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SwitcherMode {
    #[default]
    All,
    Sessions,
    /// ⇧⌘P: commands only.
    Commands,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Session(String),
    /// A command's id in `commands.rs`.
    Command(&'static str),
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
    restore_composer: bool,
    /// Sessions the server found while this was open, kept for later keystrokes.
    pub(crate) loaded: Vec<SessionInfo>,
    /// The query last sent to the server and the one that failed.
    searched: String,
    failed: Option<String>,
    fetch: Latest<Found>,
}

impl Default for Switcher {
    fn default() -> Self {
        Switcher {
            open: false,
            mode: SwitcherMode::All,
            search: TextEdit::new(""),
            active: Highlight::default(),
            restore_composer: false,
            loaded: Vec::new(),
            searched: String::new(),
            failed: None,
            fetch: Latest::default().debounce(Duration::from_millis(100)),
        }
    }
}

impl Switcher {
    pub fn set_waker(&mut self, waker: Waker) {
        self.fetch = Latest::new(waker).debounce(Duration::from_millis(100));
    }

    pub fn activate(&mut self, key: String) {
        self.active.set(key, self.search.text.clone());
    }

    pub fn query(&self) -> &str {
        self.search.text.trim()
    }

    pub(crate) fn restore_composer(&self) -> bool {
        self.restore_composer
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
    };
    [e.title.as_str(), e.description.as_str(), id, group].iter().any(|t| t.to_lowercase().contains(&q))
}

/// `rankCommandPaletteEntries`: sessions lead, open tabs first and then the newest, then commands.
pub fn rank(mut entries: Vec<Entry>) -> Vec<Entry> {
    let kind = |e: &Entry| u8::from(matches!(e.action, Action::Command(_)));
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

    pub fn switcher_entries(&self) -> Vec<Entry> {
        let q = self.switcher.query();
        let all = self.commands();
        let command = |c: &crate::commands::Command| Entry {
            key: format!("command:{}", c.id),
            action: Action::Command(c.id),
            title: c.title.into(),
            project: String::new(),
            description: c.description.into(),
            hint: c.keys,
            chord: c.chord,
            group: "Commands",
            updated: None,
            open: false,
        };
        if self.switcher.mode == SwitcherMode::Commands {
            return all.iter().filter(|c| c.id != crate::commands::SEARCH).map(command).filter(|e| matches(e, q)).collect();
        }
        let group = if q.is_empty() { "Recent sessions" } else { "Session" };
        // A project's tasks live in its inbox.
        let sessions = self.merged_roots(&self.switcher.loaded).into_iter().map(|s| Entry {
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
        if self.switcher.mode == SwitcherMode::Sessions {
            if q.is_empty() {
                return sessions.take(RECENT_SESSIONS).collect();
            }
            return rank(sessions.filter(|e| matches(e, q)).take(MATCHES).collect());
        }
        if q.is_empty() {
            let common = crate::commands::COMMON.iter().filter_map(|id| all.iter().find(|c| c.id == *id)).map(command);
            return common.take(RECENT).chain(sessions.take(RECENT)).collect();
        }
        let found: Vec<Entry> =
            all.iter().map(command).filter(|e| matches(e, q)).chain(sessions.filter(|e| matches(e, q)).take(MATCHES)).collect();
        rank(found)
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

    /// ⇧⌘P: the palette on commands alone.
    pub fn open_command_search(&mut self, tx: &mut Tx) {
        tx.instant();
        self.open_switcher_mode(SwitcherMode::Commands);
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
        self.switcher.loaded.clear();
        self.switcher.searched.clear();
        self.switcher.fetch.cancel();
        self.switcher.failed = None;
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
        self.composer.focused = restore && self.switcher.restore_composer;
    }

    pub fn switcher_choose(&mut self, key: &str, tx: &mut Tx) {
        let Some(entry) = self.switcher_entries().into_iter().find(|e| e.key == key) else { return };
        self.close_switcher(false);
        match entry.action {
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
        }
    }

    /// ⌘K toggles "Search everything", ⇧⌘F "Search sessions" and ⇧⌘P "Search commands"; while
    /// open, the list's keys.
    pub fn switcher_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if Bind::CommandSearch.matches_tx(ev, tx) {
            if self.switcher.open && self.switcher.mode == SwitcherMode::Commands {
                self.close_switcher(true);
            } else {
                self.open_command_search(tx);
            }
            return true;
        }
        for (bind, mode) in [(Bind::Palette, SwitcherMode::All), (Bind::SessionSearch, SwitcherMode::Sessions)] {
            if !bind.matches_tx(ev, tx) {
                continue;
            }
            if self.switcher.open && self.switcher.mode == mode {
                self.close_switcher(true);
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
        if !self.switcher.open {
            return changed;
        }
        let entries = self.switcher_entries();
        if let Some(active) = self.switcher_active(&entries)
            && let Some(Entry { action: Action::Session(id), .. }) = entries.iter().find(|e| e.key == active)
        { self.prefetch_session(id); }
        // Results land at once: no row fades in or slides to its place.
        if changed {
            tx.instant();
        }
        let q = self.switcher.query().to_owned();
        if q == self.switcher.searched {
            return changed;
        }
        self.switcher.searched = q.clone();
        self.switcher.failed = None;
        tx.scroll_to_y("switcher-scroll", 0.0);
        self.switcher.fetch.cancel();
        let Source::Live { server, .. } = &self.source else { return true };
        if q.is_empty() {
            return true;
        }
        // Typed searches wait for a pause, and a newer keystroke cancels this one.
        let server = server.clone();
        self.switcher.fetch.request(q, move |q| {
            let path = format!("/api/session?parentID=null&archived=false&limit=50&search={}", encode(&q));
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

// ---------------------------------------------------------------------------------------------
// View: a 640 × 480 dialog centred over a scrim (`dialog.css`, `search-list.css`).

/// The first match of the query in `s`, in the accent colour (`SearchHighlight`).
pub(crate) fn highlighted(s: &str, q: &str, style: rune::TextStyle, color: Color, accent: Color) -> El {
    let t = text(s.to_owned()).font(style).color(color).truncate_fade();
    let lower = s.to_lowercase();
    let needle = q.trim().to_lowercase();
    match lower.find(&needle) {
        Some(at) if !needle.is_empty() && lower.len() == s.len() => t.spans(vec![Span::new(at, at + needle.len()).color(accent)]),
        _ => t,
    }
}

fn entry_row(s: &App, p: &Pal, e: &Entry, active: bool, q: &str, now: i64) -> El {
    let key = e.key.clone();
    let hover = e.key.clone();
    let mut main = row().key("main").grow(1.0).shrink().min_w(0.0).gap(8.0).center_y();
    if matches!(e.action, Action::Session(_)) {
        let mut badge = el().key("badge").size(16.0, 16.0).child(crate::icons::badge(s, p, s.project(&e.project), &e.description));
        if e.open {
            badge = badge.child(el().key("open").absolute(-6.0, 2.0).size(2.0, 12.0).radius(2.0).fill(p.q.layer4));
        }
        main = main.child(badge);
    }
    let mut words = row().key("words").shrink().min_w(0.0).gap(6.0).center_y().child(highlighted(&e.title, q, UI_MED, p.text, p.accent));
    if !e.description.is_empty() {
        words = words.child(highlighted(&e.description, q, UI, p.muted, p.accent));
    }
    let mut r = row()
        .key(format!("sw-{}", e.key))
        .role("option")
        .label(e.title.clone())
        .w_full()
        .h(36.0)
        .px(12.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .fill(if active { p.row_active } else { Color::TRANSPARENT })
        // Not `on_hover`: its commit would animate the highlight's fill from row to row.
        .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
            if inside {
                tx.instant();
                s.switcher.activate(hover.clone());
                if let Some(id) = hover.strip_prefix("session:") { s.prefetch_session(id); }
            }
        })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.switcher_choose(&key, tx))
        .child(main.child(words));
    if let Some(at) = e.updated {
        r = r.child(text(model::ago(now, at)).key("meta").font(UI).color(p.muted));
    } else if let Some(bind) = e.hint.filter(|b| b.chord().is_some()) {
        r = r.child(spacer()).child(crate::keymap::keycaps(p, bind).key("hint"));
    } else if let Some(chord) = e.chord {
        let caps = crate::keymap::caps(chord).into_iter().fold(row().key("keys").gap(2.0).center_y(), |r, c| r.child(crate::queue::keycap(p, &c.to_uppercase())));
        r = r.child(spacer()).child(caps.key("hint"));
    }
    r
}

pub fn view(s: &App, p: &Pal, cx: &Cx<App>) -> [El; 2] {
    let [win_w, win_h] = cx.layout_size("window").unwrap_or([1280.0, 820.0]);
    let (w, h) = ((win_w - 24.0).min(640.0), (win_h - 96.0).min(480.0));
    let scrim = el().key("switcher-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.scrim).transition(Transition::none());
    [scrim, dialog(s, p, w, h).absolute((win_w - w) / 2.0, (win_h - h) / 2.0)]
}

/// The dialog itself, `w` × `h`.
pub fn dialog(s: &App, p: &Pal, w: f32, h: f32) -> El {
    let q = s.switcher.query();
    let entries = s.switcher_entries();
    let active = s.switcher_active(&entries);

    let sessions_only = s.switcher.mode == SwitcherMode::Sessions;
    let placeholder = match s.switcher.mode {
        _ if sessions_only => "Search sessions",
        SwitcherMode::Commands => "Search commands",
        _ => "Search commands and sessions",
    };
    let search = row()
        .key("switcher-field")
        .h(36.0)
        .pl(12.0)
        .pr(12.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .fill(p.search_bg)
        .hover(p.layer2)
        .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
        .child(
            text_input(&s.switcher.search, UI.lh(20.0))
                .key("switcher-input")
                .grow(1.0)
                .shrink()
                .h(20.0)
                .color(p.text)
                .placeholder(placeholder, p.faint)
                .caret(p.text, p.selection)
                .on_edit(|s: &mut App| &mut s.switcher.search)
                .on_input_key(|s: &mut App, ev, tx| s.switcher_nav(ev, tx) || s.shortcut(ev, tx)),
        );

    let mut results = col().key("switcher-results").w_full().px(6.0).pt(6.0).pb(8.0);
    let mut i = 0;
    while i < entries.len() {
        let group = entries[i].group;
        let end = entries[i..].iter().position(|e| e.group != group).map_or(entries.len(), |n| i + n);
        // Space inside the section, so the next header meets this one and pushes it up.
        let mut section = col().key(format!("sw-section-{group}")).w_full().gap(1.0).pb(if end < entries.len() { 16.0 } else { 0.0 }).child(
            row()
                .key("header")
                .w_full()
                .h(28.0)
                .px(12.0)
                .center_y()
                .fill(p.bg_base)
                .sticky_top(0.0)
                .child(text(group).font(UI).color(p.muted)),
        );
        for e in &entries[i..end] {
            section = section.child(entry_row(s, p, e, active.as_deref() == Some(e.key.as_str()), q, s.now_ms));
        }
        results = results.child(section);
        i = end;
    }
    if entries.is_empty() {
        let state = if s.switcher.fetch.is_pending() {
            "Loading"
        } else if s.switcher.failed.as_deref() == Some(q) {
            "Request failed"
        } else {
            "No results found"
        };
        results = results.child(col().key("switcher-empty").w_full().min_h(120.0).center().child(text(state).font(UI).color(p.muted)));
    }

    col()
        .key("switcher")
        .role("dialog")
        .label("Search everything")
        .w(w)
        .h(h)
        .radius(12.0)
        .fill(p.bg_base)
        .border(0.5, p.ring)
        .shadow(8.0, 16.0, p.shadow_far)
        .shadow(4.0, 8.0, p.shadow)
        .clip_soft(0.0)
        .transition(Transition::none())
        .modal(true, |s: &mut App| s.close_switcher(true))
        .child(col().key("switcher-search").w_full().padding(6.0).child(search))
        .child(
            col()
                .key("switcher-scroll")
                .role("listbox")
                .w_full()
                .grow(1.0)
                .shrink()
                .scroll(ScrollSpec { stick_bottom: false })
                .clip_fade(0.0, 0.0, 0.0, 24.0)
                .child(results),
        )
}

#[cfg(test)]
mod tests;

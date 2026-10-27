//! ⌘K, Desktop's "Search everything" palette as the home surface opens it
//! (`home/start/command-palette.tsx`, `shell/commands/{palette,search,dialog}.ts(x)`,
//! `ui/forms/search-list`): commands and sessions, opening a session as a tab.
//!
//! Empty, it lists a few commands, then the five newest sessions. Typed, it filters on title,
//! project and session id, asks the server for title matches past the loaded list
//! (`/api/session?search=`, after a 100 ms pause), and ranks sessions first (open tabs, then
//! the newest) ahead of commands.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rune::{Color, Cx, El, Key, KeyEvent, ScrollSpec, Span, Spring, SpringPair, TextEdit, Theme, Transition, Tx, col, el, icon, row, spacer, text, text_input};
use rune_theme_opencode::icons as ic;

use opencode_kit::model::{self, SessionInfo};

use crate::palette::Pal;
use crate::view::{UI, UI_MED, avatar, title_of};
use crate::{App, Route, Source};

/// Rows each list shows: the empty query's commands and recent sessions (`ENTRY_LIMIT`), and a
/// query's local session matches.
const RECENT: usize = 5;
const MATCHES: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    NewSession,
    Home,
    ToggleSidebar,
}

const COMMANDS: [(Command, &str, &str, &str); 3] = [
    (Command::NewSession, "new", "New session", "⌘T"),
    (Command::Home, "home", "Home", "⌘B"),
    (Command::ToggleSidebar, "sidebar", "Toggle sidebar", "⇧⌘B"),
];

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Session(String),
    Command(Command),
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub key: String,
    pub action: Action,
    pub title: String,
    /// A session's project, a command's nothing.
    pub description: String,
    pub hint: &'static str,
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
    pub search: TextEdit,
    /// The highlighted row and the query it was picked under; a new query starts from its first row.
    active: Option<(String, String)>,
    restore_composer: bool,
    /// Sessions the server found while this was open, kept for later keystrokes.
    loaded: Vec<SessionInfo>,
    /// The query last sent to the server, the one still in flight and the one that failed.
    searched: String,
    loading: Option<String>,
    failed: Option<String>,
    generation: Arc<AtomicU64>,
    found_tx: Sender<Found>,
    found: Receiver<Found>,
}

impl Default for Switcher {
    fn default() -> Self {
        let (found_tx, found) = channel();
        Switcher {
            open: false,
            search: TextEdit::new(""),
            active: None,
            restore_composer: false,
            loaded: Vec::new(),
            searched: String::new(),
            loading: None,
            failed: None,
            generation: Arc::new(AtomicU64::new(0)),
            found_tx,
            found,
        }
    }
}

impl Switcher {
    pub fn activate(&mut self, key: String) {
        self.active = Some((key, self.search.text.clone()));
    }

    pub fn query(&self) -> &str {
        self.search.text.trim()
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
    let id = match &e.action {
        Action::Session(id) => id.as_str(),
        Action::Command(_) => "",
    };
    let group = if matches!(e.action, Action::Command(_)) { e.group } else { "" };
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
        Move::Next => (index + 1) % count,
        Move::Previous => (index + count - 1) % count,
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
    fn project_label(&self, s: &SessionInfo) -> String {
        self.project(&s.project)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| s.directory.rsplit('/').find(|x| !x.is_empty()).unwrap_or("").to_owned())
    }

    /// Loaded root sessions and what the server found, newest metadata winning, newest first.
    fn known_sessions(&self) -> Vec<&SessionInfo> {
        let mut by_id: HashMap<&str, &SessionInfo> = HashMap::new();
        for s in self.sessions.iter().filter(|s| s.parent.is_none()).chain(&self.switcher.loaded) {
            let keep = by_id.get(s.id.as_str()).is_none_or(|old| s.updated >= old.updated);
            if keep {
                by_id.insert(&s.id, s);
            }
        }
        let mut all: Vec<&SessionInfo> = by_id.into_values().collect();
        all.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
        all
    }

    pub fn switcher_entries(&self) -> Vec<Entry> {
        let q = self.switcher.query();
        let commands = COMMANDS.iter().map(|&(c, key, title, hint)| Entry {
            key: format!("command:{key}"),
            action: Action::Command(c),
            title: title.into(),
            description: String::new(),
            hint,
            group: "Commands",
            updated: None,
            open: false,
        });
        let group = if q.is_empty() { "Recent sessions" } else { "Session" };
        let sessions = self.known_sessions().into_iter().map(|s| Entry {
            key: format!("session:{}", s.id),
            action: Action::Session(s.id.clone()),
            title: title_of(s),
            description: self.project_label(s),
            hint: "",
            group,
            updated: Some(s.updated),
            open: self.tabs.iter().any(|t| t.id == s.id),
        });
        if q.is_empty() {
            return commands.take(RECENT).chain(sessions.take(RECENT)).collect();
        }
        let found: Vec<Entry> = commands.filter(|e| matches(e, q)).chain(sessions.filter(|e| matches(e, q)).take(MATCHES)).collect();
        rank(found)
    }

    /// The highlighted row's key: the one picked under this query, else the first.
    pub fn switcher_active(&self, entries: &[Entry]) -> Option<String> {
        let picked = self.switcher.active.as_ref().filter(|(_, q)| *q == self.switcher.search.text).map(|(k, _)| k);
        picked.filter(|k| entries.iter().any(|e| &e.key == *k)).cloned().or_else(|| entries.first().map(|e| e.key.clone()))
    }

    pub fn open_switcher(&mut self) {
        if self.switcher.open {
            return;
        }
        self.close_picker(false);
        self.switcher.restore_composer = self.composer.focused;
        self.composer.focused = false;
        self.search.focused = false;
        self.switcher.open = true;
        self.switcher.search = TextEdit::new("");
        self.switcher.search.focused = true;
        self.switcher.active = None;
        self.switcher.loaded.clear();
        self.switcher.searched.clear();
        self.switcher.loading = None;
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
        self.switcher.generation.fetch_add(1, Ordering::Relaxed);
        self.switcher.loading = None;
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
            Action::Command(Command::NewSession) => self.new_session(tx),
            Action::Command(Command::Home) => {
                self.route = Route::Home;
                tx.settle();
            }
            Action::Command(Command::ToggleSidebar) => self.sidebar_open = !self.sidebar_open,
        }
    }

    /// ⌘K toggles it from anywhere; while it's open, the list's keys.
    pub fn switcher_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if ev.is_command('k') && !ev.mods.shift && !ev.mods.alt && !ev.mods.ctrl {
            if self.switcher.open {
                self.close_switcher(true);
            } else {
                self.open_switcher();
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
        for f in self.switcher.found.try_iter() {
            for s in f.sessions {
                match self.switcher.loaded.iter_mut().find(|x| x.id == s.id) {
                    Some(old) => *old = s,
                    None => self.switcher.loaded.push(s),
                }
            }
            if self.switcher.loading.as_deref() == Some(f.query.as_str()) {
                self.switcher.loading = None;
                self.switcher.failed = f.failed.then_some(f.query);
            }
            changed = true;
        }
        if !self.switcher.open {
            return changed;
        }
        let q = self.switcher.query().to_owned();
        if q == self.switcher.searched {
            return changed;
        }
        self.switcher.searched = q.clone();
        self.switcher.failed = None;
        tx.scroll_to_y("switcher-scroll", 0.0);
        let generation = self.switcher.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.switcher.loading = None;
        let Source::Live { server, .. } = &self.source else { return true };
        if q.is_empty() {
            return true;
        }
        self.switcher.loading = Some(q.clone());
        let (server, wake, out, current) = (server.clone(), self.waker.clone(), self.switcher.found_tx.clone(), self.switcher.generation.clone());
        std::thread::spawn(move || {
            // Typed searches wait for a pause, and a newer keystroke cancels this one.
            std::thread::sleep(Duration::from_millis(100));
            if current.load(Ordering::Relaxed) != generation {
                return;
            }
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
            let _ = out.send(Found { query: q, sessions, failed });
            if let Some(w) = wake {
                w();
            }
        });
        true
    }
}

/// Percent-encodes a query parameter's value.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// View: a 640 × 480 dialog centred over a scrim (`dialog.css`, `search-list.css`).

/// Tokens the shared palette doesn't carry.
struct Ink {
    scrim: Color,
    active: Color,
    marker: Color,
    shadow_far: Color,
    shadow_near: Color,
}

fn ink(dark: bool) -> &'static Ink {
    static DARK: OnceLock<Ink> = OnceLock::new();
    static LIGHT: OnceLock<Ink> = OnceLock::new();
    let build = || {
        let t = Theme::from_json(include_str!("../assets/theme.json"), dark).unwrap_or_else(|e| panic!("{e}"));
        let c = |n: &str| t.color(n);
        Ink {
            scrim: c("overlay-simple-overlay-scrim"),
            // A row's highlight: the hover overlay, lifted to alpha-light-10 in dark.
            active: if dark { c("alpha-light-10") } else { c("overlay-simple-overlay-hover") },
            marker: c("background-bg-layer-04"),
            // --v2-elevation-floating's two drops.
            shadow_far: if dark { c("alpha-dark-30") } else { c("alpha-dark-4") },
            shadow_near: if dark { c("alpha-dark-30") } else { c("alpha-dark-8") },
        }
    };
    if dark { DARK.get_or_init(build) } else { LIGHT.get_or_init(build) }
}

/// Snappy, no bounce; the exit a little shorter.

/// The first match of the query in `s`, in the accent colour (`SearchHighlight`).
fn highlighted(s: &str, q: &str, style: rune::TextStyle, color: Color, accent: Color) -> El {
    let t = text(s.to_owned()).font(style).color(color).truncate();
    let lower = s.to_lowercase();
    let needle = q.trim().to_lowercase();
    match lower.find(&needle) {
        Some(at) if !needle.is_empty() && lower.len() == s.len() => t.spans(vec![Span::new(at, at + needle.len()).color(accent)]),
        _ => t,
    }
}

fn entry_row(p: &Pal, k: &Ink, e: &Entry, active: bool, q: &str, now: i64) -> El {
    let key = e.key.clone();
    let hover = e.key.clone();
    let mut main = row().key("main").grow(1.0).shrink().min_w(0.0).gap(8.0).center_y();
    if matches!(e.action, Action::Session(_)) {
        let mut badge = el().key("badge").size(16.0, 16.0).child(avatar(p, &e.description));
        if e.open {
            badge = badge.child(el().key("open").absolute(-6.0, 2.0).size(2.0, 12.0).radius(2.0).fill(k.marker));
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
        .fill(if active { k.active } else { Color::TRANSPARENT })
        .on_hover(move |s: &mut App| s.switcher.activate(hover.clone()))
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.switcher_choose(&key, tx))
        .child(main.child(words));
    if let Some(at) = e.updated {
        r = r.child(text(model::ago(now, at)).key("meta").font(UI).color(p.muted));
    } else if !e.hint.is_empty() {
        r = r.child(spacer()).child(text(e.hint).key("hint").font(UI).color(p.faint));
    }
    r
}

fn no_motion() -> SpringPair {
    SpringPair { enter: Spring::new(0.001, 0.0), exit: Spring::new(0.001, 0.0) }
}

pub fn view(s: &App, p: &Pal, cx: &Cx<App>) -> [El; 2] {
    let k = ink(p.dark);
    let win = cx.layout("window").unwrap_or([0.0, 0.0, 1280.0, 820.0]);
    let (w, h) = ((win[2] - 24.0).min(640.0), (win[3] - 96.0).min(480.0));
    let q = s.switcher.query();
    let entries = s.switcher_entries();
    let active = s.switcher_active(&entries);

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
                .placeholder("Search commands and sessions", p.faint)
                .caret(p.text, p.selection)
                .on_edit(|s: &mut App| &mut s.switcher.search)
                .on_input_key(|s: &mut App, ev, tx| s.switcher_nav(ev, tx)),
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
            section = section.child(entry_row(p, k, e, active.as_deref() == Some(e.key.as_str()), q, s.now_ms));
        }
        results = results.child(section);
        i = end;
    }
    if entries.is_empty() {
        let state = if s.switcher.loading.as_deref() == Some(q) {
            "Loading"
        } else if s.switcher.failed.as_deref() == Some(q) {
            "Request failed"
        } else {
            "No results found"
        };
        results = results.child(col().key("switcher-empty").w_full().min_h(120.0).center().child(text(state).font(UI).color(p.muted)));
    }

    let dialog = col()
        .key("switcher")
        .role("dialog")
        .label("Search everything")
        .w(w)
        .h(h)
        .radius(12.0)
        .fill(p.bg_base)
        .border(0.5, p.ring)
        .shadow(8.0, 16.0, k.shadow_far)
        .shadow(4.0, 8.0, k.shadow_near)
        .clip_soft(0.0)
        .transition(Transition::opacity().springs(no_motion()))
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
        );

    // Overrides the default fade: the palette opens and closes on the keypress. (`tx.instant()`
    // would also skip it, but an instant unmount of the focused search drops the composer's
    // restored focus.)
    let scrim = el().key("switcher-scrim").absolute(0.0, 0.0).w_full().h_full().fill(k.scrim).transition(Transition::opacity().springs(no_motion()));
    [scrim, dialog.absolute(((win[2] - w) / 2.0).round(), ((win[3] - h) / 2.0).round())]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Mods, Runtime};

    fn app() -> App {
        // Never App::new: tests must not probe a server.
        let mut app = App::empty();
        app.load_fixture(crate::FIXTURE);
        app
    }

    fn runtime() -> Runtime<App> {
        let mut app = app();
        crate::open_scene(&mut app);
        app.finish_stream();
        let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
        rt.on_key = Some(crate::on_key);
        rt.poll = Some(crate::poll);
        rt.frame(0.0);
        rt
    }

    fn press(rt: &mut Runtime<App>, key: Key, mods: Mods) {
        rt.key(KeyEvent { key, mods });
        rt.frame(1.0 / 60.0);
    }

    fn key(rt: &mut Runtime<App>, k: Key) {
        press(rt, k, Mods::default());
    }

    fn cmd_k(rt: &mut Runtime<App>) {
        press(rt, Key::Command('k'), Mods { cmd: true, ..Default::default() });
    }

    fn keys(a: &App) -> Vec<String> {
        a.switcher_entries().into_iter().map(|e| e.key).collect()
    }

    fn query(a: &mut App, q: &str) {
        a.switcher.search = TextEdit::new(q);
    }

    #[test]
    fn empty_lists_commands_then_the_newest_sessions() {
        let mut a = app();
        a.open_switcher();
        let e = a.switcher_entries();
        let groups: Vec<&str> = e.iter().map(|e| e.group).collect();
        assert_eq!(groups, [["Commands"; 3].as_slice(), ["Recent sessions"; 5].as_slice()].concat());
        assert_eq!(e[0].title, "New session");
        // Roots only, newest first: the subagent session never lists.
        assert_eq!(e[3].key, "session:ses_fx0001aaaaaaaaaaaaaaaaaa");
        assert_eq!(e[4].key, "session:ses_fx0003aaaaaaaaaaaaaaaaaa");
        assert!(!keys(&a).iter().any(|k| k.contains("fx0002")));
        assert_eq!(e[3].description, "lumen");
    }

    #[test]
    fn a_query_ranks_open_tabs_then_the_newest_then_commands() {
        let mut a = app();
        a.open_switcher();
        // Only the oldest match is open as a tab.
        a.tabs = vec![crate::tabs::Tab { id: "ses_fx0012aaaaaaaaaaaaaaaaaa".into(), title: String::new(), directory: String::new() }];
        query(&mut a, "THE");
        let k = keys(&a);
        assert_eq!(k[0], "session:ses_fx0012aaaaaaaaaaaaaaaaaa");
        assert_eq!(k[1], "session:ses_fx0001aaaaaaaaaaaaaaaaaa");
        assert!(a.switcher_entries().iter().all(|e| e.group == "Session"));
        // Sessions lead commands; a command matches by title.
        query(&mut a, "sidebar");
        assert_eq!(keys(&a), ["session:ses_fx0003aaaaaaaaaaaaaaaaaa", "command:sidebar"]);
        // Project names and ids match; the "Session" group name doesn't.
        query(&mut a, "harbor");
        assert_eq!(keys(&a), ["session:ses_fx0006aaaaaaaaaaaaaaaaaa", "session:ses_fx0007aaaaaaaaaaaaaaaaaa"]);
        query(&mut a, "fx0009");
        assert_eq!(keys(&a), ["session:ses_fx0009aaaaaaaaaaaaaaaaaa"]);
        query(&mut a, "session");
        assert!(keys(&a).iter().all(|k| k == "command:new" || a.switcher_entries().iter().any(|e| &e.key == k && e.title.to_lowercase().contains("session"))));
        query(&mut a, "zzz-nothing");
        assert!(keys(&a).is_empty());
    }

    #[test]
    fn server_finds_join_the_matches() {
        let mut a = app();
        a.open_switcher();
        let mut far = a.sessions[0].clone();
        far.id = "ses_far000000000000000000".into();
        far.title = "An old reconnect fix".into();
        far.updated = 1;
        a.switcher.loaded.push(far);
        query(&mut a, "reconnect");
        assert_eq!(keys(&a).last().map(String::as_str), Some("session:ses_far000000000000000000"));
        a.switcher_choose("session:ses_far000000000000000000", &mut Tx::default());
        assert_eq!(a.route, Route::Session("ses_far000000000000000000".into()));
        assert!(a.tabs.iter().any(|t| t.id == "ses_far000000000000000000" && t.title == "An old reconnect fix"));
    }

    #[test]
    fn moves_wrap_and_jump_sections() {
        let s = ["a", "a", "a", "b", "b"];
        assert_eq!(target(&s, 4, Move::Next), Some(0));
        assert_eq!(target(&s, 0, Move::Previous), Some(4));
        assert_eq!(target(&s, 1, Move::NextSection), Some(3));
        assert_eq!(target(&s, 3, Move::NextSection), Some(0));
        assert_eq!(target(&s, 4, Move::PreviousSection), Some(3));
        assert_eq!(target(&s, 3, Move::PreviousSection), Some(0));
        assert_eq!(target(&s, 0, Move::PreviousSection), Some(3));
        assert_eq!(target(&s, 2, Move::Last), Some(4));
        assert_eq!(target(&[], 0, Move::Next), None);
        let ev = |key, mods| KeyEvent { key, mods };
        assert_eq!(move_for(&ev(Key::Down, Mods { alt: true, ..Default::default() })), Some(Move::NextSection));
        assert_eq!(move_for(&ev(Key::Up, Mods { shift: true, ..Default::default() })), None);
    }

    #[test]
    fn cmd_k_arrows_enter_and_escape() {
        let mut rt = runtime();
        let before = rt.state.route.clone();
        cmd_k(&mut rt);
        assert!(rt.state.switcher.open);
        assert!(rt.state.switcher.search.focused);
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("switcher")));
        // First row active; Down steps, Up wraps from the top to the last row.
        let active = |rt: &Runtime<App>| rt.state.switcher_active(&rt.state.switcher_entries());
        assert_eq!(active(&rt).as_deref(), Some("command:new"));
        key(&mut rt, Key::Down);
        assert_eq!(active(&rt).as_deref(), Some("command:home"));
        key(&mut rt, Key::Up);
        key(&mut rt, Key::Up);
        assert_eq!(active(&rt), rt.state.switcher_entries().last().map(|e| e.key.clone()));
        // Typing starts again from the first match.
        key(&mut rt, Key::Text("footnote".into()));
        assert_eq!(active(&rt).as_deref(), Some("session:ses_fx0010aaaaaaaaaaaaaaaaaa"));
        key(&mut rt, Key::Enter);
        assert!(!rt.state.switcher.open);
        assert_eq!(rt.state.route, Route::Session("ses_fx0010aaaaaaaaaaaaaaaaaa".into()));
        assert_ne!(rt.state.route, before);
        // ⌘K toggles; Escape closes and gives the composer its focus back.
        rt.update(|s, _| s.composer.focused = true);
        rt.frame(1.0 / 60.0);
        cmd_k(&mut rt);
        assert!(rt.state.switcher.open && !rt.state.composer.focused);
        assert_eq!(rt.state.switcher.search.text, "", "it opens empty");
        key(&mut rt, Key::Escape);
        assert!(!rt.state.switcher.open);
        assert!(rt.state.composer.focused);
        cmd_k(&mut rt);
        cmd_k(&mut rt);
        assert!(!rt.state.switcher.open);
        assert_eq!(rt.state.route, Route::Session("ses_fx0010aaaaaaaaaaaaaaaaaa".into()));
    }

    #[test]
    fn a_click_opens_a_row_and_outside_closes() {
        let mut rt = runtime();
        cmd_k(&mut rt);
        for _ in 0..40 {
            rt.frame(1.0 / 60.0);
        }
        let (x, y) = rt.find("sw-session:ses_fx0003aaaaaaaaaaaaaaaaaa").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.route, Route::Session("ses_fx0003aaaaaaaaaaaaaaaaaa".into()));
        assert!(!rt.state.switcher.open);
        cmd_k(&mut rt);
        rt.pointer_down(10.0, 400.0);
        rt.pointer_up(10.0, 400.0);
        rt.frame(1.0 / 60.0);
        assert!(!rt.state.switcher.open);
        assert_eq!(rt.state.route, Route::Session("ses_fx0003aaaaaaaaaaaaaaaaaa".into()));
    }

    #[test]
    fn it_opens_and_closes_instantly() {
        let mut rt = runtime();
        let info = |rt: &Runtime<App>, k: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k));
        rt.frame(1.0 / 60.0);
        cmd_k(&mut rt);
        rt.frame(1.0 / 60.0);
        let d = info(&rt, "switcher").unwrap();
        assert!((d.scale - 1.0).abs() < 1e-3 && (d.opacity - 1.0).abs() < 1e-3, "first frame is final: scale {} opacity {}", d.scale, d.opacity);
        assert!((info(&rt, "switcher-scrim").unwrap().opacity - 1.0).abs() < 1e-3);
        key(&mut rt, Key::Escape);
        rt.frame(1.0 / 60.0);
        assert!(info(&rt, "switcher").is_none() && info(&rt, "switcher-scrim").is_none());
    }

    #[test]
    fn queries_encode_for_the_url() {
        assert_eq!(encode("a b&c/é"), "a%20b%26c%2F%C3%A9");
    }

    #[test]
    fn a_failed_search_says_so() {
        let mut a = app();
        a.open_switcher();
        query(&mut a, "zzz");
        a.switcher.loading = Some("zzz".into());
        let _ = a.switcher.found_tx.send(Found { query: "zzz".into(), sessions: vec![], failed: true });
        a.switcher.searched = "zzz".into();
        assert!(a.poll_switcher(&mut Tx::default()));
        assert_eq!(a.switcher.failed.as_deref(), Some("zzz"));
        assert_eq!(a.switcher.loading, None);
    }

}

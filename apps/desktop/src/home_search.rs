//! Session search from Home (`home/sessions/search.ts`, `home/sessions/view.tsx`):
//! - ⌘F on Home (and ⌘B opening Home from a session) focuses Home's session search.
//! - Typing filters Home's sessions by title and project name, and looks up a pasted `ses_…` id
//!   on the focused server (`GET /api/session/:id`), which heads the results while it's found.
//! - ↑ / ↓ step through matching rows, ↩ opens the highlighted session, Esc clears and blurs; a
//!   press outside the search clears it too, while Tab only moves focus on.

use crate::pickers::PickerHost as _;

use opencode_kit::model::{self, Project, SessionInfo};
use rune::machine::{Attempt, Effects, Seq};
use rune::{El, Key, KeyEvent, Latest, Play, TextEdit, Tx, Waker, col};

use crate::mentions::list::{Highlight, cycle};
use crate::palette::Pal;
use crate::stories::{Story, Trigger, replay, stage};
use crate::{App, Route, Source};

/// Home lists at most this many sessions (`HOME_SESSION_LIMIT`, `home/sessions/index.ts`).
pub(crate) const HOME_SESSION_LIMIT: usize = 64;

/// `HOME_SESSION_LOOKUP_TIMEOUT_MS`.
const LOOKUP_TIMEOUT: f64 = 10.0;

/// Home's session search: Desktop's `HomeSessionSearchState` (`home/sessions/search-machine.ts:
/// 6-24`). Its value is the field's text (`App::search`), its highlight SM-7's [`Highlight`].
#[derive(Clone, Debug, Default)]
pub struct Search {
    /// The server and project the results belong to: an exact match from another is never shown.
    scope: String,
    connected: bool,
    /// The field, or anything else in the search, has focus.
    focused: bool,
    /// The trimmed text the status was settled for.
    query: String,
    seq: Seq,
    pub status: Status,
}

#[derive(Clone, Debug, Default)]
pub enum Status {
    #[default]
    Idle,
    /// A typed session id is being looked up on the focused server, until `due` on the app clock.
    LookingUp { attempt: Attempt, id: String, due: f64 },
    /// The lookup found it: it heads the results until the text, server or project changes.
    Found(SessionInfo),
}

#[derive(Clone, Debug)]
pub enum Event<'a> {
    /// `focused`.
    Focused,
    /// `blurred`: focus left the search by the keys.
    Blurred,
    /// `closed`: Escape, a press outside it, or a result opening.
    Closed,
    /// `input.changed` and `scope.changed`: what the field holds, and where it searches.
    Synced { query: &'a str, scope: &'a str, connected: bool, now: f64 },
    /// `lookup.resolved` (and `lookup.failed`, as `None`).
    Resolved { attempt: Attempt, session: Option<SessionInfo> },
    /// `lookup.timed-out` once the clock reaches `now`.
    Tick { now: f64 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// `lookup.start`.
    Lookup { attempt: Attempt, id: String },
    /// `lookup.abort`.
    Abort,
    /// The field empties and nothing is highlighted.
    Clear,
    /// The highlight goes back to the first result.
    Unhighlight,
}

impl Search {
    /// `transitionHomeSessionSearch`.
    pub fn step(&mut self, event: Event) -> Effects<Effect> {
        match event {
            Event::Focused => {
                self.focused = true;
                Effects::none()
            }
            Event::Blurred => {
                self.focused = false;
                Effects::none()
            }
            Event::Closed => {
                self.focused = false;
                self.query.clear();
                let mut effects = self.settle(0.0);
                effects.push(Effect::Clear);
                effects
            }
            Event::Synced { query, scope, connected, now } => {
                if query == self.query && scope == self.scope && connected == self.connected {
                    return Effects::none();
                }
                let rescoped = scope != self.scope;
                let retyped = query != self.query;
                let reconnected = connected && !self.connected;
                (self.query, self.scope, self.connected) = (query.to_owned(), scope.to_owned(), connected);
                if retyped || rescoped || (reconnected && matches!(self.status, Status::Idle)) {
                    let mut effects = self.settle(now);
                    if retyped || rescoped {
                        effects.push(Effect::Unhighlight);
                    }
                    return effects;
                }
                if !connected && matches!(self.status, Status::LookingUp { .. }) {
                    self.status = Status::Idle;
                    return Effects::one(Effect::Abort);
                }
                Effects::none()
            }
            Event::Resolved { attempt, session } => {
                if !matches!(&self.status, Status::LookingUp { attempt: a, .. } if *a == attempt) {
                    return Effects::none();
                }
                self.status = session.map_or(Status::Idle, Status::Found);
                Effects::none()
            }
            Event::Tick { now } => {
                if !matches!(&self.status, Status::LookingUp { due, .. } if now + 1e-9 >= *due) {
                    return Effects::none();
                }
                self.status = Status::Idle;
                Effects::one(Effect::Abort)
            }
        }
    }

    /// `settle`: a pending lookup goes, and a query that looks like a session id is looked up
    /// while the server is connected.
    fn settle(&mut self, now: f64) -> Effects<Effect> {
        let mut effects = Effects::none();
        if matches!(self.status, Status::LookingUp { .. }) {
            effects.push(Effect::Abort);
        }
        if !looks_like_session_id(&self.query) || !self.connected {
            self.status = Status::Idle;
            return effects;
        }
        let attempt = self.seq.next();
        self.status = Status::LookingUp { attempt, id: self.query.clone(), due: now + LOOKUP_TIMEOUT };
        effects.push(Effect::Lookup { attempt, id: self.query.clone() });
        effects
    }

    /// `selectSearchOpen`.
    pub fn open(&self) -> bool {
        self.focused && !self.query.is_empty()
    }

    /// `selectSearchExact`.
    pub fn exact(&self) -> Option<&SessionInfo> {
        match &self.status {
            Status::Found(session) => Some(session),
            _ => None,
        }
    }

    /// What tells one status from another.
    fn mark(&self) -> (u8, Option<Attempt>, Option<String>) {
        match &self.status {
            Status::Idle => (0, None, None),
            Status::LookingUp { attempt, .. } => (1, Some(*attempt), None),
            Status::Found(session) => (2, None, Some(session.id.clone())),
        }
    }

    /// When a pending lookup times out.
    fn due(&self) -> Option<f64> {
        match self.status {
            Status::LookingUp { due, .. } => Some(due),
            _ => None,
        }
    }
}

pub struct HomeSearch {
    pub search: Search,
    active: Highlight,
    /// The Home row under the pointer: it shows its worktree and branch (`HomeSessionLocation`).
    pub(crate) hovered: Option<String>,
    /// Alt is held: every row shows its worktree and branch (`alt-hold.ts`, Desktop only).
    pub(crate) alt_held: bool,
    /// The list's order while the pointer is on it (`view::home::list`).
    pub(crate) hold: crate::view::HomeHold,
    /// The exact match, as a list: what [`Search::exact`] holds, for readers that look sessions up.
    pub(crate) loaded: Vec<SessionInfo>,
    /// The id lookup on its worker: its attempt and what it found.
    lookup: Latest<(Attempt, Option<SessionInfo>)>,
}

impl Default for HomeSearch {
    fn default() -> Self {
        HomeSearch { search: Search::default(), active: Highlight::default(), hovered: None, alt_held: false, hold: crate::view::HomeHold::Free, loaded: Vec::new(), lookup: Latest::default() }
    }
}

impl HomeSearch {
    pub fn set_waker(&mut self, waker: Waker) {
        self.lookup = Latest::new(waker);
    }

    pub fn activate(&mut self, id: String) {
        self.active.set(id);
    }

    /// The query changed: the first result lights (search-machine.ts:87).
    pub fn query_changed(&mut self) {
        self.active.clear();
    }
}

/// Desktop's `looksLikeSessionID`: `value.length > 20 && value.startsWith("ses_")`.
pub fn looks_like_session_id(s: &str) -> bool {
    s.len() > 20 && s.starts_with("ses_")
}

impl App {
    pub fn focus_home_search(&mut self, tx: &mut Tx) {
        self.close_picker(false);
        self.project_state.home_menu_open = false;
        self.search.focused = true;
        let len = self.search.text.len();
        self.search.set_selection(0, len);
        self.home_search_step(Event::Focused, tx);
        tx.instant();
    }

    pub fn clear_home_search(&mut self, keep_focus: bool, tx: &mut Tx) {
        self.home_search_step(Event::Closed, tx);
        self.search.focused = keep_focus;
        if keep_focus {
            self.home_search_step(Event::Focused, tx);
        }
        tx.instant();
    }

    /// Steps the search's machine and carries out what it asks.
    fn home_search_step(&mut self, event: Event, tx: &mut Tx) {
        for effect in self.home_search.search.step(event) {
            match effect {
                Effect::Lookup { attempt, id } => {
                    let Source::Live { server, .. } = &*self.source else { continue };
                    let server = server.clone();
                    self.home_search.lookup.request(id, move |id| {
                        let found = server.get(&format!("/api/session/{id}")).ok().map(|v| model::parse_session(v.get("data").unwrap_or(&v)));
                        // An archived session isn't one Home finds (`lookup`).
                        (attempt, found.filter(|s| s.archived.is_none()))
                    });
                }
                Effect::Abort => self.home_search.lookup.cancel(),
                Effect::Clear => {
                    self.search.clear();
                    self.search.cancel_composition();
                    self.search.focused = false;
                    self.home_search.active.clear();
                }
                Effect::Unhighlight => self.home_search.active.clear(),
            }
        }
        self.home_search.loaded = self.home_search.search.exact().cloned().into_iter().collect();
        if let Some(due) = self.home_search.search.due() {
            tx.wake_at(due);
        }
    }

    /// A person's root sessions Home lists for `selected` project, newest first: at most
    /// `HOME_SESSION_LIMIT` (64, `home/sessions/index.ts`). An automation's sessions
    /// fold away (`view::home::list`). Search never narrows them; it shows its matches in its own panel.
    pub fn home_sessions<'a>(&'a self, selected: Option<&Project>) -> Vec<&'a SessionInfo> {
        self.home_roots(selected).filter(|s| !crate::view::is_automation(&s.title)).take(HOME_SESSION_LIMIT).collect()
    }

    /// The search panel's results (`search.ts`): sessions whose `"{title} {project}"` holds the
    /// trimmed query, after the session a typed id found when it isn't one of them.
    pub fn home_search_results<'a>(&'a self, selected: Option<&Project>) -> Vec<&'a SessionInfo> {
        let q = self.search.text.trim();
        if q.is_empty() {
            return vec![];
        }
        let scope = crate::switcher::search_index::Scope::Home { project: selected.map(|p| p.id.clone()) };
        let records: Vec<&SessionInfo> = self.find_sessions(scope, q, 50).into_iter().filter(|(score, _, _)| *score >= opencode_search::SUBSTRING).filter_map(|(_, _, id)| self.search_session(&id)).filter(|s| s.archived.is_none()).collect();
        match self.home_search.search.exact().filter(|exact| !records.iter().any(|r| r.id == exact.id)) {
            Some(exact) => std::iter::once(exact).chain(records).collect(),
            None => records,
        }
    }

    pub(crate) fn home_roots<'a>(&'a self, selected: Option<&Project>) -> impl Iterator<Item = &'a SessionInfo> {
        let selected = selected.map(|p| p.id.clone());
        self.merged_roots(&[])
            .into_iter()
            .filter(move |ses| selected.as_ref().is_none_or(|id| self.project_for_session(ses).is_some_and(|p| &p.id == id)))
    }

    /// Whether the results panel shows: while the search has focus and text (`selectSearchOpen`).
    pub fn home_search_open(&self) -> bool {
        self.home_search.search.open() && !self.search.text.trim().is_empty()
    }

    /// Whether a typed id is being looked up, or a large list's index is still building, which
    /// shows the panel's loader (`searchLoading`).
    pub fn home_search_loading(&self) -> bool {
        matches!(self.home_search.search.status, Status::LookingUp { .. }) || (!self.search.text.trim().is_empty() && self.session_search_building())
    }

    /// The highlighted result while the panel shows: the one last pointed or stepped to, else
    /// the first (`search.ts`'s `active`).
    pub fn home_search_active(&self, ids: &[String]) -> Option<String> {
        if !self.home_search_open() || ids.is_empty() {
            return None;
        }
        self.home_search.active.resolve(ids.iter().map(String::as_str))
    }

    pub fn home_search_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if self.search.is_composing() {
            return false;
        }
        if ev.key == Key::Escape {
            self.clear_home_search(false, tx);
            return true;
        }
        let m = ev.mods;
        if !m.alt && !m.cmd && !m.ctrl && self.home_search_open() {
            let selected = self.home_selected().cloned();
            let ids: Vec<String> = self.home_search_results(selected.as_ref()).into_iter().map(|s| s.id.clone()).collect();
            match ev.key {
                Key::Down | Key::Up if !m.shift && !ids.is_empty() => {
                    let active = self.home_search_active(&ids);
                    let from = ids.iter().position(|id| Some(id) == active.as_ref());
                    let next = cycle(ids.len(), from, if ev.key == Key::Down { 1 } else { -1 });
                    let picked = ids[next].clone();
                    self.home_search.activate(picked.clone());
                    self.prefetch_session(&picked);
                    tx.instant();
                    tx.scroll_into_view_in("home-search-list", format!("sr-{picked}"));
                    return true;
                }
                Key::Enter if !m.shift => {
                    if let Some(id) = self.home_search_active(&ids) {
                        if self.session(&id).is_none()
                            && let Some(ses) = self.home_search.search.exact().filter(|s| s.id == id).cloned()
                        {
                            self.sessions.push(ses);
                        }
                        self.clear_home_search(false, tx);
                        self.open(&id, tx);
                    }
                    return true;
                }
                _ => {}
            }
        }
        self.switcher_key(ev, tx) || self.shortcut(ev, tx)
    }

    /// Keeps the search up with its field, focus, scope and lookup.
    pub fn poll_home_search(&mut self, tx: &mut Tx) -> bool {
        let mut changed = self.release_home_hold(tx);
        if let Some((attempt, session)) = self.home_search.lookup.take() {
            self.home_search_step(Event::Resolved { attempt, session }, tx);
            tx.instant();
            changed = true;
        }
        // The search goes with Home (`disposed`): back on Home, it starts empty.
        if self.route != Route::Home {
            if self.home_search.search.focused || !self.home_search.search.query.is_empty() || !self.search.text.is_empty() {
                self.home_search_step(Event::Closed, tx);
                self.home_search.search = Search::default();
            }
            return changed;
        }
        if self.settings.open {
            return changed;
        }
        // Focus anywhere in the search keeps it open; leaving it by the keys keeps its text, a
        // press anywhere else clears it (`blur`, and the document `pointerdown` that closes it).
        let within = self.search.focused || tx.focused() == Some("search-clear");
        if within != self.home_search.search.focused {
            let event = if within { Event::Focused } else if tx.focus_visible() { Event::Blurred } else { Event::Closed };
            self.home_search_step(event, tx);
            tx.instant();
            changed = true;
        }
        let selection = crate::projects::home::selection(self);
        let scope = format!("{}\0{}", selection.server.as_ref().map_or("", |k| k.as_str()), selection.directory.as_deref().unwrap_or(""));
        let key = crate::projects::home::server(self);
        let connected = matches!(*self.source, Source::Live { .. }) && self.home_state.servers.iter().any(|v| v.key == key && v.healthy());
        let query = self.search.text.trim().to_owned();
        let now = *self.clock;
        let before = self.home_search.search.mark();
        self.home_search_step(Event::Synced { query: &query, scope: &scope, connected, now: now }, tx);
        self.home_search_step(Event::Tick { now: now }, tx);
        changed | (self.home_search.search.mark() != before)
    }
}

/// Storybook story: `Home/Session search`.
fn home_story_view(s: &App, p: &Pal) -> El {
    col()
        .key("story-home")
        .w_full()
        .max_w(920.0)
        .h_full()
        .max_h(680.0)
        .radius(10.0)
        .fill(p.bg_base)
        .box_shadow(Pal::ring_out(p.ring))
        .shadow(1.0, 2.0, p.shadow)
        .clip_soft(0.0)
        .child(crate::view::home(s, p, [920.0, 0.0], [0.0, 0.0]))
}

const STORY_KEYS: &[Trigger] = &[
    replay(story_open),
    Trigger { key: 'n', label: "Next result", run: |s, tx| { let _ = s.home_search_key(&KeyEvent { key: Key::Down, mods: Default::default() }, tx); } },
    Trigger { key: 'c', label: "Clear or fill query", run: story_toggle },
];

const STORY_VARIANTS: &[(&str, usize)] = &[("By title", 0), ("By project", 1), ("No results", 2)];

fn story_open(s: &mut App, tx: &mut Play<App>) {
    *s.route = Route::Home;
    let q = match *s.variant {
        1 => "harbor",
        2 => "notpresent",
        _ => "reconnect",
    };
    *s.search = TextEdit::new(q);
    s.search.focused = true;
    s.composer.focused = false;
    tx.instant();
}

fn story_toggle(s: &mut App, tx: &mut Play<App>) {
    if s.search.text.is_empty() {
        *s.search = TextEdit::new("reconnect");
        s.search.focused = true;
        tx.instant();
    } else {
        s.clear_home_search(false, tx);
    }
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Home/Session search", scene: |s| stage(s, home_story_view), knobs: &[], keys: STORY_KEYS, variants: STORY_VARIANTS },
]);

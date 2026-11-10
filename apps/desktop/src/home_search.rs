//! Session search from Home (`home/sessions/search.ts`, `home/sessions/view.tsx`):
//! - ⌘F on Home (and ⌘B opening Home from a session) focuses Home's session search.
//! - Typing filters sessions by title, project name and session id, queries the server's
//!   `/api/session?parentID=null&archived=false&limit=50&search=` after 100 ms, and looks up
//!   pasted `ses_…` ids (`GET /api/session/:id`).
//! - ↑ / ↓ step through matching rows, ↩ opens the highlighted session, Esc clears and blurs.

use crate::pickers::PickerHost as _;
use std::time::Duration;

use opencode_kit::model::{self, Project, SessionInfo};
use rune::{El, Key, KeyEvent, Latest, Play, TextEdit, Tx, Waker, col};

use crate::mentions::list::{Highlight, cycle, upsert_sessions};
use crate::palette::Pal;
use crate::stories::{Story, Trigger, replay, stage};
use crate::switcher::encode;
use crate::view::title_of;
use crate::{App, Route, Source};

pub(crate) struct Found {
    sessions: Vec<SessionInfo>,
}

/// Home lists at most this many sessions (`HOME_SESSION_LIMIT`, `home/sessions/index.ts`).
pub(crate) const HOME_SESSION_LIMIT: usize = 64;

pub struct HomeSearch {
    active: Highlight,
    /// The Home row under the pointer: it shows its worktree and branch (`HomeSessionLocation`).
    pub(crate) hovered: Option<String>,
    /// Alt is held: every row shows its worktree and branch (`alt-hold.ts`, Desktop only).
    pub(crate) alt_held: bool,
    /// The list's order while the pointer is on it (`view::home::list`).
    pub(crate) hold: Option<crate::view::HomeHold>,
    /// Sessions fetched from the server while searching on Home.
    pub(crate) loaded: Vec<SessionInfo>,
    searched: String,
    pub(crate) fetch: Latest<Found>,
}

impl Default for HomeSearch {
    fn default() -> Self {
        HomeSearch {
            active: Highlight::default(),
            hovered: None,
            alt_held: false,
            hold: None,
            loaded: Vec::new(),
            searched: String::new(),
            fetch: Latest::default().debounce(Duration::from_millis(100)),
        }
    }
}

impl HomeSearch {
    pub fn set_waker(&mut self, waker: Waker) {
        self.fetch = Latest::new(waker).debounce(Duration::from_millis(100));
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
        tx.instant();
    }

    pub fn clear_home_search(&mut self, keep_focus: bool, tx: &mut Tx) {
        self.search.clear();
        self.search.cancel_composition();
        self.search.focused = keep_focus;
        self.home_search.active.clear();
        self.home_search.searched.clear();
        self.home_search.fetch.cancel();
        tx.instant();
    }

    /// A person's root sessions Home lists for `selected` project, newest first: at most
    /// `HOME_SESSION_LIMIT` (64, `home/sessions/index.ts`). An automation's sessions
    /// fold away (`view::home::list`). Search never narrows them; it shows its matches in its own panel.
    pub fn home_sessions<'a>(&'a self, selected: Option<&Project>) -> Vec<&'a SessionInfo> {
        self.home_roots(selected).filter(|s| !crate::view::is_automation(&s.title)).take(HOME_SESSION_LIMIT).collect()
    }

    /// The search panel's results (`search.ts`): sessions whose `"{title} {project}"` holds the
    /// trimmed query, or whose id it is.
    pub fn home_search_results<'a>(&'a self, selected: Option<&Project>) -> Vec<&'a SessionInfo> {
        let q = self.search.text.trim().to_lowercase();
        if q.is_empty() {
            return vec![];
        }
        self.home_roots(selected)
            .filter(|ses| ses.id.to_lowercase() == q || format!("{} {}", title_of(ses), self.project_label(ses)).to_lowercase().contains(&q))
            .collect()
    }

    pub(crate) fn home_roots<'a>(&'a self, selected: Option<&Project>) -> impl Iterator<Item = &'a SessionInfo> {
        let selected = selected.map(|p| p.id.clone());
        self.merged_roots(&self.home_search.loaded)
            .into_iter()
            .filter(move |ses| selected.as_ref().is_none_or(|id| self.project_for_session(ses).is_some_and(|p| &p.id == id)))
    }

    /// Whether the results panel shows: while the field has focus and text (`selectSearchOpen`).
    pub fn home_search_open(&self) -> bool {
        self.search.focused && !self.search.text.trim().is_empty()
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
            let selected = self.project_state.home_selected.as_deref().and_then(|id| self.project(id)).cloned();
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
                            && let Some(ses) = self.home_search.loaded.iter().find(|s| s.id == id).cloned()
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

    pub fn poll_home_search(&mut self, tx: &mut Tx) -> bool {
        let mut changed = self.release_home_hold(tx);
        if let Some(f) = self.home_search.fetch.take() {
            upsert_sessions(&mut self.home_search.loaded, f.sessions);
            tx.instant();
            changed = true;
        }
        if self.route != Route::Home || self.settings.open {
            return changed;
        }
        let trimmed = self.search.text.trim();
        if trimmed == self.home_search.searched {
            return changed;
        }
        let q = trimmed.to_owned();
        self.home_search.searched = q.clone();
        self.home_search.fetch.cancel();
        let Source::Live { server, .. } = &self.source else { return true };
        if q.is_empty() {
            return true;
        }
        let server = server.clone();
        self.home_search.fetch.request(q, move |q| {
            let path = format!("/api/session?parentID=null&archived=false&limit=50&search={}", encode(&q));
            let listed = server.get(&path);
            let exact = looks_like_session_id(&q)
                .then(|| server.get(&format!("/api/session/{q}")).ok())
                .flatten()
                .map(|v| model::parse_session(v.get("data").unwrap_or(&v)));
            let mut sessions: Vec<SessionInfo> = exact.into_iter().collect();
            if let Ok(v) = &listed {
                sessions.extend(model::parse_sessions(v));
            }
            Found { sessions }
        });
        true
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
    s.route = Route::Home;
    let q = match s.variant {
        1 => "harbor",
        2 => "notpresent",
        _ => "reconnect",
    };
    s.search = TextEdit::new(q);
    s.search.focused = true;
    s.composer.focused = false;
    tx.instant();
}

fn story_toggle(s: &mut App, tx: &mut Play<App>) {
    if s.search.text.is_empty() {
        s.search = TextEdit::new("reconnect");
        s.search.focused = true;
        tx.instant();
    } else {
        s.clear_home_search(false, tx);
    }
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Home/Session search", scene: |s| stage(s, home_story_view), knobs: &[], keys: STORY_KEYS, variants: STORY_VARIANTS },
]);

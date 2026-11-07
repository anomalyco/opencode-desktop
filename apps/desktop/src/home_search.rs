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

pub struct HomeSearch {
    active: Highlight,
    /// Sessions fetched from the server while searching on Home.
    pub(crate) loaded: Vec<SessionInfo>,
    searched: String,
    pub(crate) fetch: Latest<Found>,
}

impl Default for HomeSearch {
    fn default() -> Self {
        HomeSearch {
            active: Highlight::default(),
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

    pub fn activate(&mut self, id: String, query: &str) {
        self.active.set(id, query.to_owned());
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
        self.search.preedit = None;
        self.search.focused = keep_focus;
        self.home_search.active.clear();
        self.home_search.searched.clear();
        self.home_search.fetch.cancel();
        tx.instant();
    }

    /// Root sessions shown on Home for `selected` project and the current `s.search` query.
    pub fn home_sessions<'a>(&'a self, selected: Option<&Project>) -> Vec<&'a SessionInfo> {
        let q = self.search.text.trim().to_lowercase();
        self.merged_roots(&self.home_search.loaded)
            .into_iter()
            .filter(|ses| selected.is_none_or(|proj| self.project_for_session(ses).is_some_and(|p| p.id == proj.id)))
            .filter(|ses| {
                if q.is_empty() {
                    return true;
                }
                let title = title_of(ses).to_lowercase();
                let proj = self.project_label(ses).to_lowercase();
                let id = ses.id.to_lowercase();
                title.contains(&q) || proj.contains(&q) || id.contains(&q)
            })
            .take(60)
            .collect()
    }

    /// Highlighted session ID on Home while searching (`None` when the search field is empty).
    pub fn home_search_active(&self, nav_ids: &[String]) -> Option<String> {
        if self.search.text.trim().is_empty() || nav_ids.is_empty() {
            return None;
        }
        self.home_search.active.resolve(&self.search.text, nav_ids.iter().map(String::as_str))
    }

    pub fn home_search_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if self.search.preedit.is_some() {
            return false;
        }
        if ev.key == Key::Escape {
            self.clear_home_search(false, tx);
            return true;
        }
        let m = ev.mods;
        if !m.alt && !m.cmd && !m.ctrl && !self.search.text.trim().is_empty() {
            let selected = self.project_state.home_selected.as_deref().and_then(|id| self.project(id)).cloned();
            let ids: Vec<String> = self.home_sessions(selected.as_ref()).into_iter().map(|s| s.id.clone()).collect();
            match ev.key {
                Key::Down | Key::Up if !m.shift && !ids.is_empty() => {
                    let active = self.home_search_active(&ids);
                    let from = ids.iter().position(|id| Some(id) == active.as_ref());
                    let next = cycle(ids.len(), from, if ev.key == Key::Down { 1 } else { -1 });
                    let picked = ids[next].clone();
                    let query = self.search.text.clone();
                    self.home_search.activate(picked.clone(), &query);
                    self.prefetch_session(&picked);
                    tx.instant();
                    tx.scroll_into_view_in("home", format!("r-{picked}"));
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
        let mut changed = false;
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
        .child(crate::view::home(s, p, 920.0))
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
    s.search.focused = false;
    s.composer.focused = false;
    tx.instant();
}

fn story_toggle(s: &mut App, tx: &mut Play<App>) {
    if s.search.text.is_empty() {
        s.search = TextEdit::new("reconnect");
        s.search.focused = false;
        tx.instant();
    } else {
        s.clear_home_search(false, tx);
    }
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story { path: "Home/Session search", scene: |s| stage(s, home_story_view), knobs: &[], keys: STORY_KEYS, variants: STORY_VARIANTS },
]);

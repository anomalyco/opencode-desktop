//! Full text (better-search.md tier 2): message text kept on this device, searched by the storage
//! lane's tables (`opencode_storage::native::search`, STORAGE-4). They hold the copy of each
//! recently shown session (its last 200 messages), every tab's draft and the queued prompts, and
//! answer a query with up to 250 of the newest rows that match; `opencode_search::fulltext` ranks
//! them, two at most a session. The palette and Home's search each keep one, which asks for a
//! query of two characters or more; the newest question wins (`SearchSurface`).
//!
//! A profile without the tables (schema 2 off, `RUNE_STORAGE_V2`; storage in memory; the browser)
//! answers each question with an error: titles and the loaded sessions are still searched, and the
//! surface says message text isn't.

use std::sync::{Arc, Mutex};

use opencode_search::fulltext::{self, Kind, Snippet};

use crate::App;

/// The fewest characters a question has: one letter matches nearly every message.
const MIN_CHARS: usize = 2;
/// Characters of context a snippet shows around its first match.
const SNIPPET: usize = 90;
/// Rows a surface shows from full text, after its title matches.
pub(crate) const ROWS: usize = 20;

/// A ranked row: the session, what matched and its line of context.
pub(crate) struct Row {
    pub session: String,
    pub kind: Kind,
    pub snippet: Snippet,
    /// The session's other rows that matched, left out.
    pub more: usize,
}

/// What a surface shows from full text.
pub(crate) enum Shown {
    Nothing,
    Rows(Arc<Vec<Row>>),
    /// The profile has no search tables: message text isn't searched.
    Off,
}

/// The storage lane's answer to `query`, as it lands.
struct Landed {
    query: String,
    hits: Result<Vec<Stored>, String>,
}

/// A row as storage keeps it.
struct Stored {
    server: String,
    session: String,
    entity: String,
    kind: String,
    title: String,
    body: String,
    updated_ms: i64,
}

/// One surface's full text: nothing asked (no query, or under [`MIN_CHARS`]); a question out,
/// showing what the last one found meanwhile; answered; or off (the profile has no tables).
#[derive(Default)]
enum State {
    #[default]
    Idle,
    Asking { query: String, shown: Last },
    Answered { query: String, rows: Arc<Vec<Row>> },
    Off { query: String },
}

/// What the last answer was, shown while the next question is out so a key doesn't blink.
#[derive(Default)]
enum Last {
    #[default]
    Nothing,
    Rows(Arc<Vec<Row>>),
    Off,
}

enum Event {
    Typed(String),
    /// An answer for `query`: its rows ranked, or the reason there are none.
    Landed { query: String, rows: Result<Vec<Row>, String> },
}

enum Effect {
    Ask(String),
    Cancel,
}

impl State {
    fn query(&self) -> &str {
        match self {
            State::Idle => "",
            State::Asking { query, .. } | State::Answered { query, .. } | State::Off { query } => query,
        }
    }

    fn step(&mut self, event: Event) -> Option<Effect> {
        match (std::mem::take(self), event) {
            (state, Event::Typed(q)) if q == state.query() => {
                *self = state;
                None
            }
            (_, Event::Typed(q)) if q.chars().count() < MIN_CHARS => Some(Effect::Cancel),
            (state, Event::Typed(q)) => {
                let shown = match state {
                    State::Idle => Last::Nothing,
                    State::Asking { shown, .. } => shown,
                    State::Answered { rows, .. } => Last::Rows(rows),
                    State::Off { .. } => Last::Off,
                };
                *self = State::Asking { query: q.clone(), shown };
                Some(Effect::Ask(q))
            }
            (State::Asking { query, .. }, Event::Landed { query: answered, rows: Ok(rows) }) if answered == query => {
                *self = State::Answered { query, rows: Arc::new(rows) };
                None
            }
            (State::Asking { query, .. }, Event::Landed { query: answered, rows: Err(why) }) if answered == query => {
                eprintln!("opencode-rune: message text isn't searched: {why}");
                *self = State::Off { query };
                None
            }
            (state, Event::Landed { .. }) => {
                *self = state;
                None
            }
        }
    }

    fn shown(&self) -> Shown {
        match self {
            State::Idle => Shown::Nothing,
            State::Asking { shown: Last::Nothing, .. } => Shown::Nothing,
            State::Asking { shown: Last::Rows(rows), .. } | State::Answered { rows, .. } => Shown::Rows(rows.clone()),
            State::Asking { shown: Last::Off, .. } | State::Off { .. } => Shown::Off,
        }
    }
}

/// Which surface asks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Surface {
    Palette,
    Home,
}

#[derive(Default)]
pub(crate) struct FullText {
    state: State,
    /// Answers from the storage thread, for the window's next poll.
    inbox: Arc<Mutex<Vec<Landed>>>,
    #[cfg(not(target_arch = "wasm32"))]
    surface: opencode_storage::native::search::SearchSurface,
}

#[derive(Default)]
pub(crate) struct Surfaces {
    palette: FullText,
    home: FullText,
}

impl Surfaces {
    fn get(&self, which: Surface) -> &FullText {
        match which {
            Surface::Palette => &self.palette,
            Surface::Home => &self.home,
        }
    }

    fn get_mut(&mut self, which: Surface) -> &mut FullText {
        match which {
            Surface::Palette => &mut self.palette,
            Surface::Home => &mut self.home,
        }
    }
}

fn kind(stored: &str) -> Kind {
    match stored {
        "draft" => Kind::Draft,
        // A queued prompt is the user's words.
        "outbox" | "user" => Kind::UserPrompt,
        _ => Kind::AssistantReply,
    }
}

impl App {
    /// What `which` shows from full text for its query now.
    pub(crate) fn fulltext(&self, which: Surface) -> Shown {
        self.fulltext.get(which).state.shown()
    }

    /// The query each surface searches now, empty while it searches nothing.
    fn fulltext_query(&self, which: Surface) -> String {
        let q = match which {
            Surface::Palette if self.switcher.open && self.switcher.mode != crate::switcher::SwitcherMode::Commands => self.switcher.query(),
            Surface::Home if self.home_search_open() => self.search.text.as_str(),
            _ => "",
        };
        q.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Asks each surface's question when its query changed, and lands the answers: true when
    /// what shows changed.
    pub(crate) fn poll_fulltext(&mut self) -> bool {
        let mut changed = false;
        for which in [Surface::Palette, Surface::Home] {
            let landed: Vec<Landed> = std::mem::take(&mut *self.fulltext.get(which).inbox.lock().unwrap_or_else(|e| e.into_inner()));
            for Landed { query, hits } in landed {
                let rows = hits.map(|hits| self.fulltext_rank(which, &query, hits));
                self.fulltext.get_mut(which).state.step(Event::Landed { query, rows });
                changed = true;
            }
            let q = self.fulltext_query(which);
            match self.fulltext.get_mut(which).state.step(Event::Typed(q)) {
                Some(Effect::Ask(q)) => self.fulltext_ask(which, q),
                Some(Effect::Cancel) => {
                    #[cfg(not(target_arch = "wasm32"))]
                    self.fulltext.get(which).surface.cancel();
                    changed = true;
                }
                None => {}
            }
        }
        changed
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn fulltext_ask(&mut self, which: Surface, query: String) {
        use opencode_storage::native::search::Scope;
        let ft = self.fulltext.get(which);
        let (surface, inbox, wake) = (ft.surface.clone(), ft.inbox.clone(), (*self.waker).clone());
        let scope = Scope {
            server: Some(crate::servers::runtime::active(self).as_str().to_owned()),
            session: None,
            include_archived: which == Surface::Palette && self.switcher.mode == crate::switcher::SwitcherMode::Archived,
        };
        crate::storage::with_storage(move |storage| {
            let asked = query.clone();
            surface.query(storage, query, scope, move |_, result| {
                let hits = result
                    .map(|hits| {
                        hits.into_iter()
                            .map(|h| Stored { server: h.server, session: h.session, entity: h.entity, kind: h.kind, title: h.title, body: h.body, updated_ms: h.updated_ms })
                            .collect()
                    })
                    .map_err(|e| e.to_string());
                inbox.lock().unwrap_or_else(|e| e.into_inner()).push(Landed { query: asked, hits });
                if let Some(w) = wake {
                    w();
                }
            });
        });
    }

    /// The browser keeps no search tables.
    #[cfg(target_arch = "wasm32")]
    fn fulltext_ask(&mut self, which: Surface, query: String) {
        self.fulltext.get(which).inbox.lock().unwrap_or_else(|e| e.into_inner()).push(Landed { query, hits: Err("no search tables in the browser".into()) });
    }

    /// Ranks what storage found for `query`: the rows of root sessions the surface lists, with a
    /// session's title as the list has it now, each with its line of context.
    fn fulltext_rank(&self, which: Surface, query: &str, stored: Vec<Stored>) -> Vec<Row> {
        let server = crate::servers::runtime::active(self).as_str().to_owned();
        let archived = which == Surface::Palette && self.switcher.mode == crate::switcher::SwitcherMode::Archived;
        let home_project = (which == Surface::Home).then(|| self.home_selected_id()).flatten();
        let here = match &*self.route {
            crate::Route::Session { id, .. } => self.session(id).and_then(|s| self.project_for_session(s)).map(|p| p.id.clone()),
            _ => None,
        };
        let mut hits: Vec<fulltext::Hit> = Vec::new();
        for h in &stored {
            let Some(info) = (h.server == server).then(|| self.search_session(self.root_of(&h.session))).flatten() else { continue };
            let project = self.project_for_session(info).map(|p| p.id.as_str());
            if info.archived.is_some() != archived || home_project.as_deref().is_some_and(|p| Some(p) != project) {
                continue;
            }
            let title = if h.title.is_empty() { info.title.clone() } else { h.title.clone() };
            hits.push(fulltext::Hit {
                session: info.id.clone(),
                entity: Some(h.entity.clone()),
                kind: kind(&h.kind),
                title,
                body: h.body.clone(),
                updated_ms: h.updated_ms,
                active_project: project.is_some() && project == home_project.as_deref().or(here.as_deref()),
            });
        }
        fulltext::rank(&hits, query, *self.now_ms, 1)
            .into_iter()
            .filter_map(|r| {
                let hit = &hits[r.hit];
                let snippet = fulltext::snippet(&hit.body, query, SNIPPET)?;
                Some(Row { session: hit.session.clone(), kind: hit.kind, snippet, more: r.more })
            })
            .take(ROWS)
            .collect()
    }

    /// The row for `id` among `which`'s full text rows.
    pub(crate) fn fulltext_row(&self, which: Surface, id: &str) -> Option<(Arc<Vec<Row>>, usize)> {
        let Shown::Rows(rows) = self.fulltext(which) else { return None };
        let at = rows.iter().position(|r| r.session == id)?;
        Some((rows, at))
    }
}

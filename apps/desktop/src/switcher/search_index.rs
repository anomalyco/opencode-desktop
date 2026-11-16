//! One index of the root sessions' titles and projects, for the palette and Home's search,
//! kept with the server's shared data (`windows::peer`): every window of a server searches the
//! same one. A key only scans it. At 512 roots or more it is built on a worker while a surface
//! that searches is open (the palette, or Home's focused search) and the sessions changed since
//! (at most once a second while they keep changing), so opening a surface costs what it costs
//! at ten sessions; under 512 the first query builds it in place. A window at rest frees it
//! when nothing there searches and no window has asked it anything for `IDLE`.
//! Rows name their session by id, so an index a change behind still answers: a row whose
//! session has gone is skipped, and what a row shows is read from the session as it is now.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::Duration;

use opencode_kit::model::SessionInfo;
use opencode_search::SearchArena;
use rune::time::Instant;
use rune::{Latest, Tracked, Tx, Waker};

use crate::App;

/// Roots at which the index is kept up on a worker rather than built by the query that needs it.
const EAGER: usize = 512;
/// The newest rows each list keeps ready for an empty query.
const RECENT: usize = 64;
/// Rows kept per first letter.
const FIRST: usize = 64;
/// While the sessions keep changing, a new build at most this often (s).
const SETTLE: f64 = 1.0;
/// An index no window has asked anything for this long goes when a window rests (s): a
/// window's rest comes after 10 s with nothing to do.
const IDLE: Duration = Duration::from_secs(10);

/// The sessions' and projects' cells and versions, and SessionSearch::invalidate's count.
type Version = (u32, u64, u32, u64, u64);

/// What a surface searches in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    /// The ordinary command palette's title source (`palette.ts:localSessions`); unlike
    /// the dedicated fuzzy picker it offers only titles containing the search string.
    CommandTitles,
    /// The palette's sessions, or its archived ones.
    Palette { archived: bool },
    /// Home: unarchived roots, in the project picked (none: all).
    Home { project: Option<String> },
}

#[derive(Clone, Copy)]
struct Row {
    active_ms: i64,
    archived: bool,
    automation: bool,
    /// Index into `Built::projects`; `u32::MAX` for none.
    project: u32,
}

/// The sessions as the UI thread copied them: each root's `title\0project`, end to end, and its id.
/// An id is never matched: official matches a palette query against titles (`shell/commands/palette.ts:439-440`,
/// and the server's `title LIKE`, `core/src/session/store.ts:162`) and Home's against title and project
/// (`home/sessions/search.ts:80`); only Home looks a whole typed id up (`lookup`).
struct Snapshot {
    version: Version,
    text: String,
    ids: Vec<Box<str>>,
    ends: Vec<u32>,
    rows: Vec<Row>,
    projects: Vec<String>,
}

pub(crate) struct Built {
    version: Version,
    ids: Vec<Box<str>>,
    rows: Vec<Row>,
    projects: Vec<String>,
    arena: SearchArena,
    /// Newest first, automations left out: `[unarchived, archived]`.
    recent: [Vec<u32>; 2],
    /// The palette's best rows for each first letter typed (unarchived), made on the worker so a
    /// one-letter query doesn't rank every row on the key.
    first: Option<Box<[Vec<(i32, i64, u32)>; 26]>>,
}

impl Built {
    fn from(s: Snapshot, prepare: bool) -> Self {
        let mut arena = SearchArena::default();
        let mut start = 0;
        for &end in &s.ends {
            arena.push(&s.text[start..end as usize]);
            start = end as usize;
        }
        let ids = s.ids;
        arena.compact();
        let mut order: Vec<u32> = (0..s.rows.len() as u32).filter(|&i| !s.rows[i as usize].automation).collect();
        order.sort_unstable_by_key(|&i| std::cmp::Reverse(s.rows[i as usize].active_ms));
        let recent = [false, true].map(|archived| order.iter().copied().filter(|&i| s.rows[i as usize].archived == archived).take(RECENT).collect());
        let mut built = Built { version: s.version, ids, rows: s.rows, projects: s.projects, arena, recent, first: None };
        if prepare {
            let scope = Scope::Palette { archived: false };
            built.first = Some(Box::new(std::array::from_fn(|i| {
                let letter = char::from(b'a' + i as u8).to_string();
                built.rank(&scope, &letter, FIRST).into_iter().map(|(score, updated, row)| (score, updated, row as u32)).collect()
            })));
        }
        built
    }

    /// Up to `limit` session ids for `q` in `scope`, best first (newest first for an empty query).
    fn query(&self, scope: &Scope, q: &str, limit: usize) -> Vec<(i32, i64, Box<str>)> {
        if q.trim().is_empty() {

            let archived = match scope { Scope::CommandTitles => false, Scope::Palette { archived } => *archived, Scope::Home { .. } => return Vec::new() };
            return self.recent[usize::from(archived)].iter().take(limit).map(|&i| (0, self.rows[i as usize].active_ms, self.ids[i as usize].clone())).collect();
        }
        if let (Scope::Palette { archived: false } | Scope::CommandTitles, Some(first), [b]) = (scope, &self.first, q.trim().as_bytes())
            && b.is_ascii_alphabetic()
            && limit <= FIRST
        {
            return first[usize::from(b.to_ascii_lowercase() - b'a')].iter().take(limit).map(|&(score, updated, row)| (score, updated, self.ids[row as usize].clone())).collect();
        }
        self.rank(scope, q, limit).into_iter().map(|(score, updated, i)| (score, updated, self.ids[i].clone())).collect()
    }

    fn rank(&self, scope: &Scope, q: &str, limit: usize) -> Vec<(i32, i64, usize)> {
        let project = match scope {
            Scope::Home { project: Some(p) } => Some(self.projects.iter().position(|x| x == p).map_or(u32::MAX - 1, |i| i as u32)),
            _ => None,
        };
        let within = |r: &Row| match scope {
            Scope::CommandTitles => !r.archived,
            Scope::Palette { archived } => r.archived == *archived,
            Scope::Home { .. } => !r.archived && project.is_none_or(|p| r.project == p),
        };
        // The palette matches titles; Home titles and projects.
        let context = |i: usize| within(&self.rows[i]).then(|| (0, self.rows[i].active_ms));
        match scope {
            Scope::CommandTitles => self.arena.ranked_titles(q, limit, context),
            Scope::Palette { .. } => self.arena.ranked_scored_in(q, limit, 1, context),
            Scope::Home { .. } => self.arena.ranked_scored_in(q, limit, 2, context),
        }
    }
}

/// The index's life: none yet (or freed); a build out on the worker (answering from the last one
/// meanwhile); built; or built and waiting out `SETTLE` before the next build. `used`: when a
/// surface last asked it something, by the wall clock (a window rests 10 s after its last frame,
/// while the app clock stands still).
#[derive(Default)]
enum State {
    #[default]
    Empty,
    Building { shown: Option<Built>, used: Instant },
    Ready { built: Built, at: f64, used: Instant },
    Settling { built: Built, due: f64, used: Instant },
}

enum Event {
    /// A poll: the sessions' version now, and whether a surface of this window searches them
    /// (so a large list keeps the index up).
    Poll { version: Version, now: f64, wall: Instant, wanted: bool },
    Landed { built: Built, now: f64 },
    /// A small list's query built it in place.
    BuiltHere { built: Built, now: f64, wall: Instant },
    /// A surface asked it something.
    Asked { wall: Instant },
    /// The window rests; `wanted` as for [`Event::Poll`].
    Rest { wall: Instant, wanted: bool },
}

enum Effect {
    Build,
    WakeAt(f64),
    /// The index went: the build out, if any, isn't wanted.
    Cancel,
}

impl State {
    fn built(&self) -> Option<&Built> {
        match self {
            State::Empty => None,
            State::Building { shown, .. } => shown.as_ref(),
            State::Ready { built, .. } | State::Settling { built, .. } => Some(built),
        }
    }

    fn step(&mut self, event: Event) -> Option<Effect> {
        match (std::mem::take(self), event) {
            (State::Empty, Event::Poll { wanted: true, wall, .. }) => {
                *self = State::Building { shown: None, used: wall };
                Some(Effect::Build)
            }
            (State::Ready { built, at, used }, Event::Poll { version, now, wanted: true, .. }) if built.version != version => {
                if now >= at + SETTLE {
                    *self = State::Building { shown: Some(built), used };
                    Some(Effect::Build)
                } else {
                    let due = at + SETTLE;
                    *self = State::Settling { built, due, used };
                    Some(Effect::WakeAt(due))
                }
            }
            (State::Settling { built, due, used }, Event::Poll { now, wanted: true, .. }) if now >= due => {
                *self = State::Building { shown: Some(built), used };
                Some(Effect::Build)
            }
            (State::Building { shown, used }, Event::Landed { built, now }) => {
                *self = State::Ready { built, at: now, used };
                // The index it replaces is freed off the UI thread: 100,000 rows take ~10 ms.
                crate::transcript_index::retire(shown);
                None
            }
            (old, Event::BuiltHere { built, now, wall }) => {
                let used = match &old {
                    State::Empty => wall,
                    State::Building { used, .. } | State::Ready { used, .. } | State::Settling { used, .. } => *used,
                };
                *self = State::Ready { built, at: now, used };
                crate::transcript_index::retire(old);
                None
            }
            (mut state, Event::Asked { wall }) => {
                if let State::Building { used, .. } | State::Ready { used, .. } | State::Settling { used, .. } = &mut state {
                    *used = wall;
                }
                *self = state;
                None
            }
            (old @ (State::Building { used, .. } | State::Ready { used, .. } | State::Settling { used, .. }), Event::Rest { wall, wanted: false })
                if wall.duration_since(used) >= IDLE =>
            {
                crate::transcript_index::retire(old);
                Some(Effect::Cancel)
            }
            (state, _) => {
                *self = state;
                None
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct SessionSearch {
    state: RefCell<State>,
    worker: Latest<Built>,
    /// The last answer per surface, for the frames that ask again with the same query.
    last: RefCell<Vec<(Scope, String, Vec<(i32, i64, Box<str>)>)>>,
    /// Bumped by what changes a label without writing the sessions or projects (a worktree).
    generation: std::cell::Cell<u64>,
}

impl SessionSearch {
    pub fn set_waker(&mut self, waker: Waker) {
        self.worker.set_waker(waker);
    }

    /// What the rows show changed: the next poll (or a small list's next query) builds again.
    pub fn invalidate(&self) {
        self.generation.set(self.generation.get() + 1);
    }
}

impl App {
    fn session_version(&self) -> Version {
        let (s, p) = (&self.sessions, &self.projects);
        (Tracked::id(s), Tracked::version(s), Tracked::id(p), Tracked::version(p), self.session_search.generation.get())
    }

    /// A surface of this window searches the sessions: the palette (but for commands alone), or
    /// Home's search while it has focus or text.
    fn session_search_wanted(&self) -> bool {
        let palette = self.switcher.open && self.switcher.mode != super::SwitcherMode::Commands;
        let home = matches!(*self.route, crate::Route::Home) && (self.search.focused || !self.search.text.is_empty());
        palette || home
    }

    /// A large list's index isn't built yet: a query waits for it (and the surface says so).
    pub(crate) fn session_search_building(&self) -> bool {
        self.sessions.len() >= EAGER && self.session_search.state.borrow().built().is_none()
    }

    /// `App::rest`: the index goes when nothing here searches and no window asked it anything
    /// for `IDLE`; the next surface that searches builds it again.
    pub(crate) fn rest_session_search(&mut self) {
        let event = Event::Rest { wall: Instant::now(), wanted: self.session_search_wanted() };
        if let Some(Effect::Cancel) = self.session_search.state.get_mut().step(event) {
            self.session_search.worker.cancel();
            self.session_search.last.get_mut().clear();
        }
    }

    /// The newest `limit` roots, for an empty query while a large list's index builds: one pass
    /// over the sessions (about 1 ms at 100,000), kept for the frames after it.
    fn recent_sessions(&self, archived: bool, limit: usize) -> Vec<(i32, i64, Box<str>)> {
        let mut rows: Vec<(i64, &str)> = self.sessions.iter()
            .filter(|s| s.parent.is_none() && s.archived.is_some() == archived && !crate::view::is_automation(&s.title))
            .map(|s| (s.active_ms(), s.id.as_str()))
            .collect();
        if limit < rows.len() {
            rows.select_nth_unstable_by_key(limit, |r| std::cmp::Reverse(r.0));
            rows.truncate(limit);
        }
        rows.sort_by_key(|r| std::cmp::Reverse(r.0));
        rows.into_iter().map(|(active, id)| (0, active, id.into())).collect()
    }

    /// The roots as the worker needs them. About 2-3 ms at 100,000 roots, once per build.
    fn session_snapshot(&self) -> Snapshot {
        let mut labels: HashMap<(&str, &str), (String, u32)> = HashMap::new();
        let mut projects: Vec<String> = Vec::new();
        let roots = self.sessions.iter().filter(|s| s.parent.is_none());
        let (mut text, mut ends, mut rows, mut ids) = (String::new(), Vec::new(), Vec::new(), Vec::new());
        for s in roots {
            let (label, project) = labels.entry((&s.project, &s.directory)).or_insert_with(|| {
                let project = self.project_for_session(s).map_or(u32::MAX, |p| match projects.iter().position(|x| *x == p.id) {
                    Some(i) => i as u32,
                    None => {
                        projects.push(p.id.clone());
                        projects.len() as u32 - 1
                    }
                });
                (self.project_label(s), project)
            });
            let title = if s.title.is_empty() { opencode_i18n::t!("rune.session.untitled") } else { s.title.as_str() };
            let _ = write!(text, "{title}\0{label}");
            ids.push(s.id.as_str().into());
            ends.push(text.len() as u32);
            rows.push(Row { active_ms: s.active_ms(), archived: s.archived.is_some(), automation: crate::view::is_automation(&s.title), project: *project });
        }
        Snapshot { version: self.session_version(), text, ids, ends, rows, projects }
    }

    /// Keeps a large list's index up: lands a build, and starts the next when the sessions changed.
    pub(crate) fn poll_session_search(&mut self, now: f64, tx: &mut Tx) -> bool {
        let landed = self.session_search.worker.take();
        let changed = landed.is_some();
        if let Some(built) = landed {
            self.session_search.state.get_mut().step(Event::Landed { built, now });
            self.session_search.last.get_mut().clear();
        }
        // After the first frame, only for lists large enough to need it, while a surface searches.
        let wanted = self.view_runs.get() > 1 && self.sessions.len() >= EAGER && self.session_search_wanted();
        let version = self.session_version();
        match self.session_search.state.get_mut().step(Event::Poll { version, now, wall: Instant::now(), wanted }) {
            Some(Effect::Build) => {
                let snapshot = self.session_snapshot();
                // The index is the server's: whichever of its windows turns next lands it.
                if let Some(w) = crate::windows::peer::waker(self) {
                    self.session_search.worker.set_waker(w);
                }
                self.session_search.worker.request(snapshot, |s| Built::from(s, true));
            }
            Some(Effect::WakeAt(due)) => tx.wake_at(due),
            Some(Effect::Cancel) | None => {}
        }
        changed
    }

    /// Session ids for `q` in `scope`, with each one's score and recency: from the index as last
    /// built (a small list builds it here first).
    pub(crate) fn find_sessions(&self, scope: Scope, q: &str, limit: usize) -> Vec<(i32, i64, Box<str>)> {
        let ss = &self.session_search;
        let version = self.session_version();
        let stale = ss.state.borrow().built().is_none_or(|b| b.version != version);
        if stale && self.sessions.len() < EAGER {
            let built = Built::from(self.session_snapshot(), false);
            ss.state.borrow_mut().step(Event::BuiltHere { built, now: *self.clock, wall: Instant::now() });
            ss.last.borrow_mut().clear();
        }
        ss.state.borrow_mut().step(Event::Asked { wall: Instant::now() });
        if let Some((_, _, hits)) = ss.last.borrow().iter().find(|(s, query, _)| *s == scope && query == q) {
            return hits.clone();
        }
        let hits = match (ss.state.borrow().built(), &scope) {
            (Some(b), _) => b.query(&scope, q, limit),
            (None, Scope::Palette { archived }) if q.trim().is_empty() => self.recent_sessions(*archived, limit),
            (None, Scope::CommandTitles) if q.trim().is_empty() => self.recent_sessions(false, limit),
            (None, _) => Vec::new(),
        };
        let mut last = ss.last.borrow_mut();
        last.retain(|(s, _, _)| *s != scope);
        last.push((scope, q.to_owned(), hits.clone()));
        hits
    }

    /// `id`'s session as it is now, if it is still a root.
    pub(crate) fn search_session(&self, id: &str) -> Option<&SessionInfo> {
        self.session(id).filter(|s| s.parent.is_none())
    }
}

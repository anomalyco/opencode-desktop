//! One index of the root sessions' titles, projects and ids, for the palette and Home's search.
//! A key only scans it: at 512 roots or more it is built on a worker when the sessions change
//! (after the first frame, at most once a second while they keep changing), so opening a
//! surface costs what it costs at ten sessions; under 512 the first query builds it in place.
//! Rows name their session by id, so an index a change behind still answers: a row whose
//! session has gone is skipped, and what a row shows is read from the session as it is now.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;

use opencode_kit::model::SessionInfo;
use opencode_search::SearchArena;
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

/// The sessions' and projects' versions, and SessionSearch::invalidate's count.
type Version = (u64, u64, u64);

/// What a surface searches in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
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

/// The sessions as the UI thread copied them: each root's `title\0project`, then its id after
/// `opencode_search::EXACT` (an id matches only as typed), end to end.
struct Snapshot {
    version: Version,
    text: String,
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
        let mut ids = Vec::with_capacity(s.rows.len());
        let mut start = 0;
        for &end in &s.ends {
            let row = &s.text[start..end as usize];
            arena.push(row);
            ids.push(row.rsplit(opencode_search::EXACT).next().unwrap_or_default().into());
            start = end as usize;
        }
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

            let Scope::Palette { archived } = scope else { return Vec::new() };
            return self.recent[usize::from(*archived)].iter().take(limit).map(|&i| (0, self.rows[i as usize].active_ms, self.ids[i as usize].clone())).collect();
        }
        if let (Scope::Palette { archived: false }, Some(first), [b]) = (scope, &self.first, q.trim().as_bytes())
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
            Scope::Palette { archived } => r.archived == *archived,
            Scope::Home { .. } => !r.archived && project.is_none_or(|p| r.project == p),
        };
        self.arena.ranked_scored(q, limit, |i| within(&self.rows[i]).then(|| (0, self.rows[i].active_ms)))
    }
}

/// The index's life: none yet; a build out on the worker (answering from the last one meanwhile);
/// built; or built and waiting out `SETTLE` before the next build.
#[derive(Default)]
enum State {
    #[default]
    Empty,
    Building { shown: Option<Built> },
    Ready { built: Built, at: f64 },
    Settling { built: Built, due: f64 },
}

enum Event {
    /// A poll: the sessions' version now, and whether this window keeps the index up.
    Poll { version: Version, now: f64, eager: bool },
    Landed { built: Built, now: f64 },
    /// A small list's query built it in place.
    BuiltHere { built: Built, now: f64 },
}

enum Effect {
    Build,
    WakeAt(f64),
}

impl State {
    fn built(&self) -> Option<&Built> {
        match self {
            State::Empty => None,
            State::Building { shown } => shown.as_ref(),
            State::Ready { built, .. } | State::Settling { built, .. } => Some(built),
        }
    }

    fn step(&mut self, event: Event) -> Option<Effect> {
        match (std::mem::take(self), event) {
            (State::Empty, Event::Poll { eager: true, .. }) => {
                *self = State::Building { shown: None };
                Some(Effect::Build)
            }
            (State::Ready { built, at }, Event::Poll { version, now, eager: true }) if built.version != version => {
                if now >= at + SETTLE {
                    *self = State::Building { shown: Some(built) };
                    Some(Effect::Build)
                } else {
                    let due = at + SETTLE;
                    *self = State::Settling { built, due };
                    Some(Effect::WakeAt(due))
                }
            }
            (State::Settling { built, due }, Event::Poll { now, eager: true, .. }) if now >= due => {
                *self = State::Building { shown: Some(built) };
                Some(Effect::Build)
            }
            (old @ State::Building { .. }, Event::Landed { built, now }) | (old, Event::BuiltHere { built, now }) => {
                *self = State::Ready { built, at: now };
                // The index it replaces is freed off the UI thread: 100,000 rows take ~10 ms.
                crate::transcript_index::retire(old);
                None
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
        (Tracked::version(&self.sessions), Tracked::version(&self.projects), self.session_search.generation.get())
    }

    /// The roots as the worker needs them. About 2-3 ms at 100,000 roots, once per build.
    fn session_snapshot(&self) -> Snapshot {
        let mut labels: HashMap<(&str, &str), (String, u32)> = HashMap::new();
        let mut projects: Vec<String> = Vec::new();
        let roots = self.sessions.iter().filter(|s| s.parent.is_none());
        let (mut text, mut ends, mut rows) = (String::new(), Vec::new(), Vec::new());
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
            let _ = write!(text, "{title}\0{label}{}{}", opencode_search::EXACT, s.id);
            ends.push(text.len() as u32);
            rows.push(Row { active_ms: s.active_ms(), archived: s.archived.is_some(), automation: crate::view::is_automation(&s.title), project: *project });
        }
        Snapshot { version: self.session_version(), text, ends, rows, projects }
    }

    /// Keeps a large list's index up: lands a build, and starts the next when the sessions changed.
    pub(crate) fn poll_session_search(&mut self, now: f64, tx: &mut Tx) -> bool {
        let landed = self.session_search.worker.take();
        let changed = landed.is_some();
        if let Some(built) = landed {
            self.session_search.state.get_mut().step(Event::Landed { built, now });
            self.session_search.last.get_mut().clear();
        }
        // After the first frame, and only for lists large enough to need it.
        let eager = self.view_runs.get() > 1 && self.sessions.len() >= EAGER;
        let version = self.session_version();
        match self.session_search.state.get_mut().step(Event::Poll { version, now, eager }) {
            Some(Effect::Build) => {
                let snapshot = self.session_snapshot();
                self.session_search.worker.request(snapshot, |s| Built::from(s, true));
            }
            Some(Effect::WakeAt(due)) => tx.wake_at(due),
            None => {}
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
            ss.state.borrow_mut().step(Event::BuiltHere { built, now: *self.clock });
            ss.last.borrow_mut().clear();
        }
        if let Some((_, _, hits)) = ss.last.borrow().iter().find(|(s, query, _)| *s == scope && query == q) {
            return hits.clone();
        }
        let hits = ss.state.borrow().built().map(|b| b.query(&scope, q, limit)).unwrap_or_default();
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

//! The session page's machines: opening a session (the page it leaves holds until the new one
//! can paint), each session's history above its first page, and the "still trying" toast.
//!
//! - **Opening** follows OpenCode Desktop's first sync (`packages/app/src/session/timeline/
//!   sync-machine.ts`: `syncing → ready | failed`) with Rune's hold, so a fast page never shows a
//!   loading frame.
//! - **History** follows `packages/app/src/session/timeline/history-machine.ts`: `idle → loading →
//!   idle | failed (2 s cooldown) | exhausted`, a page that never settles failing after 30 s, and
//!   a page that adds nothing stopping the paging instead of looping. Each session has its own,
//!   keyed by (server, session).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use opencode_kit::servers::Route;
use rune::machine::{Attempt, Effects, Seq};
use rune::time::Instant;

/// The latest page size fetched on session open.
pub const LATEST: usize = 12;
/// Older pages as you scroll up: Desktop's `messagePageLimit`.
pub const OLDER: usize = 20;

/// Server identifier keying per-server session histories (server base URL or empty for fixtures).
pub type ServerId = String;

/// Cache key identifying a session snapshot at a given `updated` timestamp.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub id: String,
    pub updated: i64,
}

/// How long the page a click leaves holds before the new one shows loading.
pub const HOLD: Duration = Duration::from_millis(150);

/// Opening session `id` from the page `from`.
pub struct Opening<R = Route> {
    pub id: String,
    pub from: R,
    pub phase: OpenPhase,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OpenPhase {
    /// `from` stays on screen until the target can paint or the hold passes. `started` is the
    /// wall clock (a live window), `clock` the app's (a story's); `received`: its page landed.
    Held { started: Instant, clock: f64, received: bool },
    /// The hold passed: the target shows, loading.
    Loading { received: bool },
    /// Its read failed: the target shows the error.
    Failed { error: String },
    Partial { error: String, head: Arc<str> },
}

pub enum OpenEvent {
    /// Its latest page landed.
    Received,
    /// The hold passed.
    Expired,
    Failed(String),
    Partial(String, Arc<str>),
}

impl<R> Opening<R> {
    pub fn new(id: &str, from: R, received: bool, clock: f64) -> Opening<R> {
        Opening { id: id.to_owned(), from, phase: OpenPhase::Held { started: Instant::now(), clock, received } }
    }

    /// True when what shows changed.
    pub fn step(&mut self, event: OpenEvent) -> bool {
        let next = match (&self.phase, event) {
            (OpenPhase::Held { started, clock, .. }, OpenEvent::Received) => OpenPhase::Held { started: *started, clock: *clock, received: true },
            (OpenPhase::Loading { .. }, OpenEvent::Received) => OpenPhase::Loading { received: true },
            (OpenPhase::Held { received, .. }, OpenEvent::Expired) => OpenPhase::Loading { received: *received },
            (OpenPhase::Held { .. } | OpenPhase::Loading { .. }, OpenEvent::Failed(error)) => OpenPhase::Failed { error },
            (_, OpenEvent::Partial(error, head)) => OpenPhase::Partial { error, head },
            _ => return false,
        };
        let changed = matches!((&self.phase, &next), (OpenPhase::Held { .. }, OpenPhase::Loading { .. } | OpenPhase::Failed { .. }))
            || matches!(next, OpenPhase::Partial { .. });
        self.phase = next;
        changed
    }

    /// Its latest page landed.
    pub fn received(&self) -> bool {
        matches!(self.phase, OpenPhase::Held { received: true, .. } | OpenPhase::Loading { received: true })
    }

    /// The target shows (loading, or its error), not the page it left.
    pub fn loading(&self) -> bool {
        !matches!(self.phase, OpenPhase::Held { .. })
    }

    pub fn error(&self) -> Option<&str> {
        match &self.phase {
            OpenPhase::Failed { error } | OpenPhase::Partial { error, .. } => Some(error),
            _ => None,
        }
    }

    pub fn partial(&self) -> Option<&str> {
        match &self.phase {
            OpenPhase::Partial { head, .. } => Some(head),
            _ => None,
        }
    }

    /// The hold has passed: on the wall clock for a live window, whose last poll may have been
    /// minutes before the click; on the app's clock for a story's.
    pub fn expired(&self, live: bool, clock: f64) -> bool {
        match &self.phase {
            OpenPhase::Held { started, clock: at, .. } => if live { started.elapsed() >= HOLD } else { clock >= at + HOLD.as_secs_f64() },
            _ => false,
        }
    }
}

/// A page that never settles fails, so paging can't stay loading forever.
pub const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// After a failed page, the reader's scroll waits this long before it retries.
pub const COOLDOWN: Duration = Duration::from_secs(2);
/// The latest page gets its first frame before the history behind it is read.
pub const EXPAND_AFTER: Duration = Duration::from_millis(50);

/// One session's history above its latest page.
#[derive(Clone, Debug, PartialEq)]
pub enum History {
    /// The latest page painted; the first history page (`key`, before `cursor`) is read at `due`.
    Expanding { due: Instant, key: Key, cursor: String },
    /// That first page is on its way: its landing pins the timeline to its end.
    Filling { attempt: Attempt },
    /// More waits above `cursor`.
    Idle { cursor: String },
    /// An older page is on its way; `slow` once it outlasts the hold (its row shows).
    Loading { cursor: String, attempt: Attempt, at: Instant, slow: bool },
    /// The last page failed; the reader's scroll retries after `until`.
    Failed { cursor: String, until: Instant },
    /// Nothing more above: the server's cursor ran out, or a page added nothing.
    Exhausted,
}

pub enum HistoryEvent<'a> {
    /// The latest page painted, leaving `cursor` above it.
    Latest { key: Key, cursor: &'a str, now: Instant },
    /// The reader stands near the top.
    NearTop { now: Instant },
    /// The window polls.
    Tick { now: Instant },
    /// A history page landed with `cursor` above it (`None`: the start), adding `added` messages.
    Landed { cursor: Option<String>, added: usize },
    Failed { now: Instant },
    /// The transcript was replaced by a newer read that does not adjoin what was loaded, leaving
    /// `cursor` above it (`None`: the start): older pages continue from there.
    Replaced { cursor: Option<String> },
}

#[derive(Debug, PartialEq)]
pub enum HistoryEffect {
    /// Read `limit` messages before `cursor`.
    Read { key: Option<Key>, cursor: String, limit: usize },
    WakeAfter(Duration),
    /// Pin the timeline to its end (the first page above the latest).
    Settle,
    /// What shows changed (the loading row).
    Repaint,
}

/// The first history page's size: the latest page and it make Desktop's first 80.
const FIRST: usize = 80 - LATEST;

impl History {
    pub fn step(state: Option<&History>, seq: &mut Seq, event: HistoryEvent<'_>) -> (Option<History>, Effects<HistoryEffect>) {
        use HistoryEffect::*;
        let keep = || state.cloned();
        match (state, event) {
            (_, HistoryEvent::Replaced { cursor }) => (Some(cursor.map_or(History::Exhausted, |cursor| History::Idle { cursor })), Effects::none()),
            (None, HistoryEvent::Latest { key, cursor, now }) => {
                (Some(History::Expanding { due: now + EXPAND_AFTER, key, cursor: cursor.to_owned() }), Effects::one(WakeAfter(EXPAND_AFTER)))
            }
            (Some(History::Expanding { due, key, cursor }), HistoryEvent::Tick { now }) if *due <= now => {
                (Some(History::Filling { attempt: seq.next() }), Effects::one(Read { key: Some(key.clone()), cursor: cursor.clone(), limit: FIRST }))
            }
            (Some(History::Filling { .. }), HistoryEvent::Landed { cursor, added }) => (Some(above(cursor, added)), Effects::one(Settle)),
            (Some(History::Filling { .. }), HistoryEvent::Failed { .. }) => (Some(History::Exhausted), Effects::none()),
            (Some(History::Idle { cursor }), HistoryEvent::NearTop { now }) => (
                Some(History::Loading { cursor: cursor.clone(), attempt: seq.next(), at: now, slow: false }),
                Effects::two(Read { key: None, cursor: cursor.clone(), limit: OLDER }, WakeAfter(HOLD)),
            ),
            (Some(History::Failed { cursor, until }), HistoryEvent::NearTop { now }) if *until <= now => (
                Some(History::Loading { cursor: cursor.clone(), attempt: seq.next(), at: now, slow: false }),
                Effects::two(Read { key: None, cursor: cursor.clone(), limit: OLDER }, WakeAfter(HOLD)),
            ),
            (Some(History::Loading { .. }), HistoryEvent::Landed { cursor, added }) => (Some(above(cursor, added)), Effects::one(Repaint)),
            (Some(History::Loading { cursor, .. }), HistoryEvent::Failed { now }) => {
                (Some(History::Failed { cursor: cursor.clone(), until: now + COOLDOWN }), Effects::two(Repaint, WakeAfter(COOLDOWN)))
            }
            (Some(History::Loading { cursor, attempt, at, slow }), HistoryEvent::Tick { now }) => {
                let waited = now.saturating_duration_since(*at);
                if waited >= LOAD_TIMEOUT {
                    return (Some(History::Failed { cursor: cursor.clone(), until: now + COOLDOWN }), Effects::two(Repaint, WakeAfter(COOLDOWN)));
                }
                if !slow && waited >= HOLD {
                    return (
                        Some(History::Loading { cursor: cursor.clone(), attempt: *attempt, at: *at, slow: true }),
                        Effects::two(Repaint, WakeAfter(LOAD_TIMEOUT - waited)),
                    );
                }
                (keep(), Effects::none())
            }
            _ => (keep(), Effects::none()),
        }
    }
}

/// What a landed page leaves: its cursor, unless the server's ran out or the page added nothing
/// (a server that keeps handing out cursors can't loop).
fn above(cursor: Option<String>, added: usize) -> History {
    match cursor.filter(|_| added > 0) {
        Some(cursor) => History::Idle { cursor },
        None => History::Exhausted,
    }
}

/// Sizes of the older pages a new read of the newest page may read to reach the loaded messages
/// (`packages/client/src/solid/data.ts` `bridgePageLimits`). They grow: a short gap costs one
/// small page, and the longest gap that is closed costs three requests.
pub const BRIDGE: [usize; 3] = [20, 40, 140];

/// A newest page read again (every reconnect of the event stream) that does not reach the
/// loaded messages: more than a page arrived while its events were missed. Older pages are read
/// until they meet what is loaded; after a longer absence only the newest messages stay, as on
/// a first read. A transcript never skips messages (`data.ts` `session.message.sync`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bridge {
    /// The older page number `step`, before `cursor`, is on its way.
    Reading { step: usize, cursor: String },
}

pub enum BridgeEvent {
    /// The newest page landed apart from the loaded messages, leaving `cursor` above it.
    Apart { cursor: Option<String> },
    /// An older page landed, leaving `cursor` above it: `meets` when the pages read so far
    /// reach the loaded messages.
    Landed { meets: bool, cursor: Option<String> },
}

#[derive(Debug, PartialEq, Eq)]
pub enum BridgeEffect {
    /// Read `limit` messages before `cursor`.
    Read { cursor: String, limit: usize },
    /// The pages read reach the loaded messages: join them.
    Join,
    /// They do not: keep the pages read only.
    Replace,
}

impl Bridge {
    pub fn step(state: Option<&Bridge>, event: BridgeEvent) -> (Option<Bridge>, Effects<BridgeEffect>) {
        use BridgeEffect::*;
        let read = |step: usize, cursor: String| (Some(Bridge::Reading { step, cursor: cursor.clone() }), Effects::one(Read { cursor, limit: BRIDGE[step] }));
        match (state, event) {
            (_, BridgeEvent::Apart { cursor: Some(cursor) }) => read(0, cursor),
            // Nothing older on the server, and still apart: what was loaded is no longer there.
            (_, BridgeEvent::Apart { cursor: None }) => (None, Effects::one(Replace)),
            (Some(Bridge::Reading { .. }), BridgeEvent::Landed { meets: true, .. }) => (None, Effects::one(Join)),
            (Some(Bridge::Reading { step, .. }), BridgeEvent::Landed { meets: false, cursor: Some(cursor) }) if step + 1 < BRIDGE.len() => read(step + 1, cursor),
            (Some(Bridge::Reading { .. }), BridgeEvent::Landed { meets: false, .. }) => (None, Effects::one(Replace)),
            (None, BridgeEvent::Landed { .. }) => (None, Effects::none()),
        }
    }
}

/// Every session's history, by (server, session).
#[derive(Default)]
pub struct Histories {
    map: HashMap<(ServerId, String), History>,
    seq: Seq,
}

impl Histories {
    pub fn get(&self, server: &str, id: &str) -> Option<&History> {
        self.map.get(&(server.to_owned(), id.to_owned()))
    }

    pub fn step(&mut self, server: &str, id: &str, event: HistoryEvent<'_>) -> Effects<HistoryEffect> {
        let key = (server.to_owned(), id.to_owned());
        let (next, effects) = History::step(self.map.get(&key), &mut self.seq, event);
        match next {
            Some(next) => self.map.insert(key, next),
            None => self.map.remove(&key),
        };
        effects
    }

    /// The sessions whose history waits on a timer, on `server`.
    pub fn timed(&self, server: &str) -> Vec<String> {
        self.map
            .iter()
            .filter(|((s, _), h)| s == server && matches!(h, History::Expanding { .. } | History::Loading { .. }))
            .map(|((_, id), _)| id.clone())
            .collect()
    }

    /// A new pool reads every history again.
    pub fn clear(&mut self) {
        self.map.clear();
    }
}

/// The calm "still trying" toast after a silence that lasts. It says so once per silence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stall {
    /// The server answers, or its silence hasn't lasted long enough to mention.
    #[default]
    Quiet,
    /// Toast `toast` says so, until the server answers.
    Told { toast: u64 },
}

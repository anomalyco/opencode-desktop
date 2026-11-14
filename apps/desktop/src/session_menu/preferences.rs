//! Per-session preferences the Recent sidebar sets (`shell/state/session-preferences.ts`):
//! Hide takes a session out of Recent, Mute stops its notifications and its unread dot. Kept in
//! `opencode.global.dat`'s `session.preferences` as Desktop keeps it, `{ hidden, muted }`, each a
//! list of `${server}\u{0}${sessionID}` keys (`recentSessionKey`), so a session hidden or muted in
//! either app is in the other. Windows share the set; a deleted session's entries go.

use std::cell::RefCell;

use opencode_kit::servers::ServerKey;
use serde_json::{Value, json};

use crate::view::sidebar::state::key;
use crate::{App, storage};

const NAMESPACE: &str = "opencode.global.dat";
const KEY: &str = "session.preferences";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Hidden,
    Muted,
}

#[derive(Default)]
struct Prefs {
    hidden: Vec<String>,
    muted: Vec<String>,
}

impl Prefs {
    fn from_json(v: &Value) -> Prefs {
        let list = |k: &str| v[k].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
        Prefs { hidden: list("hidden"), muted: list("muted") }
    }

    fn to_json(&self) -> Value {
        json!({ "hidden": self.hidden, "muted": self.muted })
    }

    fn list(&mut self, kind: Kind) -> &mut Vec<String> {
        match kind {
            Kind::Hidden => &mut self.hidden,
            Kind::Muted => &mut self.muted,
        }
    }
}

/// Desktop's `persisted(...)` with its `ready` promise: nothing reads as set, and nothing
/// changes, until the stored value is in.
#[derive(Default)]
enum State {
    #[default]
    Waiting,
    Ready { prefs: Prefs, generation: u64 },
}

enum Event<'a> {
    Hydrate(Prefs),
    Set(Kind, &'a str, bool),
}

impl State {
    /// The value to store when it changed.
    fn step(&mut self, event: Event<'_>) -> Option<Value> {
        match (self, event) {
            (state @ Self::Waiting, Event::Hydrate(prefs)) => *state = Self::Ready { prefs, generation: 1 },
            (Self::Ready { prefs, generation }, Event::Set(kind, k, on)) => {
                let list = prefs.list(kind);
                if list.iter().any(|x| x == k) == on {
                    return None;
                }
                if on {
                    list.push(k.to_owned());
                } else {
                    list.retain(|x| x != k);
                }
                *generation += 1;
                return Some(prefs.to_json());
            }
            _ => {}
        }
        None
    }

    fn has(&self, kind: Kind, k: &str) -> bool {
        match self {
            Self::Ready { prefs, .. } => match kind {
                Kind::Hidden => prefs.hidden.iter().any(|x| x == k),
                Kind::Muted => prefs.muted.iter().any(|x| x == k),
            },
            Self::Waiting => false,
        }
    }
}

thread_local! { static PREFS: RefCell<State> = const { RefCell::new(State::Waiting) }; }

/// Reads the stored set once storage is up; true when this window has a change to draw.
pub(crate) fn poll(s: &mut App) -> bool {
    if s.checkout.boot != crate::servers::runtime::Boot::Ready && !crate::headless() {
        return false;
    }
    PREFS.with_borrow_mut(|state| {
        if matches!(state, State::Waiting) && storage::is_ready() {
            state.step(Event::Hydrate(storage::json(NAMESPACE, KEY).map(|v| Prefs::from_json(&v)).unwrap_or_default()));
        }
        let State::Ready { generation, .. } = state else { return false };
        if s.menus.preferences_generation == *generation {
            return false;
        }
        s.menus.preferences_generation = *generation;
        true
    })
}

pub(crate) fn hidden(server: &ServerKey, id: &str) -> bool {
    PREFS.with_borrow(|state| state.has(Kind::Hidden, &key(server.as_str(), id)))
}

pub(crate) fn muted(server: &ServerKey, id: &str) -> bool {
    PREFS.with_borrow(|state| state.has(Kind::Muted, &key(server.as_str(), id)))
}

/// Notifications skip a session muted itself or through its root (`host-apis.tsx`'s notify,
/// `notification.tsx`'s requests).
pub(crate) fn silenced(s: &App, server: &ServerKey, id: &str) -> bool {
    muted(server, id) || muted(server, s.root_of(id))
}

pub(crate) fn update(s: &mut App, kind: Kind, server: &ServerKey, id: &str, on: bool) {
    let k = key(server.as_str(), id);
    if let Some(value) = PREFS.with_borrow_mut(|state| state.step(Event::Set(kind, &k, on))) {
        storage::set_json(NAMESPACE, KEY, &value);
        poll(s);
        crate::windows::peer::touch(s);
    }
}

/// A deleted session's preferences go (`forget`).
pub(crate) fn forget(s: &mut App, server: &ServerKey, id: &str) {
    update(s, Kind::Hidden, server, id, false);
    update(s, Kind::Muted, server, id, false);
}

//! Manual unread is local to a client and scoped to the server and root session. The server's
//! viewed watermark cannot go backwards (`shell/notifications/notification.tsx`). Windows share
//! the set; SQLite saves it through the existing state writer. No boot read or new state file.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use opencode_kit::servers::ServerKey;
use crate::{App, storage};

const NAMESPACE: &str = "opencode.global.dat";
const KEY: &str = "notification.manual-unread";
type Marks = BTreeMap<String, BTreeSet<String>>;

#[derive(Default)]
enum State {
    #[default]
    Waiting,
    Ready { marks: Marks, generation: u64 },
}

enum Event<'a> { Hydrate(Marks), Set(&'a str, &'a str, bool) }

impl State {
    fn step(&mut self, event: Event<'_>) -> Option<serde_json::Value> {
        match (self, event) {
            (state @ Self::Waiting, Event::Hydrate(marks)) => { *state = Self::Ready { marks, generation: 1 }; }
            (Self::Ready { marks, generation }, Event::Set(server, session, on)) => {
                let changed = if on { marks.entry(server.to_owned()).or_default().insert(session.to_owned()) }
                    else { marks.get_mut(server).is_some_and(|set| set.remove(session)) };
                if changed {
                    if marks.get(server).is_some_and(BTreeSet::is_empty) { marks.remove(server); }
                    *generation += 1;
                    return Some(serde_json::json!(marks));
                }
            }
            _ => {}
        }
        None
    }
}

thread_local! { static MANUAL: RefCell<State> = const { RefCell::new(State::Waiting) }; }

pub(crate) fn poll(s: &mut App) -> bool {
    if s.checkout.boot != crate::servers::runtime::Boot::Ready && !crate::headless() { return false; }
    MANUAL.with_borrow_mut(|state| {
        if matches!(state, State::Waiting) && storage::is_ready() {
            let marks = storage::json(NAMESPACE, KEY).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
            state.step(Event::Hydrate(marks));
        }
        let State::Ready { generation, .. } = state else { return false };
        if s.menus.unread_generation == *generation { return false; }
        s.menus.unread_generation = *generation;
        true
    })
}

pub(crate) fn manual(s: &App, server: &ServerKey, id: &str) -> bool {
    s.menus.unread_generation != 0 && MANUAL.with_borrow(|state| matches!(state,
        State::Ready { marks, .. } if marks.get(server.as_str()).is_some_and(|set| set.contains(id))))
}

pub(crate) fn set(s: &mut App, server: &ServerKey, root: &str, on: bool) {
    if let Some(value) = MANUAL.with_borrow_mut(|state| state.step(Event::Set(server.as_str(), root, on))) {
        storage::set_json(NAMESPACE, KEY, &value);
        poll(s);
        crate::windows::peer::touch(s);
    }
}

pub(crate) fn read(s: &mut App, id: &str) {
    let root = s.root_of(id).to_owned();
    set(s, &crate::servers::runtime::current(), &root, false);
    if let Some(info) = s.sessions.iter_mut().find(|i| i.id == root) && info.unread() && let Some(idle) = info.idle {
        info.viewed = Some(idle);
        s.write(opencode_kit::client::Write::View { session: root, idle });
    }
}

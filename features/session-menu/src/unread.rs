//! Manual unread is local to a client and scoped to the server and root session. The server's
//! viewed watermark cannot go backwards (`shell/notifications/notification.tsx`). Windows share
//! the set; SQLite saves it through the existing state writer. No boot read or new state file.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use opencode_kit::client::Write;
use opencode_kit::servers::ServerKey;

use crate::host::SessionMenuHost;

pub const NAMESPACE: &str = "opencode.global.dat";
pub const KEY: &str = "notification.manual-unread";

type Marks = BTreeMap<String, BTreeSet<String>>;

#[derive(Default)]
enum State {
    #[default]
    Waiting,
    Ready { marks: Marks, generation: u64 },
}

enum Event<'a> {
    Hydrate(Marks),
    Set(&'a str, &'a str, bool),
}

impl State {
    fn step(&mut self, event: Event<'_>) -> Option<serde_json::Value> {
        match (self, event) {
            (state @ Self::Waiting, Event::Hydrate(marks)) => {
                *state = Self::Ready { marks, generation: 1 };
            }
            (Self::Ready { marks, generation }, Event::Set(server, session, on)) => {
                let changed = if on {
                    marks.entry(server.to_owned()).or_default().insert(session.to_owned())
                } else {
                    marks.get_mut(server).is_some_and(|set| set.remove(session))
                };
                if changed {
                    if marks.get(server).is_some_and(BTreeSet::is_empty) {
                        marks.remove(server);
                    }
                    *generation += 1;
                    return Some(serde_json::json!(marks));
                }
            }
            _ => {}
        }
        None
    }
}

thread_local! {
    static MANUAL: RefCell<State> = const { RefCell::new(State::Waiting) };
}

/// Hydrates manual unread marks once storage is ready; returns `true` when this window changed.
pub fn poll(host: &mut dyn SessionMenuHost) -> bool {
    if !host.boot_ready() && !host.is_headless() {
        return false;
    }
    let next = MANUAL.with_borrow_mut(|state| {
        if matches!(state, State::Waiting) && host.storage_ready() {
            let marks = host.storage_read_json(NAMESPACE, KEY).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
            state.step(Event::Hydrate(marks));
        }
        match state {
            State::Ready { generation, .. } => Some(*generation),
            State::Waiting => None,
        }
    });
    let Some(generation) = next else {
        return false;
    };
    if host.menus().unread_generation == generation {
        return false;
    }
    host.menus_mut().unread_generation = generation;
    true
}

/// Whether `(server, id)` is manually marked unread.
pub fn manual(host: &dyn SessionMenuHost, server: &ServerKey, id: &str) -> bool {
    host.menus().unread_generation != 0
        && MANUAL.with_borrow(|state| matches!(state, State::Ready { marks, .. } if marks.get(server.as_str()).is_some_and(|set| set.contains(id))))
}

/// Sets or clears the local manual-unread mark for `(server, root)`.
pub fn set(host: &mut dyn SessionMenuHost, server: &ServerKey, root: &str, on: bool) {
    if let Some(value) = MANUAL.with_borrow_mut(|state| state.step(Event::Set(server.as_str(), root, on))) {
        host.storage_write_json(NAMESPACE, KEY, &value);
        poll(host);
        host.touch_peers();
    }
}

/// Marks `id`'s root session as read on the currently checked-out server (clearing any manual
/// unread mark and advancing the server's viewed watermark if unread).
pub fn read(host: &mut dyn SessionMenuHost, id: &str) {
    let root = host.root_of(id).to_owned();
    let server = host.active_server();
    set(host, &server, &root, false);
    let mark_idle = host.session(&root).and_then(|info| info.unread().then_some(info.idle).flatten());
    if let Some(idle) = mark_idle {
        if let Some(sessions) = host.sessions_mut()
            && let Some(info) = sessions.iter_mut().find(|i| i.id == root)
        {
            info.viewed = Some(idle);
        }
        host.write_server(Write::View { session: root, idle });
    }
}

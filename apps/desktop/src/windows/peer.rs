//! The session data every window of one server shares, and the server's event stream. The
//! server's projects, sessions, transcripts, running sessions, stops, open requests and inboxes
//! are one copy for the process ([`ServerData`]), as Desktop's windows read one server's sync
//! store, so a session open in two windows loads once and updates in both. What each window
//! shows (tabs, page, panes, scroll, drafts, closed tabs) stays its own.
//!
//! The copy lives in the server's [`ServerPeer`] between turns. Rune runs one window's turn at a
//! time, and each turn starts and ends in `App::turn_scope`: [`enter`] moves the copy into the
//! window's own fields, [`leave`] moves it back, so code reads and writes plain fields with no
//! shared borrow (a turn that finds the copy out has a bug, and panics).
//!
//! The peer holds the server's one event stream. The first window to poll after events come
//! applies them to the copy once (`App::apply_shared`), and every window of the server, itself
//! included, then reacts to each in its own view (`App::react`), in order. A window that closes
//! or opens mid-stream loses nothing: the events wait in the peer, not in a window. A window
//! that changes the copy from what only it heard (its own reads' replies, a press of Stop)
//! marks it ([`touch`]), and the others draw it when its turn ends. The peer goes, stream and
//! data, with the last window to leave it.

#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use opencode_kit::client::{EventHub, Incoming};
use opencode_kit::model::{Inbox, Pending, Project, SessionInfo, Transcript};

use crate::events::Heard;
use crate::{App, Source};

/// One server's session data.
#[derive(Default)]
pub struct ServerData {
    projects: Vec<Project>,
    sessions: Vec<SessionInfo>,
    transcripts: HashMap<String, Transcript>,
    running: HashSet<String>,
    stops: crate::stop::Stops,
    requests: HashSet<(String, String)>,
    pending: Vec<Pending>,
    inboxes: HashMap<String, Inbox>,
}

impl ServerData {
    /// Trades places with the window's fields: no copies, no allocations.
    fn swap(&mut self, s: &mut App) {
        std::mem::swap(&mut self.projects, &mut s.projects);
        std::mem::swap(&mut self.sessions, &mut s.sessions);
        std::mem::swap(&mut self.transcripts, &mut s.transcripts);
        std::mem::swap(&mut self.running, &mut s.running);
        std::mem::swap(&mut self.stops, &mut s.stops);
        std::mem::swap(&mut self.requests, &mut s.requests);
        std::mem::swap(&mut self.pending, &mut s.pending);
        std::mem::swap(&mut self.inboxes, &mut s.inboxes);
    }

    /// The window's fields, which it no longer holds.
    fn take(s: &mut App) -> ServerData {
        let mut data = ServerData::default();
        data.swap(s);
        data
    }

    /// A window joins with what it read: what this copy lacks goes in (sessions and transcripts
    /// no other window loaded, its projects while none are listed).
    fn absorb(&mut self, mut from: ServerData) {
        if self.projects.is_empty() {
            self.projects = from.projects;
        }
        for info in from.sessions.drain(..) {
            if !self.sessions.iter().any(|x| x.id == info.id) {
                self.sessions.push(info);
            }
        }
        for (id, transcript) in from.transcripts {
            self.transcripts.entry(id).or_insert(transcript);
        }
        self.running.extend(from.running);
        self.requests.extend(from.requests);
        for p in from.pending {
            if !self.pending.iter().any(|x| x.id() == p.id()) {
                self.pending.push(p);
            }
        }
        for (id, inbox) in from.inboxes {
            self.inboxes.entry(id).or_insert(inbox);
        }
    }
}

/// Where the copy is.
enum Vault {
    /// In the peer, between turns.
    Home(ServerData),
    /// In the fields of window `token`, whose turn it is.
    Out { token: u64 },
}

/// Whether a window has drawn the copy as it is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Current,
    /// Another window changed it since.
    Stale,
}

/// One window of the server.
struct Member {
    token: u64,
    wake: Option<rune::Waker>,
    /// Stream events applied to the copy that it hasn't reacted to yet, in order.
    heard: VecDeque<Rc<Heard>>,
    view: View,
}

/// The server's event stream, while windows hold it (`ServerPeer`'s lifecycle: it opens with
/// the first window to join and ends with the last to leave, when the peer drops).
struct Stream {
    hub: EventHub,
    events: Receiver<Incoming>,
}

/// One server's shared data and stream, held by its windows.
pub struct ServerPeer {
    vault: RefCell<Vault>,
    stream: Stream,
    members: RefCell<Vec<Member>>,
    /// The members' wakers, for the stream's thread to wake them all.
    wakers: Arc<Mutex<Vec<(u64, rune::Waker)>>>,
}

/// Whether a window's turn changed the copy from what only it heard.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Edit {
    #[default]
    Clean,
    Dirty,
}

/// A window's place in its server's peer.
#[derive(Default)]
pub struct Link {
    peer: Option<Rc<ServerPeer>>,
    /// This window among the members.
    token: u64,
    edit: Edit,
}

thread_local! {
    /// Each server's peer, by its base URL, while a window holds it.
    static PEERS: RefCell<HashMap<String, Weak<ServerPeer>>> = RefCell::default();
    static TOKENS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// `App::turn_scope`: the window's turn starts, with its server's copy in its fields.
pub fn enter(s: &mut App) {
    let Some(peer) = s.store.peer.clone() else { return };
    let token = s.store.token;
    let mut vault = peer.vault.borrow_mut();
    match std::mem::replace(&mut *vault, Vault::Out { token }) {
        Vault::Home(mut data) => data.swap(s),
        Vault::Out { token: other } if other == token => {}
        Vault::Out { token: other } => panic!("window {token}'s turn found the server data in window {other}'s"),
    }
}

/// `App::turn_scope`: the turn ends; the copy goes back, and if the turn changed it from what
/// only it heard, the server's other windows draw it.
pub fn leave(s: &mut App) {
    let Some(peer) = s.store.peer.clone() else { return };
    *peer.vault.borrow_mut() = Vault::Home(ServerData::take(s));
    if std::mem::take(&mut s.store.edit) == Edit::Dirty {
        stale_others(&peer, s.store.token);
    }
}

fn stale_others(peer: &ServerPeer, token: u64) {
    let wake: Vec<_> = peer.members.borrow_mut().iter_mut().filter(|m| m.token != token).filter_map(|m| {
        m.view = View::Stale;
        m.wake.clone()
    }).collect();
    wake.iter().for_each(|w| w());
}

/// The window changed the copy from what only it heard (a reply, a press): the others draw it.
pub fn touch(s: &mut App) {
    if s.store.peer.is_some() {
        s.store.edit = Edit::Dirty;
    }
}

/// `poll`: another window changed the copy since this one drew it.
pub fn behind(s: &mut App) -> bool {
    let Some(peer) = &s.store.peer else { return false };
    let token = s.store.token;
    peer.members.borrow_mut().iter_mut().find(|m| m.token == token).is_some_and(|m| std::mem::replace(&mut m.view, View::Current) == View::Stale)
}

/// The window, connected to its server, joins that server's peer, or starts it: what it read
/// goes into the copy, and the copy is in its fields for the rest of the turn.
pub fn join(s: &mut App, wake: Option<rune::Waker>) {
    let Source::Live { server, .. } = &s.source else { return };
    let server = server.clone();
    let base = server.base.clone();
    if s.store.peer.as_ref().is_some_and(|p| p.stream.hub.base() == base) {
        return;
    }
    leave_server(s);
    let token = TOKENS.with(|t| {
        t.set(t.get() + 1);
        t.get()
    });
    let found = PEERS.with(|p| p.borrow().get(&base).and_then(Weak::upgrade));
    let peer = match found {
        Some(peer) => {
            let mut vault = peer.vault.borrow_mut();
            let Vault::Home(mut data) = std::mem::replace(&mut *vault, Vault::Out { token }) else {
                panic!("a window joined while another's turn held the server data");
            };
            data.absorb(ServerData::take(s));
            data.swap(s);
            drop(vault);
            peer
        }
        None => {
            let wakers: Arc<Mutex<Vec<(u64, rune::Waker)>>> = Arc::default();
            let all = wakers.clone();
            let hub = EventHub::start(server.clone(), crate::reconnect::STREAM_IDLE);
            let events = hub.subscribe(Arc::new(move || all.lock().unwrap_or_else(|e| e.into_inner()).iter().for_each(|(_, w)| w())));
            let peer = Rc::new(ServerPeer { vault: RefCell::new(Vault::Out { token }), stream: Stream { hub, events }, members: RefCell::default(), wakers });
            PEERS.with(|p| p.borrow_mut().insert(base, Rc::downgrade(&peer)));
            peer
        }
    };
    if let Some(w) = &wake {
        peer.wakers.lock().unwrap_or_else(|e| e.into_inner()).push((token, w.clone()));
    }
    peer.members.borrow_mut().push(Member { token, wake, heard: VecDeque::new(), view: View::Current });
    s.store = Link { peer: Some(peer), token, edit: Edit::Dirty };
}

/// The window leaves its server's peer (it closes, moves to another server, or loses it): it
/// keeps nothing of the copy, and the other windows keep it all. The last to leave ends the
/// peer, stream and data.
pub fn leave_server(s: &mut App) {
    let Some(peer) = s.store.peer.take() else { return };
    let token = s.store.token;
    let data = ServerData::take(s);
    let mut vault = peer.vault.borrow_mut();
    if matches!(*vault, Vault::Out { token: t } if t == token) {
        *vault = Vault::Home(data);
    }
    drop(vault);
    peer.members.borrow_mut().retain(|m| m.token != token);
    peer.wakers.lock().unwrap_or_else(|e| e.into_inner()).retain(|(t, _)| *t != token);
    if std::mem::take(&mut s.store.edit) == Edit::Dirty {
        stale_others(&peer, token);
    }
    drop(peer);
    PEERS.with(|p| p.borrow_mut().retain(|_, w| w.strong_count() > 0));
}

/// A torn-out tab's window starts in its opener's peer, so it shows the tab at once. `s` holds
/// nothing of its own yet, and isn't in a turn.
pub fn adopt(s: &mut App, from: &Link) {
    let Some(peer) = from.peer.clone() else { return };
    let token = TOKENS.with(|t| {
        t.set(t.get() + 1);
        t.get()
    });
    peer.members.borrow_mut().push(Member { token, wake: None, heard: VecDeque::new(), view: View::Current });
    s.store = Link { peer: Some(peer), token, edit: Edit::Clean };
}

/// `setup`, for an adopted window: the stream wakes it from now on.
pub fn set_waker(s: &App, wake: rune::Waker) {
    let Some(peer) = &s.store.peer else { return };
    let token = s.store.token;
    if let Some(m) = peer.members.borrow_mut().iter_mut().find(|m| m.token == token) {
        m.wake = Some(wake.clone());
    }
    let mut wakers = peer.wakers.lock().unwrap_or_else(|e| e.into_inner());
    wakers.retain(|(t, _)| *t != token);
    wakers.push((token, wake));
}

/// Whether the window holds a peer (it joined, or was adopted into one).
pub fn joined(s: &App) -> bool {
    s.store.peer.is_some()
}

/// The window's opener's place, for a window it opens.
pub fn link_of(s: &App) -> Link {
    Link { peer: s.store.peer.clone(), token: 0, edit: Edit::Clean }
}

/// Runs `f` on `s` in a turn of its own, for work outside the event loop's turns (a window's
/// first state, built before its first turn).
pub fn in_turn(s: &mut App, f: impl FnOnce(&mut App)) {
    enter(s);
    f(s);
    leave(s);
}

/// `poll`: the stream's new events, applied to the copy once by whichever window polls first,
/// then what this window hasn't reacted to yet, in order.
pub fn heard(s: &mut App) -> Vec<Rc<Heard>> {
    let Some(peer) = s.store.peer.clone() else { return Vec::new() };
    let fresh: Vec<Incoming> = peer.stream.events.try_iter().collect();
    if !fresh.is_empty() {
        let applied: Vec<Rc<Heard>> = fresh.into_iter().map(|m| Rc::new(s.apply_shared(m))).collect();
        let token = s.store.token;
        let wake: Vec<_> = peer.members.borrow_mut().iter_mut().filter_map(|m| {
            m.heard.extend(applied.iter().cloned());
            m.wake.clone().filter(|_| m.token != token)
        }).collect();
        wake.iter().for_each(|w| w());
    }
    let token = s.store.token;
    peer.members.borrow_mut().iter_mut().find(|m| m.token == token).map(|m| m.heard.drain(..).collect()).unwrap_or_default()
}

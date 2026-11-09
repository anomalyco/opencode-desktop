//! The session data every window of the app shares: the server's projects, sessions, running
//! sessions, open requests, inboxes and transcripts are one copy per server for the process, as
//! Desktop's windows read one server's sync store, so a session open in two windows loads once
//! and updates in both. What each window shows (tabs, page, panes, scroll, drafts, closed tabs)
//! stays its own.
//!
//! A window joins its server's copy once its first reads land (`launch::connect`), bringing
//! what it read; a torn-out tab's window starts in its opener's. Every window still hears every
//! stream event and reacts in its own view, but an event changes the shared copy once: the hub
//! numbers its events (`HUB_SEQ`), and only the first window to see a number applies what isn't
//! the same however often it lands (a transcript's text deltas). When a window changes the copy
//! from what only it heard (its own reads' replies), the others wake and draw it.

use std::cell::{Cell, RefCell, UnsafeCell};
use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use opencode_kit::model::{Inbox, Pending, Project, SessionInfo, Transcript};

use crate::{App, Source};

/// One piece of the session data, which windows of one server hold together.
///
/// It reads and writes like the value it holds. Windows live on the UI thread (`Rc` keeps it
/// there), which runs one window's code at a time, and a window reaches its pieces only through
/// its own `App`; so at any moment the data has one borrower, the window running, whose borrows
/// the compiler checks field by field as it would a plain field.
pub struct Shared<T>(Rc<UnsafeCell<T>>);

impl<T: Default> Default for Shared<T> {
    fn default() -> Self {
        Shared(Rc::new(UnsafeCell::new(T::default())))
    }
}

impl<T> Shared<T> {
    /// Another handle to the same data, for another window.
    fn share(&self) -> Self {
        Shared(self.0.clone())
    }

    fn same(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

#[allow(unsafe_code)]
impl<T> Deref for Shared<T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: see the type: the running window is the only borrower, and `&self` keeps its
        // field borrowed for as long as the reference lives.
        unsafe { &*self.0.get() }
    }
}

#[allow(unsafe_code)]
impl<T> DerefMut for Shared<T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as for `deref`; `&mut self` makes this the field's only borrow in the window.
        unsafe { &mut *self.0.get() }
    }
}

impl<'a, T> IntoIterator for &'a Shared<T>
where
    &'a T: IntoIterator,
{
    type Item = <&'a T as IntoIterator>::Item;
    type IntoIter = <&'a T as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        (**self).into_iter()
    }
}

impl<'a, T> IntoIterator for &'a mut Shared<T>
where
    &'a mut T: IntoIterator,
{
    type Item = <&'a mut T as IntoIterator>::Item;
    type IntoIter = <&'a mut T as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        (&mut **self).into_iter()
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Shared<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}

/// What the windows of one server have in common besides the data: which stream event the data
/// has taken last, how often a window changed it from a reply only it heard, and how to wake them.
#[derive(Default)]
struct Common {
    applied: Cell<u64>,
    noticed: Cell<u64>,
    sounded: Cell<u64>,
    revision: Cell<u64>,
    wakers: RefCell<Vec<(u64, rune::Waker)>>,
}

/// A window's place in its server's shared data.
#[derive(Default)]
pub struct Link {
    common: Option<Rc<Common>>,
    /// The revision this window last drew.
    seen: u64,
    /// This window among the wakers.
    token: u64,
}

/// One server's shared data.
struct Handles {
    common: Rc<Common>,
    projects: Shared<Vec<Project>>,
    sessions: Shared<Vec<SessionInfo>>,
    transcripts: Shared<HashMap<String, Transcript>>,
    running: Shared<HashSet<String>>,
    requests: Shared<HashSet<(String, String)>>,
    pending: Shared<Vec<Pending>>,
    inboxes: Shared<HashMap<String, Inbox>>,
}

thread_local! {
    /// Each server's shared data, by its base URL.
    static STORES: RefCell<HashMap<String, Handles>> = RefCell::default();
    static TOKENS: Cell<u64> = const { Cell::new(0) };
}

impl Handles {
    fn of(s: &App, common: Rc<Common>) -> Handles {
        Handles {
            common,
            projects: s.projects.share(),
            sessions: s.sessions.share(),
            transcripts: s.transcripts.share(),
            running: s.running.share(),
            requests: s.requests.share(),
            pending: s.pending.share(),
            inboxes: s.inboxes.share(),
        }
    }

    /// More handles to the same data.
    fn share(&self) -> Handles {
        Handles {
            common: self.common.clone(),
            projects: self.projects.share(),
            sessions: self.sessions.share(),
            transcripts: self.transcripts.share(),
            running: self.running.share(),
            requests: self.requests.share(),
            pending: self.pending.share(),
            inboxes: self.inboxes.share(),
        }
    }

    /// `s` takes these. What it read that they lack goes into them first: sessions and
    /// transcripts it knows that no other window loaded, its projects while none are listed.
    fn give(&self, s: &mut App) {
        let mut to = self.share();
        if to.projects.is_empty() {
            *to.projects = std::mem::take(&mut *s.projects);
        }
        for info in s.sessions.drain(..) {
            if !to.sessions.iter().any(|x| x.id == info.id) {
                to.sessions.push(info);
            }
        }
        for (id, transcript) in s.transcripts.drain() {
            to.transcripts.entry(id).or_insert(transcript);
        }
        to.running.extend(s.running.drain());
        to.requests.extend(s.requests.drain());
        for p in s.pending.drain(..) {
            if !to.pending.iter().any(|x| x.id() == p.id()) {
                to.pending.push(p);
            }
        }
        for (id, inbox) in s.inboxes.drain() {
            to.inboxes.entry(id).or_insert(inbox);
        }
        let Handles { projects, sessions, transcripts, running, requests, pending, inboxes, .. } = to;
        (s.projects, s.sessions, s.transcripts, s.running, s.requests, s.pending, s.inboxes) = (projects, sessions, transcripts, running, requests, pending, inboxes);
    }
}
/// The window, connected to its server, joins that server's shared data, or starts it.
pub fn join(s: &mut App) {
    let Source::Live { server, .. } = &s.source else { return };
    let base = server.base.clone();
    STORES.with(|stores| {
        let mut stores = stores.borrow_mut();
        match stores.get(&base) {
            Some(h) if h.transcripts.same(&s.transcripts) => {}
            Some(h) => {
                h.give(s);
                s.store.common = Some(h.common.clone());
            }
            None => {
                let common = Rc::new(Common::default());
                stores.insert(base, Handles::of(s, common.clone()));
                s.store.common = Some(common);
            }
        }
    });
    let Some(common) = s.store.common.clone() else { return };
    s.store.seen = common.revision.get();
    if s.store.token == 0 {
        s.store.token = TOKENS.with(|t| {
            t.set(t.get() + 1);
            t.get()
        });
    }
    let token = s.store.token;
    let mut wakers = common.wakers.borrow_mut();
    wakers.retain(|(t, _)| *t != token);
    if let Some(waker) = s.waker.clone() {
        wakers.push((token, waker));
    }
}

/// The window leaves the shared data (it closes, or moves to another server, or loses it): it
/// keeps nothing of it, and the other windows keep it all.
pub fn leave(s: &mut App) {
    let Some(common) = s.store.common.take() else { return };
    let token = s.store.token;
    common.wakers.borrow_mut().retain(|(t, _)| *t != token);
    s.projects = Shared::default();
    s.sessions = Shared::default();
    s.transcripts = Shared::default();
    s.running = Shared::default();
    s.requests = Shared::default();
    s.pending = Shared::default();
    s.inboxes = Shared::default();
}

/// The shared data a window is in, for a window it opens (a torn-out tab's) to start in.
pub struct Share(Handles);

pub fn share(s: &App) -> Option<Share> {
    let common = s.store.common.clone()?;
    Some(Share(Handles::of(s, common)))
}

/// A torn-out tab's window starts in its opener's shared data, so it shows the tab at once.
pub fn adopt(s: &mut App, share: Share) {
    let Handles { common, projects, sessions, transcripts, running, requests, pending, inboxes } = share.0;
    (s.projects, s.sessions, s.transcripts, s.running, s.requests, s.pending, s.inboxes) = (projects, sessions, transcripts, running, requests, pending, inboxes);
    s.store.common = Some(common);
}
/// Whether the shared data hasn't taken stream event `data` yet: the first window to see it
/// applies it, the others only react. Events from elsewhere (replies, scripts) always apply.
pub fn first_sight(s: &App, data: &serde_json::Value) -> bool {
    take(s, data, |c| &c.applied)
}

/// Whether no window posted a notification for stream event `data` yet: each window that has the
/// session's tab decides, and the first one posts.
pub fn first_notice(s: &App, data: &serde_json::Value) -> bool {
    take(s, data, |c| &c.noticed)
}

/// Whether no window played a sound for stream event `data` yet, as `first_notice`.
pub fn first_sound(s: &App, data: &serde_json::Value) -> bool {
    take(s, data, |c| &c.sounded)
}

fn take(s: &App, data: &serde_json::Value, mark: impl Fn(&Common) -> &Cell<u64>) -> bool {
    let (Some(common), Some(seq)) = (s.store.common.as_ref(), data[opencode_kit::client::HUB_SEQ].as_u64()) else { return true };
    let mark = mark(common);
    if seq <= mark.get() {
        return false;
    }
    mark.set(seq);
    true
}

/// `poll`: another window changed the shared data since this one last drew it.
pub fn behind(s: &mut App) -> bool {
    let Some(common) = s.store.common.as_ref() else { return false };
    let revision = common.revision.get();
    std::mem::replace(&mut s.store.seen, revision) != revision
}

/// This window changed the shared data from what only it heard: the others draw it.
pub fn changed(s: &mut App) {
    let Some(common) = s.store.common.as_ref() else { return };
    let revision = common.revision.get() + 1;
    common.revision.set(revision);
    s.store.seen = revision;
    let token = s.store.token;
    for (_, wake) in common.wakers.borrow().iter().filter(|(t, _)| *t != token) {
        wake();
    }
}

//! `widget.storage` for the widgets feature (`opencode-feature-widgets`, `storage`): each widget
//! template's storage is its server's space `widget.<name>` (`specs/v2/widgets-live.md`,
//! Storage), followed with `opencode_kit`'s generic [`Space`] over the server's own stream, as
//! pins follow theirs (`shell/tabs/pin_sync.rs`). A space is followed from the first storage
//! widget of its template that runs on the server, and kept for as long as the server's stream
//! lives: its copy stays for the next run, and its pending writes go out once the server is back.
//! Every window of the app shares it; each tells its own runs what moved.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use opencode_feature_widgets::StorageCopy;
use opencode_kit::client::Space;

use crate::App;
use crate::windows::peer::ServerPeer;

/// One followed space.
struct Followed {
    peer: Weak<ServerPeer>,
    space: Space,
    /// Bumped whenever its copy may have moved (never 0, so a first ask always gets it).
    moved: u64,
    /// A whole sync came in since it was opened: a missing key is missing on the server.
    synced: bool,
}

thread_local! {
    /// By server key and widget template name.
    static SPACES: RefCell<HashMap<(String, String), Followed>> = RefCell::default();
}

/// The space of widget template `name`.
fn space_name(name: &str) -> String {
    format!("widget.{name}")
}

/// The server `session` is on, as its key names it.
pub(crate) fn server_of(s: &App, session: &str) -> String {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(key) = crate::servers::runtime::server_for_session(s, session) {
        return key.as_str().to_owned();
    }
    let _ = session;
    crate::servers::runtime::active(s).as_str().to_owned()
}

/// Follows `name`'s space on `server` over the server's current stream: opened on the first ask,
/// opened again on a new stream (the old one's peer went).
fn follow<R>(server: &str, name: &str, f: impl FnOnce(&mut Followed) -> R) -> Option<R> {
    let listed = crate::servers::runtime::listed().into_iter().find(|l| l.key.as_str() == server && !l.signed_out)?;
    let peer = crate::servers::runtime::peer(&listed.key)?;
    SPACES.with_borrow_mut(|spaces| {
        let key = (server.to_owned(), name.to_owned());
        let current = spaces.get(&key).is_some_and(|f| f.peer.upgrade().is_some_and(|p| Rc::ptr_eq(&p, &peer)));
        if !current {
            let moved = spaces.remove(&key).map_or(0, |old| {
                old.space.close();
                old.moved
            });
            let space = peer.space(listed.server.clone(), &space_name(name));
            spaces.insert(key.clone(), Followed { peer: Rc::downgrade(&peer), space, moved: moved + 1, synced: false });
        }
        spaces.get_mut(&key).map(f)
    })
}

/// [`opencode_feature_widgets::WidgetsHost::storage_copy`].
pub(crate) fn copy(s: &App, server: &str, name: &str, seen: u64) -> Option<StorageCopy> {
    // A fixture run or story has no server: its storage is empty and whole, and keeps nothing.
    if !s.live() {
        return (seen != 1).then(|| StorageCopy { moved: 1, complete: true, entries: Default::default() });
    }
    follow(server, name, |f| {
        take(f);
        (f.moved != seen).then(|| StorageCopy { moved: f.moved, complete: f.synced, entries: f.space.entries().cloned().unwrap_or_default() })
    })
    .flatten()
}

/// [`opencode_feature_widgets::WidgetsHost::storage_write`].
pub(crate) fn write(s: &App, server: &str, name: &str, key: String, value: Option<String>) {
    if !s.live() {
        return;
    }
    follow(server, name, |f| {
        f.space.write(vec![(key, value)]);
        f.moved += 1;
    });
}

/// What came in for `f`'s space.
fn take(f: &mut Followed) {
    if f.space.poll() {
        f.moved += 1;
    }
    if !f.synced && f.space.complete() {
        f.synced = true;
        f.moved += 1;
    }
}

/// Each frame: every followed space takes what came in, so nothing piles up while none of its
/// widgets runs; one whose server's stream went is let go.
pub(crate) fn poll() -> bool {
    SPACES.with_borrow_mut(|spaces| {
        spaces.retain(|_, f| f.peer.strong_count() > 0);
        spaces.values_mut().for_each(take);
    });
    false
}

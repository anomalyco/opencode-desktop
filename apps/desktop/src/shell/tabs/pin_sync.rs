//! Experimental › Sync pinned sessions (Rune's own): which sessions are pinned follows each
//! server's `pins` space (key: the session's ID, value `"1"`), which the other apps on that server
//! share. Pin order, groups and their colors stay this device's. The first sync of a server since
//! the switch went on merges: this device's pins the space lacks go into it. From then on the
//! space decides: a pin it drops is unpinned here (leaving its group), one it adds is pinned at
//! the end. Off, nothing here runs and pins stay this device's alone.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::{Rc, Weak};

use opencode_kit::client::Space;
use opencode_kit::servers::ServerKey;
use serde_json::Value;

use super::pins::{self, PinRef};
use crate::App;
use crate::windows::peer::ServerPeer;

const SPACE: &str = "pins";
const PINNED: &str = "1";
const GLOBAL: &str = "opencode.global.dat";
/// The servers whose pins went into their space since the switch went on.
const MERGED: &str = "navigation.pins.merged";

/// Whether a server's pins went into its space yet.
enum Phase {
    Merging,
    Following,
}

/// One server's space, for as long as its stream lives.
struct Link {
    key: ServerKey,
    peer: Weak<ServerPeer>,
    space: Space,
    phase: Phase,
}

enum Sync {
    Off,
    On(Vec<Link>),
}

thread_local! {
    static SYNC: RefCell<Sync> = const { RefCell::new(Sync::Off) };
}

/// The pins before a change of the window's own, while pins sync ([`changed`] takes them).
pub fn before(s: &App) -> Option<Vec<PinRef>> {
    let on = s.live() && SYNC.with_borrow(|sync| matches!(sync, Sync::On(_)));
    on.then(|| pins::state(s).doc.refs().cloned().collect())
}

/// The window pinned or unpinned sessions itself: their servers' spaces hear it.
pub fn changed(s: &App, before: Vec<PinRef>) {
    let after: Vec<&PinRef> = pins::state(s).doc.refs().collect();
    SYNC.with_borrow_mut(|sync| {
        let Sync::On(links) = sync else { return };
        for link in links {
            let pinned = after.iter().filter(|r| r.server == link.key && !before.contains(r)).map(|r| (r.session_id.clone(), Some(PINNED.to_owned())));
            let unpinned = before.iter().filter(|r| r.server == link.key && !after.contains(r)).map(|r| (r.session_id.clone(), None));
            link.space.write(pinned.chain(unpinned).collect());
        }
    });
}

/// Each frame: follows the listed servers' spaces while the switch is on, and takes what they
/// say into the window's pins. True when its pins changed.
pub fn poll(s: &mut App) -> bool {
    if !s.live() {
        return false;
    }
    let on = s.settings.pin_sync();
    let follow = SYNC.with_borrow_mut(|sync| match (std::mem::replace(sync, Sync::Off), on) {
        (Sync::Off, false) => false,
        (Sync::On(links), false) => {
            links.into_iter().for_each(|l| l.space.close());
            crate::storage::delete(GLOBAL, MERGED);
            false
        }
        (Sync::Off, true) => {
            *sync = Sync::On(Vec::new());
            true
        }
        (Sync::On(links), true) => {
            *sync = Sync::On(links);
            true
        }
    });
    if !follow {
        return false;
    }
    refit();
    let merges: Vec<ServerKey> = SYNC.with_borrow_mut(|sync| {
        let Sync::On(links) = sync else { return Vec::new() };
        let mut merges = Vec::new();
        for link in links {
            link.space.poll();
            if link.space.complete() && matches!(link.phase, Phase::Merging) {
                merges.push(link.key.clone());
            }
        }
        merges
    });
    for key in merges {
        merge(s, &key);
    }
    let (gone, added) = SYNC.with_borrow(|sync| {
        let Sync::On(links) = sync else { return (Vec::new(), Vec::new()) };
        let doc = &pins::state(s).doc;
        let (mut gone, mut added) = (Vec::new(), Vec::new());
        for link in links {
            let Some(entries) = link.space.entries() else { continue };
            // A pin the space lacks goes only once a whole sync says so, after the merge.
            if matches!(link.phase, Phase::Following) && link.space.complete() {
                gone.extend(doc.refs().filter(|r| r.server == link.key && !entries.contains_key(&r.session_id)).cloned());
            }
            let here = |id: &&String| doc.refs().any(|r| r.server == link.key && r.session_id == **id);
            added.extend(entries.keys().filter(|id| !here(id)).map(|id| PinRef::new(link.key.clone(), id.as_str())));
        }
        (gone, added)
    });
    if gone.is_empty() && added.is_empty() {
        return false;
    }
    pins::follow(s, |d| {
        for r in &gone {
            d.unpin(r);
        }
        for r in added {
            d.pin(r);
        }
        true
    });
    true
}

/// Keeps one link per listed server with a stream, on its current stream.
fn refit() {
    let listed: Vec<_> = crate::servers::runtime::listed().into_iter().filter(|l| !l.signed_out).collect();
    SYNC.with_borrow_mut(|sync| {
        let Sync::On(links) = sync else { return };
        let mut kept = Vec::with_capacity(listed.len());
        for l in &listed {
            let Some(peer) = crate::servers::runtime::peer(&l.key) else { continue };
            let same = |link: &Link| link.key == l.key && link.peer.upgrade().is_some_and(|p| Rc::ptr_eq(&p, &peer));
            match links.iter().position(same) {
                Some(at) => kept.push(links.swap_remove(at)),
                None => kept.push(Link { key: l.key.clone(), peer: Rc::downgrade(&peer), space: peer.space(l.server.clone(), SPACE), phase: Phase::Merging }),
            }
        }
        std::mem::take(links).into_iter().for_each(|l| l.space.close());
        *links = kept;
    });
}

/// A server's first sync since the switch went on: the window's pins of it that the space lacks
/// go into it, once.
fn merge(s: &App, key: &ServerKey) {
    let mut merged: BTreeSet<String> = crate::storage::json(GLOBAL, MERGED)
        .and_then(|v| v.as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()))
        .unwrap_or_default();
    let first = merged.insert(key.as_str().to_owned());
    let local: Vec<String> = pins::state(s).doc.refs().filter(|r| r.server == *key).map(|r| r.session_id.clone()).collect();
    SYNC.with_borrow_mut(|sync| {
        let Sync::On(links) = sync else { return };
        let Some(link) = links.iter_mut().find(|l| l.key == *key) else { return };
        if first {
            let missing = local.iter().filter(|id| link.space.entries().is_some_and(|e| !e.contains_key(*id)));
            let changes = missing.map(|id| (id.clone(), Some(PINNED.to_owned()))).collect();
            link.space.write(changes);
        }
        link.phase = Phase::Following;
    });
    if first && s.persists() {
        crate::storage::set_json(GLOBAL, MERGED, &Value::from(merged.into_iter().collect::<Vec<_>>()));
    }
}

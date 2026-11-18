//! Each session's transcript as it last settled on screen, kept in the app's database: a
//! discardable cache row per session (`rune.copy`), the [`KEEP`] most recently shown. A tab opened
//! without its transcript in memory asks for its copy; it arrives from the storage thread, decoded
//! there, a millisecond or two later, so a session read before stays readable while its server
//! can't be reached (a restart offline included), and one opened online paints before its page.
//! The server's page then merges into it as it would into a preview (`restore::preview`).
//!
//! Desktop keeps no copy: a tab whose session its server can't give shows the unavailable panel.
//! Written when the shown session settles (its stamp changed, nothing streaming), never per token;
//! only its last [`MESSAGES`] messages, at most [`BYTES`].

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use opencode_kit::model::{Message, Transcript, preview};
use opencode_kit::servers::ServerKey;
use rune::Tx;

use crate::{RestoreHost, RestoreRoute};

/// How many sessions keep a copy (open tabs and the recently shown).
const KEEP: usize = 30;
/// How much of a session a copy keeps: what a reader scrolls back through offline.
const MESSAGES: usize = 200;
const BYTES: usize = 2 << 20;
/// The copies, most recently written first (`[server, session]` pairs).
const INDEX: (&str, &str) = ("rune.copies", "index");

type ArrivedCopy = (String, String, Vec<Message>);

#[derive(Default)]
pub struct Copies {
    /// What each session's copy was last written (or read) from.
    written: HashMap<String, u64>,
    /// Reads the storage thread answered: the server, the session and its decoded messages (none
    /// when it had no copy), for the window's next poll.
    arrived: Arc<Mutex<Vec<ArrivedCopy>>>,
    /// Sessions asked for: answered or not, each is asked once a run (one with no copy gets one
    /// only by being shown, which fills its transcript anyway).
    asked: HashSet<String>,
}

fn key(server: &str, session: &str) -> String {
    format!("rune.copy\n{server}\n{session}")
}

/// A copy's bytes: the last [`MESSAGES`] messages, fewer while they're over [`BYTES`].
pub fn encode(messages: &[Message]) -> Vec<u8> {
    let mut start = messages.len().saturating_sub(MESSAGES);
    loop {
        let bytes = preview::encode(&messages[start..]);
        if bytes.len() <= BYTES || start + 1 >= messages.len() {
            return bytes;
        }
        start += (messages.len() - start) / 4 + 1;
    }
}

/// The tab of `id` on `server` opens without its transcript in memory: its copy is asked for.
pub fn ask_copy(host: &mut dyn RestoreHost, server: &ServerKey, id: &str) {
    if !host.persists() || host.has_transcript(id) || host.restore().copies.asked.contains(id) {
        return;
    }
    let arrived = {
        let copies = &mut host.restore_mut().copies;
        if !copies.asked.insert(id.to_owned()) {
            return;
        }
        copies.arrived.clone()
    };
    let wake = host.waker();
    let (server, id) = (server.as_str().to_owned(), id.to_owned());
    host.with_storage(Box::new(move |s| {
        s.caches().get(
            &key(&server, &id),
            i64::from(preview::VERSION),
            Box::new(move |bytes| {
                let messages = bytes.and_then(|b| preview::decode(&b)).unwrap_or_default();
                arrived.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((server, id, messages));
                if let Some(w) = wake {
                    w();
                }
            }),
        );
    }));
}

/// The shown session without its transcript asks for its copy; copies that arrived go into
/// transcripts nothing filled meanwhile, and an opening tab waiting on its page lands on its
/// copy. True when the screen changed.
pub fn poll_copies(host: &mut dyn RestoreHost, tx: &mut Tx) -> bool {
    if let RestoreRoute::Session { server, id } = host.route()
        && server == host.active_server()
        && !host.has_transcript(&id)
    {
        ask_copy(host, &server, &id);
    }
    let arrived = std::mem::take(&mut *host.restore().copies.arrived.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    let mut changed = false;
    for (server, id, messages) in arrived {
        if messages.is_empty() || host.active_server().as_str() != server || host.has_transcript(&id) {
            continue;
        }
        rune::marks::mark("state.copy");
        host.restore_mut().copies.written.insert(id.clone(), crate::preview::stamp(&messages));
        host.insert_transcript(id.clone(), Transcript { session: id.clone(), messages });
        host.on_copy_received(&id);
        host.settle_timeline_jump(tx);
        changed = true;
    }
    changed
}

/// Whether session `id`'s copy was written from anything but what `stamp` names.
pub(crate) fn copy_due(host: &dyn RestoreHost, id: &str, stamp: u64) -> bool {
    host.restore().copies.written.get(id) != Some(&stamp)
}

/// Writes session `id`'s copy on `server` (`bytes`, from [`encode`]) as of `stamp`.
pub(crate) fn store_copy(host: &mut dyn RestoreHost, server: &ServerKey, id: &str, stamp: u64, bytes: Vec<u8>) {
    host.restore_mut().copies.written.insert(id.to_owned(), stamp);
    let (server, id) = (server.as_str().to_owned(), id.to_owned());
    let data: Arc<[u8]> = bytes.into();
    // The index's namespace is read lazily: it's updated on the storage thread once it's in.
    host.with_storage(Box::new(move |s| {
        s.caches().put(&key(&server, &id), i64::from(preview::VERSION), data);
        let store = s.clone();
        s.state().load(
            &[INDEX.0],
            Some(Box::new(move || {
                let mut index: Vec<(String, String)> = store.state().json(INDEX.0, INDEX.1).unwrap_or_default();
                index.retain(|(s, i)| !(*s == server && *i == id));
                index.insert(0, (server, id));
                let evicted = index.split_off(index.len().min(KEEP));
                store.state().set_json(INDEX.0, INDEX.1, &index);
                for (s, i) in evicted {
                    store.caches().discard(&key(&s, &i));
                }
            })),
        );
    }));
}

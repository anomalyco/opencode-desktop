//! One space of a server (`/api/experimental/space`, `packages/protocol/src/groups/space.ts`):
//! a client-named map of string keys to string values the server keeps and sends to every
//! client that follows it. A [`Space`] syncs it page by page, each page applied as it lands (a
//! snapshot's missing keys go once its last page is in), subscribes the server's event
//! stream (its [`EventHub`], no second connection) from the cursor the sync saved, and applies
//! the stream's changes; a new stream (a reconnect) or `experimental.space.resync` syncs again
//! from that cursor. A sync or subscribe that fails waits for the next stream.
//!
//! A set or delete shows at once and stays pending until the server confirms it: a sync or a
//! stream event never undoes a pending write, and a write made without a stream, or one the
//! server didn't get (it was down), goes out with the next stream, before that stream's sync.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{Receiver, Sender, channel};

use rune::Waker;
use rune::machine::{Attempt, Effects, Seq};
use serde_json::{Value, json};

use super::{EventHub, Incoming, Server};
use crate::wire_error::WireError;

/// What the last sync left: every entry, the cursor it covers, and the latest revision the space
/// heard since (the sync's, then the stream's).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Synced {
    pub entries: BTreeMap<String, String>,
    pub cursor: String,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Waiting for the stream to name itself (`server.connected`).
    Waiting { copy: Option<Synced> },
    /// Applying pages to `entries` as they land. `cursor` and `revision` are the last completed
    /// position, where a new sync starts (`None` before the first); `asked` is the cursor the page on its way was asked from; `snapshot` holds the
    /// keys a snapshot's pages held so far, while one runs.
    Syncing {
        attempt: Attempt,
        stream: String,
        entries: BTreeMap<String, String>,
        cursor: Option<String>,
        revision: u64,
        asked: Option<String>,
        snapshot: Option<BTreeSet<String>>,
    },
    /// Synced; subscribing the stream from `copy.cursor`. The changes since come as events, maybe
    /// before the subscribe's answer.
    Subscribing { attempt: Attempt, stream: String, copy: Synced },
    /// Following the stream's changes.
    Live { stream: String, copy: Synced },
}

/// One sync page (`Space.Page`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    pub snapshot: bool,
    pub entries: Vec<(String, String)>,
    pub deleted: Vec<String>,
    pub cursor: String,
    pub revision: u64,
    pub next: Option<String>,
}

/// A local write the server hasn't confirmed: `value` sets the key, `None` deletes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub attempt: Attempt,
    pub value: Option<String>,
}

/// The pending writes, one per key: a later write to a key replaces the earlier one.
pub type Writes = BTreeMap<String, Pending>;

/// A pending write as one change of a write request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub key: String,
    pub attempt: Attempt,
    pub value: Option<String>,
}

/// What one write request may carry (`Space.MaxChanges`, `Space.MaxValueBytes`).
const MAX_CHANGES: usize = 1024;
const MAX_BYTES: usize = 1 << 20;

/// How a write went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Committed: the space's revision and cursor after it, when the answer said.
    Done { head: Option<(u64, String)> },
    /// The server refused it (a 4xx): it won't take it later either.
    Refused,
    /// It may not have arrived (the server was down, or failed): it goes out again.
    Lost,
}

/// What a sync request brought for the space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fetched {
    Page(Page),
    /// The request's row budget ran out before the space: ask again from the same cursor.
    Skipped,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// `server.connected`: a stream opened, with its ID.
    Connected { stream: String },
    /// `experimental.space.resync`.
    Resync,
    /// `experimental.space.updated`.
    Updated { key: String, value: String, revision: u64 },
    /// `experimental.space.deleted`.
    Deleted { key: String, revision: u64 },
    Paged { attempt: Attempt, page: Fetched },
    Subscribed { attempt: Attempt, ok: bool },
    /// The caller sets each key to its value, or deletes it (`None`), in order.
    Write { changes: Vec<(String, Option<String>)> },
    /// The server answered the write request that carried these changes.
    Wrote { carried: Vec<(String, Attempt)>, outcome: Outcome },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// `POST /api/experimental/space/sync` for the space, from `cursor` or a snapshot.
    Fetch { attempt: Attempt, cursor: Option<String> },
    Subscribe { attempt: Attempt, stream: String, cursor: String },
    /// `POST /api/experimental/space/write` with these changes, in order.
    Commit { changes: Vec<Change> },
}

/// `changes` as write requests: one, unless they'd go over a request's count or size.
fn commits(changes: Vec<Change>) -> Effects<Effect> {
    let mut effects = Effects::none();
    let (mut batch, mut bytes) = (Vec::new(), 0);
    for c in changes {
        let size = c.value.as_ref().map_or(0, String::len);
        if !batch.is_empty() && (batch.len() == MAX_CHANGES || bytes + size > MAX_BYTES) {
            effects.push(Effect::Commit { changes: std::mem::take(&mut batch) });
            bytes = 0;
        }
        bytes += size;
        batch.push(c);
    }
    if !batch.is_empty() {
        effects.push(Effect::Commit { changes: batch });
    }
    effects
}

/// Where the next sync starts: what the space holds (pending writes on top), and the stream to
/// subscribe after.
fn sync(seq: &mut Seq, state: &mut State, writes: &Writes, stream: String, copy: Option<Synced>) -> Effects<Effect> {
    let attempt = seq.next();
    let (mut entries, cursor, revision) = copy.map_or((BTreeMap::new(), None, 0), |c| (c.entries, Some(c.cursor), c.revision));
    for (key, w) in writes {
        put(&mut entries, key, &w.value);
    }
    *state = State::Syncing { attempt, stream, entries, cursor: cursor.clone(), revision, asked: cursor.clone(), snapshot: None };
    Effects::one(Effect::Fetch { attempt, cursor })
}

/// What the space holds as of the last completed position, if it has one.
fn completed(entries: BTreeMap<String, String>, cursor: Option<String>, revision: u64) -> Option<Synced> {
    cursor.map(|cursor| Synced { entries, cursor, revision })
}

/// A finished snapshot: the keys none of its pages held go, unless a write is pending on them.
fn drop_unseen(entries: &mut BTreeMap<String, String>, seen: &BTreeSet<String>, writes: &Writes) {
    entries.retain(|key, _| seen.contains(key) || writes.contains_key(key));
}

/// A new stream: the pending writes go out again, then the sync.
fn reconnect(seq: &mut Seq, state: &mut State, writes: &Writes, stream: String, copy: Option<Synced>) -> Effects<Effect> {
    let mut effects = Effects::none();
    effects.extend(commits(writes.iter().map(|(key, w)| Change { key: key.clone(), attempt: w.attempt, value: w.value.clone() }).collect()));
    effects.extend(sync(seq, state, writes, stream, copy));
    effects
}

/// Sets `key` to `value`, or deletes it.
fn put(map: &mut BTreeMap<String, String>, key: &str, value: &Option<String>) {
    match value {
        Some(v) => {
            map.insert(key.to_owned(), v.clone());
        }
        None => {
            map.remove(key);
        }
    }
}

/// What the server says of `key` at `revision` lands unless a write of the space's own is
/// pending on it.
fn heard(copy: &mut Synced, writes: &Writes, key: String, value: Option<String>, revision: u64) {
    copy.revision = copy.revision.max(revision);
    if !writes.contains_key(&key) {
        put(&mut copy.entries, &key, &value);
    }
}

pub fn step(seq: &mut Seq, state: &mut State, writes: &mut Writes, event: Event) -> Effects<Effect> {
    match (std::mem::replace(state, State::Waiting { copy: None }), event) {
        (State::Waiting { copy }, Event::Connected { stream }) => reconnect(seq, state, writes, stream, copy),
        // Mid-snapshot, the next sync starts again from the last completed position.
        (State::Syncing { entries, cursor, revision, .. }, Event::Connected { stream }) => reconnect(seq, state, writes, stream, completed(entries, cursor, revision)),
        (State::Subscribing { copy, .. } | State::Live { copy, .. }, Event::Connected { stream }) => reconnect(seq, state, writes, stream, Some(copy)),
        (State::Subscribing { stream, copy, .. } | State::Live { stream, copy }, Event::Resync) => sync(seq, state, writes, stream, Some(copy)),
        (State::Syncing { attempt, stream, mut entries, mut cursor, mut revision, mut snapshot, .. }, Event::Paged { attempt: a, page: Fetched::Page(page) }) if a == attempt => {
            if page.snapshot {
                snapshot.get_or_insert_with(BTreeSet::new).extend(page.entries.iter().map(|(key, _)| key.clone()));
            } else if let Some(seen) = snapshot.take() {
                // The page before this delta was its snapshot's last.
                drop_unseen(&mut entries, &seen, writes);
            }
            for key in &page.deleted {
                if !writes.contains_key(key) {
                    entries.remove(key);
                }
            }
            for (key, value) in page.entries {
                if !writes.contains_key(&key) {
                    entries.insert(key, value);
                }
            }
            if let Some(next) = page.next {
                // A delta's cursor is a completed position; a snapshot page's isn't.
                if !page.snapshot {
                    (cursor, revision) = (Some(page.cursor), page.revision);
                }
                *state = State::Syncing { attempt, stream, entries, cursor, revision, asked: Some(next.clone()), snapshot };
                return Effects::one(Effect::Fetch { attempt, cursor: Some(next) });
            }
            if let Some(seen) = snapshot {
                drop_unseen(&mut entries, &seen, writes);
            }
            let attempt = seq.next();
            let cursor = page.cursor;
            *state = State::Subscribing { attempt, stream: stream.clone(), copy: Synced { entries, cursor: cursor.clone(), revision: page.revision } };
            Effects::one(Effect::Subscribe { attempt, stream, cursor })
        }
        (State::Syncing { attempt, stream, entries, cursor, revision, asked, snapshot }, Event::Paged { attempt: a, page: Fetched::Skipped }) if a == attempt => {
            *state = State::Syncing { attempt, stream, entries, cursor, revision, asked: asked.clone(), snapshot };
            Effects::one(Effect::Fetch { attempt, cursor: asked })
        }
        (State::Syncing { attempt, entries, cursor, revision, .. }, Event::Paged { attempt: a, page: Fetched::Failed }) if a == attempt => {
            *state = State::Waiting { copy: completed(entries, cursor, revision) };
            Effects::none()
        }
        (State::Subscribing { attempt, stream, copy }, Event::Subscribed { attempt: a, ok }) if a == attempt => {
            // A stream that's gone (`stream_not_found`) or any other refusal: the next stream syncs.
            *state = if ok { State::Live { stream, copy } } else { State::Waiting { copy: Some(copy) } };
            Effects::none()
        }
        (State::Subscribing { attempt, stream, mut copy }, Event::Updated { key, value, revision }) => {
            heard(&mut copy, writes, key, Some(value), revision);
            *state = State::Subscribing { attempt, stream, copy };
            Effects::none()
        }
        (State::Subscribing { attempt, stream, mut copy }, Event::Deleted { key, revision }) => {
            heard(&mut copy, writes, key, None, revision);
            *state = State::Subscribing { attempt, stream, copy };
            Effects::none()
        }
        (State::Live { stream, mut copy }, Event::Updated { key, value, revision }) => {
            heard(&mut copy, writes, key, Some(value), revision);
            *state = State::Live { stream, copy };
            Effects::none()
        }
        (State::Live { stream, mut copy }, Event::Deleted { key, revision }) => {
            heard(&mut copy, writes, key, None, revision);
            *state = State::Live { stream, copy };
            Effects::none()
        }
        // Shown at once, and pending until the server confirms it. Without a stream it waits for
        // the next one (`reconnect`), so a server that's down, or has no spaces, hears nothing.
        (mut prev, Event::Write { changes }) => {
            let mut sent = Vec::with_capacity(changes.len());
            for (key, value) in changes {
                held(&mut prev, |m| put(m, &key, &value));
                let attempt = seq.next();
                writes.insert(key.clone(), Pending { attempt, value: value.clone() });
                sent.push(Change { key, attempt, value });
            }
            let waiting = matches!(prev, State::Waiting { .. });
            *state = prev;
            if waiting {
                return Effects::none();
            }
            commits(sent)
        }
        (mut prev, Event::Wrote { carried, outcome }) => {
            if outcome == Outcome::Lost {
                *state = prev;
                return Effects::none();
            }
            // An answer for a key a later write replaced leaves that write pending.
            for (key, attempt) in &carried {
                if writes.get(key).is_some_and(|w| w.attempt == *attempt) {
                    writes.remove(key);
                }
            }
            if let Outcome::Done { head } = outcome {
                // The answer's cursor covers every change before the write too: it's saved only
                // once the stream delivered them all, else a reconnect's delta could skip some.
                if let (State::Subscribing { copy, .. } | State::Live { copy, .. }, Some((revision, cursor))) = (&mut prev, head)
                    && copy.revision >= revision
                {
                    copy.cursor = cursor;
                }
                *state = prev;
                return Effects::none();
            }
            *state = prev;
            // Refused: a delta from the saved cursor wouldn't undo it, so a fresh snapshot.
            match std::mem::replace(state, State::Waiting { copy: None }) {
                State::Syncing { stream, .. } | State::Subscribing { stream, .. } | State::Live { stream, .. } => sync(seq, state, writes, stream, None),
                State::Waiting { .. } => Effects::none(),
            }
        }
        (prev, _) => {
            *state = prev;
            Effects::none()
        }
    }
}

/// Applies `f` to the map the state holds of the space.
fn held(state: &mut State, mut f: impl FnMut(&mut BTreeMap<String, String>)) {
    match state {
        State::Waiting { copy } => copy.iter_mut().for_each(|c| f(&mut c.entries)),
        State::Syncing { entries, .. } => f(entries),
        State::Subscribing { copy, .. } | State::Live { copy, .. } => f(&mut copy.entries),
    }
}

/// One space of one server, followed over that server's event stream.
pub struct Space {
    server: Server,
    name: String,
    events: Receiver<Incoming>,
    /// The answers to the requests it made, as events.
    landed_tx: Sender<Event>,
    landed: Receiver<Event>,
    wake: Waker,
    seq: Seq,
    state: State,
    writes: Writes,
}

impl Space {
    /// Follows space `name` of `server` over `hub`, its stream; `wake` is called when something
    /// came in for [`Space::poll`].
    pub fn open(server: Server, name: &str, hub: &EventHub, wake: Waker) -> Space {
        let events = hub.subscribe_where(|ty| ty == "server.connected" || ty.starts_with("experimental.space."), wake.clone());
        let (landed_tx, landed) = channel();
        Space { server, name: name.to_owned(), events, landed_tx, landed, wake, seq: Seq::default(), state: State::Waiting { copy: None }, writes: Writes::new() }
    }

    /// Every entry, as the sync's pages land; None before the first sync starts.
    pub fn entries(&self) -> Option<&BTreeMap<String, String>> {
        match &self.state {
            State::Waiting { copy } => copy.as_ref().map(|c| &c.entries),
            State::Syncing { entries, .. } => Some(entries),
            State::Subscribing { copy, .. } | State::Live { copy, .. } => Some(&copy.entries),
        }
    }

    /// Whether the entries are a whole sync's: the last sync reached its last page, and none
    /// runs now. Until then a key may be missing only because its page hasn't landed.
    pub fn complete(&self) -> bool {
        match &self.state {
            State::Waiting { copy } => copy.is_some(),
            State::Syncing { .. } => false,
            State::Subscribing { .. } | State::Live { .. } => true,
        }
    }

    /// Sets each key to its value, or deletes it (`None`), in order, in one write request.
    pub fn write(&mut self, changes: Vec<(String, Option<String>)>) {
        if !changes.is_empty() {
            self.step(Event::Write { changes });
        }
    }

    /// Takes what the stream and the requests brought. True when anything did.
    pub fn poll(&mut self) -> bool {
        let mut heard = false;
        while let Ok(m) = self.events.try_recv() {
            heard = true;
            if let Some(event) = self.event(&m) {
                self.step(event);
            }
        }
        while let Ok(landed) = self.landed.try_recv() {
            heard = true;
            self.step(landed);
        }
        heard
    }

    /// Stops following: the stream stops sending the space's changes.
    pub fn close(self) {
        if let State::Syncing { stream, .. } | State::Subscribing { stream, .. } | State::Live { stream, .. } = &self.state {
            let body = json!({ "stream": stream, "spaces": [self.name] });
            let server = self.server.clone();
            rune::task::spawn("opencode-space", move || {
                server.write("POST", "/api/experimental/space/unsubscribe", Some(body)).ok();
            })
            .ok();
        }
    }

    fn event(&self, m: &Incoming) -> Option<Event> {
        let Incoming::Event { ty, data } = m else { return None };
        let text = |k: &str| data[k].as_str().map(str::to_owned);
        if ty == "server.connected" {
            return Some(Event::Connected { stream: text("stream")? });
        }
        if data["space"].as_str() != Some(self.name.as_str()) {
            return None;
        }
        match ty.as_str() {
            "experimental.space.updated" => Some(Event::Updated { key: text("key")?, value: text("value")?, revision: data["revision"].as_u64()? }),
            "experimental.space.deleted" => Some(Event::Deleted { key: text("key")?, revision: data["revision"].as_u64()? }),
            "experimental.space.resync" => Some(Event::Resync),
            _ => None,
        }
    }

    fn step(&mut self, event: Event) {
        for effect in step(&mut self.seq, &mut self.state, &mut self.writes, event) {
            self.run(effect);
        }
    }

    /// Makes `effect`'s request off the app's thread; its answer comes back to `poll`.
    fn run(&self, effect: Effect) {
        let (server, tx, wake) = (self.server.clone(), self.landed_tx.clone(), self.wake.clone());
        let answer = move |event: Event| {
            if tx.send(event).is_ok() {
                wake();
            }
        };
        let (failed, work): (Event, Box<dyn FnOnce() -> Option<Event> + Send>) = match effect {
            Effect::Fetch { attempt, cursor } => {
                let mut asked = json!({ "space": self.name });
                if let Some(c) = cursor {
                    asked["cursor"] = c.into();
                }
                let name = self.name.clone();
                let fetch = move || match server.write("POST", "/api/experimental/space/sync", Some(json!({ "spaces": [asked] }))) {
                    Ok(v) => v["pages"].as_array().into_iter().flatten().find(|p| p["space"].as_str() == Some(name.as_str())).map_or(Fetched::Skipped, |p| page(p).map_or(Fetched::Failed, Fetched::Page)),
                    Err(_) => Fetched::Failed,
                };
                (Event::Paged { attempt, page: Fetched::Failed }, Box::new(move || Some(Event::Paged { attempt, page: fetch() })))
            }
            Effect::Subscribe { attempt, stream, cursor } => {
                let body = json!({ "stream": stream, "spaces": [{ "space": self.name, "cursor": cursor }] });
                let ok = move || server.write("POST", "/api/experimental/space/subscribe", Some(body)).is_ok();
                (Event::Subscribed { attempt, ok: false }, Box::new(move || Some(Event::Subscribed { attempt, ok: ok() })))
            }
            Effect::Commit { changes } => {
                let carried: Vec<(String, Attempt)> = changes.iter().map(|c| (c.key.clone(), c.attempt)).collect();
                let lost = Event::Wrote { carried: carried.clone(), outcome: Outcome::Lost };
                let body = json!({ "changes": changes.iter().map(|c| json!({ "space": self.name, "key": c.key, "value": c.value })).collect::<Vec<_>>() });
                let name = self.name.clone();
                (lost, Box::new(move || Some(Event::Wrote { carried, outcome: outcome(&name, server.write("POST", "/api/experimental/space/write", Some(body))) })))
            }
        };
        let back = answer.clone();
        if rune::task::spawn("opencode-space", move || work().into_iter().for_each(back)).is_err() {
            answer(failed);
        }
    }
}

fn outcome(name: &str, result: Result<Value, WireError>) -> Outcome {
    match result {
        Ok(v) => {
            let head = v["spaces"].as_array().into_iter().flatten().find(|h| h["space"].as_str() == Some(name));
            Outcome::Done { head: head.and_then(|h| h["revision"].as_u64().zip(h["cursor"].as_str().map(str::to_owned))) }
        }
        // A timeout, conflict or rate limit may take it later.
        Err(e) if e.status().is_some_and(|s| (400..500).contains(&s) && ![408, 409, 425, 429].contains(&s)) => Outcome::Refused,
        Err(_) => Outcome::Lost,
    }
}

fn page(v: &Value) -> Option<Page> {
    let text = |v: &Value| v.as_str().map(str::to_owned);
    let snapshot = match v["type"].as_str() {
        Some("snapshot") => true,
        Some("delta") => false,
        _ => return None,
    };
    let entries = v["entries"].as_array()?.iter().filter_map(|e| Some((text(&e["key"])?, text(&e["value"])?))).collect();
    let deleted = v["deleted"].as_array().into_iter().flatten().filter_map(text).collect();
    let cursor = text(&v["cursor"])?;
    let revision = v["revision"].as_u64()?;
    Some(Page { snapshot, entries, deleted, cursor, revision, next: text(&v["next"]) })
}

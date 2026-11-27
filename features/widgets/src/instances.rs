//! The server's widgets across its sessions (RPC `opencode.widget` `instances`): what shows a
//! widget outside its session's timeline (the Mini apps dashboard and list, a widget's own
//! window) reads them here, one store per window, so every such view shows the same widget.
//!
//! - **Keys:** a widget is named across the server by [`instance_key`]
//!   (`sessionID:messageID:callID`); [`resolve`](Instances::resolve) takes any key, an old one
//!   too, to the widget showing now: an agent updates a widget by posting a new call that
//!   `replaces` it, and the old one is superseded (`supersededBy`), so a key kept from before
//!   follows to the newest (the instances' `replaced` lists, and `changed` events since).
//! - **Kept current:** read when something first [`want`]s them, again on reconnect, on
//!   `plugin.updated` while the plugin wasn't loaded (`rpc.unavailable`), and when a `changed`
//!   event names a widget it doesn't know ([`changed`]); a known one's state changes in place.
//! - **Following:** [`Instances::revision`] moves on every change: a view keeps the revision it
//!   last read and reads again when it moved.
//! - **Running one:** [`open`] resolves a key and opens its widget in this window (its program,
//!   machine and view state, as the timeline's); [`crate::instance_view`] draws it at a width.

use std::collections::HashMap;

use opencode_kit::model::{WidgetInstance, WidgetState, WidgetStatus, instance_key};
use serde_json::Value;

use crate::WidgetsHost;

/// How many updates back a key is followed.
const HOPS: usize = 32;
/// How long after `rpc.unavailable` (the plugin loading in the location) `instances` is read
/// again, and how many times before it waits for `plugin.updated`.
pub const RETRY_S: f64 = 1.5;
const TRIES: u8 = 5;

/// Why `instances` didn't answer.
#[derive(Clone, Debug, PartialEq)]
pub enum InstancesError {
    /// `rpc.unavailable`: the plugin isn't loaded in the location yet.
    Unavailable,
    Failed(String),
}

/// The `instances` read, as an explicit machine.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Fetch {
    /// Nothing asked for them yet.
    #[default]
    Idle,
    /// On its way; `again`: something asked for a fresher read meanwhile; `tries`: the reads
    /// the plugin wasn't loaded for, in a row.
    Fetching { again: bool, tries: u8 },
    /// Read, and followed since.
    Ready,
    /// The plugin wasn't loaded after [`TRIES`] reads: read again on `plugin.updated`.
    Unavailable,
    /// The read failed: the list keeps what it had; read again on the next ask.
    Failed(String),
}

/// What happens to the read.
pub enum FetchEvent {
    /// A view wants them (opening, or a key it can't resolve): read unless read or reading.
    Want,
    /// They may have moved unseen (reconnected, an unknown widget changed): read again.
    Refresh,
    /// The plugin loaded somewhere.
    PluginsChanged,
    /// The answer.
    Answered(Result<(), InstancesError>),
}

/// What the read asks of the host.
#[derive(Debug, PartialEq)]
pub enum FetchEffect {
    /// Read `instances` after `delay` seconds.
    Read { delay: f64 },
}

/// The read's transition.
pub fn step(f: &mut Fetch, ev: FetchEvent) -> Option<FetchEffect> {
    let read = Some(FetchEffect::Read { delay: 0.0 });
    match (&*f, ev) {
        (Fetch::Fetching { tries, .. }, FetchEvent::Refresh) => {
            *f = Fetch::Fetching { again: true, tries: *tries };
            None
        }
        (&Fetch::Fetching { again, tries }, FetchEvent::Answered(result)) => {
            match result {
                Ok(()) if again => {
                    *f = Fetch::Fetching { again: false, tries: 0 };
                    read
                }
                Ok(()) => {
                    *f = Fetch::Ready;
                    None
                }
                Err(InstancesError::Unavailable) if tries + 1 >= TRIES => {
                    *f = Fetch::Unavailable;
                    None
                }
                Err(InstancesError::Unavailable) => {
                    *f = Fetch::Fetching { again: false, tries: tries + 1 };
                    Some(FetchEffect::Read { delay: RETRY_S })
                }
                Err(InstancesError::Failed(m)) => {
                    *f = Fetch::Failed(m);
                    None
                }
            }
        }
        (Fetch::Fetching { .. }, _) => None,
        (Fetch::Ready, FetchEvent::Want | FetchEvent::PluginsChanged) => None,
        (Fetch::Idle, FetchEvent::Refresh | FetchEvent::PluginsChanged) => None,
        (_, FetchEvent::Answered(_)) => None,
        (Fetch::Idle | Fetch::Ready | Fetch::Unavailable | Fetch::Failed(_), _) => {
            *f = Fetch::Fetching { again: false, tries: 0 };
            read
        }
    }
}

/// The server's widgets as this window last read and followed them.
#[derive(Default)]
pub struct Instances {
    pub fetch: Fetch,
    /// Last updated first.
    list: Vec<WidgetInstance>,
    /// [`instance_key`] → its place in `list`.
    index: HashMap<String, usize>,
    /// A replaced widget's key → the key of the one that replaced it.
    replaced: HashMap<String, String>,
    revision: u64,
}

impl Instances {
    /// Every widget of the server but the superseded, last updated first.
    pub fn list(&self) -> &[WidgetInstance] {
        &self.list
    }

    /// The widgets one can still use, last updated first: open, with no failure the server
    /// recorded. A submitted or dismissed widget is closed (it shows only how it ended), so it isn't
    /// one. A replaced widget is already its newest replacement, so each replace chain counts once,
    /// and only when its newest is open and hasn't failed. What lists widgets to pick from reads
    /// these; what shows a widget already picked (a dashboard tile) shows it whatever it is.
    pub fn usable(&self) -> impl Iterator<Item = &WidgetInstance> {
        self.list.iter().filter(|i| i.state.status == WidgetStatus::Open && i.state.failure.is_none())
    }

    /// Moves on every change: a view reads again when it moved since it last read.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether a first read came back.
    pub fn read(&self) -> bool {
        self.revision > 0
    }

    /// The key of the widget `key` names now: itself, or the newest that replaced it.
    pub fn current(&self, key: &str) -> Option<String> {
        let mut key = key.to_owned();
        for _ in 0..HOPS {
            if self.index.contains_key(&key) {
                return Some(key);
            }
            key = self.replaced.get(&key)?.clone();
        }
        None
    }

    /// The widget `key` names now ([`Instances::current`]).
    pub fn resolve(&self, key: &str) -> Option<&WidgetInstance> {
        self.current(key).and_then(|k| self.index.get(&k)).map(|&i| &self.list[i])
    }

    /// The location (directory) session `session`'s widgets are in, as listed.
    pub fn directory(&self, session: &str) -> Option<&str> {
        self.list.iter().find(|i| i.state.session == session).map(|i| i.directory.as_str())
    }

    fn reindex(&mut self) {
        self.list.sort_by_key(|i| std::cmp::Reverse(i.updated));
        self.index = self.list.iter().enumerate().map(|(n, i)| (i.state.instance_key(), n)).collect();
        self.revision += 1;
    }
}

/// Asks for the server's widgets: read once, then followed.
pub fn want(host: &mut dyn WidgetsHost) {
    drive(host, FetchEvent::Want);
}

/// Reads them again (reconnected, or something can't be resolved).
pub fn refresh(host: &mut dyn WidgetsHost) {
    drive(host, FetchEvent::Refresh);
}

/// The plugin loaded somewhere: read again if it wasn't loaded for the last read.
pub(crate) fn plugins_changed(host: &mut dyn WidgetsHost) {
    drive(host, FetchEvent::PluginsChanged);
}

fn drive(host: &mut dyn WidgetsHost, ev: FetchEvent) {
    if let Some(FetchEffect::Read { delay }) = step(&mut host.widgets_mut().instances.fetch, ev) {
        host.fetch_instances(delay);
    }
}

/// `instances` came back (the host's answer to [`WidgetsHost::fetch_instances`]).
pub fn loaded(host: &mut dyn WidgetsHost, result: Result<Vec<WidgetInstance>, InstancesError>) {
    let answer = result.as_ref().map(|_| ()).map_err(Clone::clone);
    if let Ok(list) = result {
        let s = &mut host.widgets_mut().instances;
        for i in &list {
            for old in &i.replaced {
                s.replaced.insert(old.clone(), i.state.instance_key());
            }
        }
        s.list = list;
        s.reindex();
        // The widgets this window runs follow their statuses.
        let states: Vec<WidgetState> = host.widgets().instances.list.iter().map(|i| i.state.clone()).collect();
        for state in &states {
            crate::follow(host, state);
        }
    }
    drive(host, FetchEvent::Answered(answer));
}

/// The plugin's `changed` event (`{ sessionID, state }`): a known widget's state changes in
/// place; one superseded leaves the list and its key follows to the one that replaced it; one
/// unknown (posted, or replacing another) has the list read again. Whether anything changed.
pub(crate) fn changed(host: &mut dyn WidgetsHost, state: &WidgetState, data: &Value) -> bool {
    let s = &mut host.widgets_mut().instances;
    if matches!(s.fetch, Fetch::Idle) {
        return false;
    }
    let key = state.instance_key();
    if state.status == WidgetStatus::Superseded {
        let Some((message, call)) = &state.superseded_by else { return false };
        let next = instance_key(&state.session, message, call);
        s.replaced.insert(key.clone(), next.clone());
        if let Some(&n) = s.index.get(&key) {
            s.list.remove(n);
        }
        let known = s.index.contains_key(&next);
        s.reindex();
        if !known {
            refresh(host);
        }
        return true;
    }
    match s.index.get(&key) {
        Some(&n) => {
            let i = &mut s.list[n];
            i.state = state.clone();
            let at = data["state"]["time"]["responded"].as_i64().unwrap_or(state.created);
            i.updated = i.updated.max(at);
            s.reindex();
        }
        None => refresh(host),
    }
    true
}

/// Opens the widget `key` names now in this window, to show it outside its timeline: its key
/// as this window's widgets know it ([`opencode_kit::model::widget_key`]), or none while it isn't
/// listed (the list is asked for).
pub fn open(host: &mut dyn WidgetsHost, key: &str) -> Option<String> {
    let Some(instance) = host.widgets().instances.resolve(key).cloned() else {
        want(host);
        return None;
    };
    crate::open(host, &instance);
    Some(instance.state.key())
}

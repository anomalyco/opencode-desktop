//! What Luau server sources list (`ServerSource:SetEntries`), shared by every window, as the
//! records the app's server lists already read (`plugin_host::servers`).

use std::cell::RefCell;

use opencode_plugin_spec::{ServerEntry, ServerState};

use crate::plugin_host::instance::ServerRec;

struct Listed {
    plugin: String,
    source: String,
    entries: Vec<ServerRec>,
}

thread_local! {
    static LISTED: RefCell<Vec<Listed>> = const { RefCell::new(Vec::new()) };
}

pub fn set_entries(plugin: &str, source: &str, entries: Vec<ServerEntry>, _ready: bool) {
    let recs = entries
        .into_iter()
        .map(|e| ServerRec {
            source: source.to_owned(),
            id: e.id,
            name: e.name,
            label: e.label.unwrap_or_default(),
            state: match e.state {
                ServerState::Stopped => 0,
                ServerState::Starting => 1,
                ServerState::Auth => 2,
                ServerState::Ready => 3,
                ServerState::Failed => 4,
                ServerState::Incompatible => 5,
            },
            url: e.http.as_ref().map(|h| h.url.clone()).unwrap_or_default(),
            password: e.http.and_then(|h| h.password).unwrap_or_default(),
            listed: e.listed.unwrap_or(true),
        })
        .collect();
    LISTED.with_borrow_mut(|l| {
        l.retain(|x| !(x.plugin == plugin && x.source == source));
        l.push(Listed { plugin: plugin.to_owned(), source: source.to_owned(), entries: recs });
    });
    super::bump();
}

/// Every listed server as `(plugin, record)`.
pub fn all() -> Vec<(String, ServerRec)> {
    LISTED.with_borrow(|l| l.iter().flat_map(|x| x.entries.iter().map(|r| (x.plugin.clone(), r.clone()))).collect())
}

/// Forgets what plugin `plugin`'s sources listed.
pub fn forget(plugin: &str) {
    LISTED.with_borrow_mut(|l| l.retain(|x| x.plugin != plugin));
}

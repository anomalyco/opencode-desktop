//! Live persistence and incremental row diffing for restored window and shared global state.
//!
//! Every poll checks a cheap hash stamp ([`restore_stamp`]) and, only when something changed,
//! writes solely the window rows (`opencode.window.<id>.dat`), shared global keys
//! (`opencode.global.dat`), and per-tab draft documents that actually differ from what was last
//! persisted.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use opencode_kit::servers::key::Route as Page;
use opencode_servers::tabs::ClosedTab;
use opencode_kit::servers::TabKey;
use serde_json::{Value, json};
use opencode_runtime::composer_mode::Mode;

use crate::state::organize_expanded_sorted;
use crate::{GLOBAL, RestoreHost, RestoreRoute, doc, draft_doc, hydrate_restore, hydrated, parked, show_draft, undo};

/// The page as Desktop's last active URL keeps it (`route-storage.ts`): Settings, while open, is
/// the page (`/settings?tab=`), whatever it was opened over.
fn page_href(host: &dyn RestoreHost) -> String {
    if host.settings_open() {
        return Page::Settings(Some(host.settings_page_id().to_owned())).href();
    }
    route_href(&host.route())
}

fn route_href(route: &RestoreRoute) -> String {
    match route {
        RestoreRoute::Home => Page::Home,
        RestoreRoute::Draft(id) => Page::Draft(id.clone()),
        RestoreRoute::Session { server, id } => Page::Session { server: server.clone(), session_id: id.clone() },
    }
    .href()
}

/// The window's own rows (Desktop's `opencode.window.<id>.dat` keys) as they stand. The side
/// panels of sessions with a tab open here are `side` (in the boot record, the first frame
/// draws them); every other session's are `side.rest`, read after it, so the record doesn't
/// grow with every session ever opened (3,600 sessions made it 1.2 MB).
fn window_keys(host: &dyn RestoreHost) -> [(&'static str, Value); 6] {
    let key = host
        .current_tab_uid()
        .or_else(|| host.home_return_tab().map(str::to_owned))
        .and_then(|uid| host.tab_key_by_uid(&uid))
        .map(|k| k.encode());
    let target = host
        .last_project()
        .map(|(server, directory)| json!({ "server": server, "directory": directory }));
    let recent = json!({ "key": key, "target": target });
    let tabs = host.tabs();
    let route = host.route();
    let hot = |k: &str| tabs.iter().any(|t| t.id == k) || matches!(&route, RestoreRoute::Session { id, .. } if id == k);
    [
        ("lastActiveUrl", json!(page_href(host))),
        ("tabs.recent", recent),
        ("side", host.side_json_where(&hot)),
        ("side.rest", host.side_json_where(&|k| !hot(k))),
        ("tabs.closed", closed_json(&host.closed_tabs_doc(), &host.restore().unread_closed)),
        ("rune.closedDrafts", host.closed_drafts_json()),
    ]
}

/// Every window's rows this one shares (Desktop's global `layout` sidebar, `sidebar.inbox`,
/// `sidebar.organize`, `sidebar.view`, `sidebar.recent`).
fn global_keys(host: &dyn RestoreHost) -> [(&'static str, Value); 5] {
    [
        ("layout.sidebar", json!({ "opened": host.sidebar_open(), "width": host.sidebar_width() })),
        ("sidebar.inbox", host.restore().inbox.to_json()),
        ("sidebar.organize", organize_json_of(host)),
        ("sidebar.view", host.sidebar_view_json()),
        ("sidebar.recent", host.sidebar_recent_json()),
    ]
}

/// Each draft's document under its tab's key (`draft:<id>`, or the session's server and
/// route): its text, its mode when it is shell, and the rest of it (`draft_doc`), so a draft
/// comes back as it was left, empty or not. A draft with nothing in it has none.
fn drafts_now(host: &dyn RestoreHost) -> HashMap<String, String> {
    let r = host.restore();
    let mut drafts: HashMap<String, (String, Mode)> = r.drafts.iter().map(|(k, t)| (k.clone(), (t.clone(), Mode::Normal))).collect();
    for k in r.parked.keys().chain(r.blobs_pending()) {
        drafts.entry(k.clone()).or_insert_with(|| (String::new(), Mode::Normal));
    }
    for (k, mode) in host.parked_composer_modes() {
        drafts.entry(k).or_insert_with(|| (String::new(), Mode::Normal)).1 = mode;
    }
    if let Some(k) = &r.shown {
        drafts.insert(k.clone(), (host.composer().text.clone(), host.composer_mode()));
    }
    drafts
        .into_iter()
        .filter_map(|(k, (text, mode))| {
            let parts = if r.shown.as_deref() == Some(k.as_str()) {
                parked::composer_parts(host)
            } else {
                r.parked.get(&k).map(draft_doc::Parts::of)
            };
            let v = draft_doc::encode(&text, mode == Mode::Shell, parts, &r.blobs, r.blobs.pending_for(&k))?;
            Some((draft_tab_key(host, &k).encode(), v))
        })
        .collect()
}

/// Every draft's document as it stands, by tab key: what a closed window keeps (`windows::reopen`).
pub fn draft_documents(host: &dyn RestoreHost) -> HashMap<String, String> {
    drafts_now(host)
}

/// Records what stands now as written: the rows read at launch.
pub(crate) fn note_written(host: &mut dyn RestoreHost) {
    let keys: HashMap<&'static str, Value> = window_keys(host).into_iter().chain(global_keys(host)).collect();
    let drafts = drafts_now(host);
    let r = host.restore_mut();
    r.written.keys = keys;
    r.written.drafts.extend(drafts);
}

/// The document draft `key`'s words are kept in (none in a window without an id, which keeps
/// no drafts): the outbox clears it in the same transaction as the prompt's first row.
pub fn draft_document_of(host: &dyn RestoreHost, key: &str) -> Option<String> {
    let id = host.window_id();
    (!id.is_empty()).then(|| opencode_storage::draft_document(id, &draft_tab_key(host, key).encode()))
}

/// The tab key draft `key` is kept under: a draft tab's, or its session's on its server.
pub fn draft_tab_key(host: &dyn RestoreHost, key: &str) -> TabKey {
    if let Some(tab_key) = host.tab_key_by_id(key) {
        return tab_key;
    }
    if key.starts_with("draft_") {
        return TabKey::draft(key);
    }
    let server = match host.route() {
        RestoreRoute::Session { server, id } if id == key => server,
        _ => host.current_server(),
    };
    TabKey::session(server, key)
}

pub(crate) fn restore_stamp(host: &dyn RestoreHost) -> u64 {
    let mut h = DefaultHasher::new();
    let r = host.restore();
    let composer = host.composer();
    host.last_project().hash(&mut h);
    (
        host.route(),
        host.sidebar_open(),
        host.sidebar_width().to_bits(),
        &composer.text,
        host.composer_mode(),
        host.closed_tabs_stamp(),
        r.inbox,
    )
        .hash(&mut h);
    (r.drafts.len(), r.shown.as_deref()).hash(&mut h);
    // The rest of the shown draft and what the parked ones hold (`draft_doc`).
    (
        composer.range(),
        host.attach_files().len(),
        host.attach_paths().len(),
        host.attach_context().len(),
        host.quote_revision(),
    )
        .hash(&mut h);
    (r.parked.len(), r.blobs.revision).hash(&mut h);
    host.settings_open().then(|| host.settings_page_id()).hash(&mut h);
    host.side_stamp().hash(&mut h);
    host.organize_collapsed().hash(&mut h);
    if let Some(expanded) = host.organize_expanded() {
        organize_expanded_sorted(expanded).hash(&mut h);
    }
    if let Some(order) = host.organize_order() {
        order.hash(&mut h);
    }
    host.sidebar_recent_pins_stamp().hash(&mut h);
    host.sidebar_view_stamp().hash(&mut h);
    h.finish()
}

/// Called every poll: keeps drafts per route and, on any change, hands the rows that changed
/// to the storage (memory now, the database behind). A keystroke changes one draft's
/// document; a sidebar drag step the shared `layout`.
pub fn poll_restore(host: &mut dyn RestoreHost) {
    let route = host.route();
    host.set_active_exit_session(route.session_id());
    let _ = show_draft(host);
    if host.persists() {
        draft_doc::poll_draft_files(host);
    }
    host.freeze_organize_order();
    hydrate_restore(host);
    let stamp = restore_stamp(host);
    if host.restore().stamp == Some(stamp) {
        return;
    }
    host.restore_mut().stamp = Some(stamp);
    if !host.persists() {
        return;
    }
    let is_hydrated = hydrated(host);
    for (key, value) in window_keys(host) {
        // The closed tabs and the other sessions' panels on disk aren't in memory until the
        // window is hydrated.
        if (matches!(key, "tabs.closed" | "rune.closedDrafts" | "side.rest") && !is_hydrated)
            || host.restore().written.keys.get(key) == Some(&value)
        {
            continue;
        }
        doc::save_window_key(host, key, &value);
        host.restore_mut().written.keys.insert(key, value);
    }
    for (key, value) in global_keys(host) {
        if host.restore().written.keys.get(key) == Some(&value) {
            continue;
        }
        match key.split_once('.') {
            Some(("layout", sub)) => {
                let mut slot = Some((sub.to_owned(), value.clone()));
                host.storage_update_json(GLOBAL, "layout", &mut |m| {
                    if let Some((k, v)) = slot.take() {
                        m.insert(k, v);
                    }
                });
            }
            _ => host.storage_set_json(GLOBAL, key, &value),
        }
        host.restore_mut().written.keys.insert(key, value);
    }
    let drafts = drafts_now(host);
    let gone: Vec<String> = host
        .restore()
        .written
        .drafts
        .keys()
        .filter(|k| !drafts.contains_key(*k))
        .cloned()
        .collect();
    for key in gone {
        host.restore_mut().written.drafts.remove(&key);
        if let Some(tab) = TabKey::parse(&key) {
            doc::save_window_draft(host, &tab, None);
            undo::forget_undo(host, &tab);
        }
    }
    for (key, draft) in drafts {
        if host.restore().written.drafts.get(&key) == Some(&draft) {
            continue;
        }
        if let Some(tab) = TabKey::parse(&key) {
            doc::save_window_draft(host, &tab, Some(draft.clone()));
        }
        host.restore_mut().written.drafts.insert(key, draft);
    }
}

/// `tabs.closed`, as the window keeps it, with the entries this build can't read after its own.
pub(crate) fn closed_json(closed: &[ClosedTab], unread: &[Value]) -> Value {
    closed.iter().map(ClosedTab::to_json).chain(unread.iter().cloned()).collect()
}

fn organize_json_of(host: &dyn RestoreHost) -> Value {
    let expanded_map: serde_json::Map<String, Value> = host
        .organize_expanded()
        .map(|set| organize_expanded_sorted(set).into_iter().map(|k| (k.clone(), Value::Bool(true))).collect())
        .unwrap_or_default();
    let order = host.organize_order().map(|o| json!(o)).unwrap_or_else(|| json!({}));
    json!({ "projectsCollapsed": host.organize_collapsed(), "expanded": expanded_map, "order": order })
}

//! What survives a relaunch beyond the tabs: the open page, every composer draft, the closed
//! tabs ⇧⌘T reopens and the inbox's disclosures. The auto-rebuild restarts the app on each
//! merge, so live mode keeps this current on every change, in the app's database
//! (`opencode_storage`), one row per thing, written behind: the window's own (its page, panes and
//! closed tabs as `lastActiveUrl`, `tabs.recent`, `side`, `tabs.closed` of
//! `opencode.window.<id>.dat`, each draft its own document), and what every window shares in
//! Desktop's global keys (`layout`'s sidebar, `sidebar.inbox`, `sidebar.organize`, `sidebar.view`,
//! `sidebar.recent`). A keystroke writes only its draft's document; a sidebar drag only `layout`.
//!
//! All entrypoints operate on non-generic `&dyn RestoreHost` / `&mut dyn RestoreHost`.

#![forbid(unsafe_code)]

pub mod copies;
pub mod doc;
pub mod draft_doc;
pub mod host;
pub mod parked;
pub mod persist;
pub mod places;
pub mod preview;
pub mod state;
pub mod undo;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use opencode_i18n::t;
use opencode_kit::servers::key::Route as Page;
use opencode_servers::tabs::WindowDoc;
use opencode_kit::servers::{ServerKey, TabKey};
use opencode_storage::FlushReason;
use opencode_ui::toast;
use serde_json::Value;
use opencode_runtime::composer_mode::Mode;

pub use copies::{Copies, ask_copy, poll_copies};
pub use doc::{load_window_raw, read_window, read_window_json, save_window_draft, save_window_key, window_drafts};
pub use draft_doc::{Blobs, Decoded, Parts, decode as decode_draft, encode as encode_draft, poll_draft_files};
pub use host::RestoreHost;
pub use parked::{
    ContextFile, Parked, composer_holds, composer_parked, draft_has_content, draft_parked, drop_shown_draft,
    land_upload, park_draft, park_shown_draft,
};
pub use persist::{draft_document_of, draft_documents, draft_tab_key, poll_restore};
pub use places::{Kept as KeptPlaces, land_place, poll_places, restore_places};
pub use preview::{Previews, encode as encode_preview, poll_preview, preview_place, stamp as preview_stamp};
pub use state::{
    Hydration, InboxUi, OrganizeState, Restore, RestoreRoute, RestoreTab, StorageNotice, draft_key, organize_json,
    parse_organize,
};
pub use undo::{Kept as KeptUndo, poll_undo, undo_document};

use persist::{closed_json, note_written, restore_stamp};
use state::Written;

/// The app-wide keys Desktop keeps the sidebar's and the inbox's state in.
pub const GLOBAL: &str = "opencode.global.dat";
/// Minimum sidebar width restored from global layout (`sidebar.rs`).
pub const SIDEBAR_MIN: f32 = 136.0;
/// Maximum sidebar width restored from global layout (`sidebar.rs`).
pub const SIDEBAR_MAX: f32 = 640.0;

/// A saved page.
#[derive(Debug, PartialEq)]
enum Saved {
    Route(RestoreRoute),
    /// Settings on this tab, which closes to Home (Desktop's `source` without a saved one).
    Settings(String),
}

fn route_from(href: &str) -> Option<Saved> {
    Some(match Page::parse(href)? {
        Page::Home | Page::Settings(None) => Saved::Route(RestoreRoute::Home),
        Page::Settings(Some(tab)) => Saved::Settings(tab),
        Page::Draft(id) => Saved::Route(RestoreRoute::Draft(id)),
        Page::Session { server, session_id } => Saved::Route(RestoreRoute::session(server, session_id)),
    })
}

/// Puts back the window's saved page, drafts, closed tabs and panes, and what every window
/// shares (live mode, before the first frame). `server` is the app's own server.
pub fn restore_ui(host: &mut dyn RestoreHost, doc: &WindowDoc, server: &str) {
    let tabs = host.tabs();
    let home_return = doc.recent.as_ref().and_then(|key| tabs.iter().find(|t| t.key() == *key)).map(|t| t.uid.clone());
    host.set_home_return_tab(home_return);

    let namespace = opencode_storage::window_namespace(host.window_id());
    let last_project = host.storage_json(&namespace, "tabs.recent").and_then(|r| {
        Some((ServerKey::new(r["target"]["server"].as_str()?), r["target"]["directory"].as_str()?.to_owned()))
    });
    host.set_last_project(last_project);

    host.restore_mut().drafts = HashMap::new();
    take_drafts(host, &doc.drafts);
    host.restore_closed_tabs(&doc.closed);
    host.restore_mut().unread_closed = doc.unread_closed.clone();

    let inbox_v = host.storage_json(GLOBAL, "sidebar.inbox").unwrap_or(Value::Null);
    let organize_v = host.storage_json(GLOBAL, "sidebar.organize").unwrap_or(Value::Null);
    let view_v = host.storage_json(GLOBAL, "sidebar.view").unwrap_or(Value::Null);
    let recent_v = host.storage_json(GLOBAL, "sidebar.recent").unwrap_or(Value::Null);
    let layout_v = host.storage_json(GLOBAL, "layout").unwrap_or(Value::Null);
    host.restore_mut().inbox = InboxUi::from_json(&inbox_v);
    host.apply_organize(parse_organize(&organize_v));
    host.restore_sidebar_view(&view_v);
    host.restore_sidebar_recent(&recent_v);

    let saved = match doc.last_active_url.as_deref().and_then(route_from) {
        Some(Saved::Route(route)) => Some(route),
        Some(Saved::Settings(tab)) => {
            host.open_restored_settings(&tab);
            Some(RestoreRoute::Home)
        }
        None => None,
    };

    let mut skipped_session = false;
    if let Some(route) = saved {
        // The window starts on the app's own server (`App::load_live`).
        let own = ServerKey::sidecar();
        let waiting = host.sidecar_launch_waiting();
        let lost = match &route {
            // A session of another server keeps its page while its tab does: that server
            // answers after the window opens.
            RestoreRoute::Session { server, id } => {
                !(waiting && *server == own) && !tabs.iter().any(|t| t.id == *id) && !(*server == own && host.has_session(id))
            }
            RestoreRoute::Draft(id) => !tabs.iter().any(|t| t.id == *id && t.is_draft),
            RestoreRoute::Home => false,
        };
        if !lost {
            if let RestoreRoute::Session { server: on, id } = &route {
                if host.take_skip_restore(id) {
                    let title = tabs
                        .iter()
                        .find(|t| t.matches(on, id) && !t.title.trim().is_empty())
                        .map(|t| t.title.trim().to_owned())
                        .or_else(|| host.session_title(id).map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned))
                        .unwrap_or_default();
                    host.note_skipped_restore(on.clone(), id.clone(), title);
                    skipped_session = true;
                } else {
                    host.note_restoring_session(id);
                }
            }
            if !skipped_session {
                if waiting
                    && let RestoreRoute::Session { server: on, id } = &route
                    && *on == own
                    && !host.has_transcript(id)
                    && preview::take_boot_preview(host, on, id)
                {
                    host.set_cold_transcript(Some(id.clone()));
                }
                host.set_route(route);
            }
        }
    }

    let sidebar = &layout_v["sidebar"];
    if let Some(open) = sidebar["opened"].as_bool() {
        host.set_sidebar_open(open);
    }
    if let Some(w) = sidebar["width"].as_f64() {
        host.set_sidebar_width((w as f32).clamp(SIDEBAR_MIN, SIDEBAR_MAX));
    }
    host.restore_side_json(doc.side.as_ref().unwrap_or(&Value::Null));
    places::restore_places(host);
    host.load_persisted_terminals(server);
    let _ = show_draft(host);

    // What was read is what's stored: nothing is written until something changes.
    host.restore_mut().written = Written::default();
    note_written(host);
    // `side` is stored as it was read (a profile from before `side.rest` has every session's
    // panels in it): once hydrated, the split is written (`hydrate_restore`).
    if let Some(side) = &doc.side {
        host.restore_mut().written.keys.insert("side", side.clone());
    }
    let stamp = restore_stamp(host);
    host.restore_mut().stamp = Some(stamp);
    if skipped_session {
        let r = host.restore_mut();
        r.written.keys.remove("lastActiveUrl");
        r.stamp = None;
    }
    if host.storage_is_ready() {
        hydrate_restore(host);
    }
}

/// Says where the window's state is when it isn't simply the saved state ([`StorageNotice`]),
/// from the first poll on: a held file's stand-in shows with its notice. True when it did.
pub fn tell_storage_notice(host: &mut dyn RestoreHost) -> bool {
    match host.take_storage_notice() {
        Some(StorageNotice::Memory(why)) => {
            host.toast_fail(t!("rune.storage.memoryOnly.title"), &t!("rune.storage.memoryOnly.description", why = toast::plain(&why)));
        }
        Some(StorageNotice::Waiting) => host.toast_fail(t!("rune.storage.waiting.title"), t!("rune.storage.waiting.description")),
        Some(StorageNotice::InUse) => host.toast_fail(t!("rune.storage.inUse.title"), t!("rune.storage.inUse.description")),
        Some(StorageNotice::Damaged(kept)) => {
            host.toast_fail(t!("rune.storage.damaged.title"), &t!("rune.storage.damaged.description", path = toast::plain(&kept)));
        }
        Some(StorageNotice::Checkpoint) => host.toast_fail(t!("rune.storage.memoryOnly.title"), t!("rune.storage.checkpoint.description")),
        Some(StorageNotice::FirstRun) => host.toast_fail(t!("rune.storage.memoryOnly.title"), t!("rune.storage.firstRun.description")),
        Some(StorageNotice::Saving) => host.toast_succeed(t!("rune.storage.saving.title"), ""),
        Some(StorageNotice::NotSaving { full: true, .. }) => host.toast_fail(t!("rune.storage.diskFull.title"), t!("rune.storage.diskFull.description")),
        Some(StorageNotice::NotSaving { full: false, why }) => {
            host.toast_fail(t!("rune.storage.notSaving.title"), &t!("rune.storage.writeFailed.description", why = toast::plain(&why)));
        }
        Some(StorageNotice::SeedDroppedDrafts { count, official_dir }) => {
            let title = if count == 1 {
                t!("rune.storage.seedDroppedDrafts.titleOne").to_owned()
            } else {
                t!("rune.storage.seedDroppedDrafts.titleMany", count = count)
            };
            let desc = if official_dir.is_empty() {
                t!("rune.storage.seedDroppedDrafts.description").to_owned()
            } else {
                t!("rune.storage.seedDroppedDrafts.descriptionPath", path = toast::plain(&official_dir))
            };
            host.toast_fail(&title, &desc);
        }
        None => return false,
    }
    true
}

/// Takes saved drafts into the window: a draft is its text, or `{ text, mode }` for one in
/// shell mode, as Desktop keeps the mode in the draft it persists (`ComposerPersistedState.mode`).
/// A draft the window already holds keeps its own.
fn take_drafts(host: &mut dyn RestoreHost, drafts: &[(TabKey, Value)]) {
    for (key, d) in drafts {
        let k = match key {
            TabKey::Draft(id) => id.clone(),
            TabKey::Session { session_id, .. } => session_id.clone(),
        };
        let skip = {
            let r = host.restore_mut();
            r.written.drafts.insert(key.encode(), d.to_string());
            r.drafts.contains_key(&k) || r.parked.contains_key(&k) || r.shown.as_deref() == Some(k.as_str())
        };
        if skip {
            continue;
        }
        let draft = draft_doc::decode(d);
        {
            let r = host.restore_mut();
            if !draft.text.is_empty() {
                r.drafts.insert(k.clone(), draft.text.clone());
            }
            if draft.parked.worth_keeping(&draft.text) {
                r.parked.insert(k.clone(), draft.parked);
            }
        }
        draft_doc::read_draft_files(host, &k, draft.stored);
        if draft.shell {
            host.park_composer_mode(k, Mode::Shell);
        }
    }
}

/// The storage is ready (or the drafts it was reading came in): the window takes its closed
/// tabs and the drafts the boot packet didn't hold. Steps [`Hydration`]; true when it did
/// (the window has more to draw).
pub fn hydrate_restore(host: &mut dyn RestoreHost) -> bool {
    match &host.restore().hydration {
        Hydration::Done => return false,
        Hydration::Loading { read } if !read.load(Ordering::Acquire) => return false,
        _ if !host.storage_is_ready() => return false,
        _ => {}
    }
    let mut expired = Vec::new();
    if matches!(host.restore().hydration, Hydration::Boot) {
        let raw = doc::load_window_raw(host);
        let parsed = WindowDoc::from_json(&raw);
        let now_ms = host.now_ms();
        // Tabs closed since launch stay newest.
        expired = host.hydrate_closed_tabs(&parsed.closed, &raw["rune.closedDrafts"], now_ms);
        let closed = closed_json(&host.closed_tabs_doc(), &parsed.unread_closed);
        let closed_drafts = host.closed_drafts_json();
        host.restore_side_rest(&raw["side.rest"]);
        let r = host.restore_mut();
        r.unread_closed = parsed.unread_closed;
        r.written.keys.insert("tabs.closed", closed);
        r.written.keys.insert("rune.closedDrafts", closed_drafts);
        r.written.keys.insert("side.rest", raw["side.rest"].clone());
        // The next poll compares every row with what's stored (writes only what differs).
        r.stamp = None;
    }
    let (ready, pending) = doc::window_drafts(host);
    take_drafts(host, &ready);
    // A draft closed more than a week ago goes with its words.
    if !expired.is_empty() {
        let r = host.restore_mut();
        for id in expired {
            r.drafts.remove(&id);
            r.parked.remove(&id);
        }
    }
    // A draft that still can't be read after its load stays where it is, unshown.
    if pending.is_empty() || matches!(host.restore().hydration, Hydration::Loading { .. }) {
        host.restore_mut().hydration = Hydration::Done;
        return true;
    }
    let read = Arc::new(AtomicBool::new(false));
    host.restore_mut().hydration = Hydration::Loading { read: read.clone() };
    let keys: Vec<&str> = pending.iter().map(String::as_str).collect();
    let waker = host.waker();
    host.storage_load_docs(
        &keys,
        Box::new(move || {
            read.store(true, Ordering::Release);
            if let Some(w) = waker {
                w();
            }
        }),
    );
    true
}

/// Whether the window has everything it saved (`Hydration::Done`).
pub fn hydrated(host: &dyn RestoreHost) -> bool {
    host.restore().hydrated()
}

/// Settings reopened on its page loads what the page reads, once the server is the source.
pub fn resume_settings_page(host: &mut dyn RestoreHost) {
    if host.settings_open() {
        host.reopen_settings_page(&mut rune::Tx::default());
    }
}

/// When the window closes: the newest draft goes out to the storage, which lands it at once.
pub fn save_on_close(host: &mut dyn RestoreHost) {
    // A tab on its way out takes its window back, and a gap it opened in another closes.
    host.before_close();
    // A delete waiting on this window's toast goes now: the toast goes with the window.
    host.commit_pending_undos();
    if host.persists() {
        poll_restore(host);
        preview::poll_preview(host, true);
        host.flush_persisted_terminals();
    }
    host.after_close();
    host.storage_flush(FlushReason::Close, false);
}

/// When the app starts to quit (`App::on_quit`, every window): what the window keeps is
/// queued, the window list and its frame with it; the last window to quit lands every
/// window's rows in one transaction before the process may end (`windows::quit_saved`).
pub fn save_on_quit(host: &mut dyn RestoreHost) {
    // A delete waiting on this window's toast goes now.
    host.commit_pending_undos();
    if host.persists() {
        poll_restore(host);
        preview::poll_preview(host, true);
        host.flush_persisted_terminals();
    }
    host.notify_quitting();
    host.after_quit();
}

/// Swaps the composer's draft when the route moves to another: its words, pills, attachments,
/// context and quotes go with the draft they were made in. A story's composer is its own.
pub fn show_draft(host: &mut dyn RestoreHost) -> bool {
    let key = draft_key(&host.route());
    if key == host.restore().shown || host.stage_active() {
        return false;
    }
    // Before any draft showed, what a scene put in the composer is the page's draft.
    if host.restore().shown.is_none()
        && (!host.composer().text.is_empty()
            || !host.attach_files().is_empty()
            || !host.attach_paths().is_empty()
            || host.quote_any())
    {
        host.restore_mut().shown = key;
        return false;
    }
    host.leave_queue_edits();
    let old_shown = host.restore().shown.clone();
    host.swap_composer_mode_draft(old_shown.as_deref(), key.as_deref());
    // The field itself (its undo history) waits for the draft to show again.
    let edit = host.composer().clone();
    let (text, parked) = parked::take_composer_draft(host);
    let old = host.restore_mut().shown.take();
    if let Some(old) = old {
        host.restore_mut().edits.insert(old.clone(), edit);
        parked::park_draft(host, old, text, parked);
    }
    // Another draft replaces this one: no list or recalled prompt applies to it (`history.reset`).
    host.suggest_drop();
    parked::put_composer_draft(host, key.as_deref());
    host.restore_mut().shown = key;
    true
}

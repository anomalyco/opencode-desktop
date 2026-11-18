//! Where each of the window's sessions was being read, and which rows its reader opened or shut,
//! kept in the window's own rows (`rune.timeline`) so a relaunch draws them there in the session's
//! first frame, never as a jump after it: the rows are in the boot record, read as the window is
//! built ([`crate::restore_ui`]), and the place is put back in the first commit whose transcript
//! has its row (`Ends::land`): the boot preview's, which starts at the place's turn when it fits
//! ([`crate::preview`]), or a page's, the timeline holding its loading look until then
//! (`session_view::returning`).
//!
//! Written once a change has held still for [`SETTLE`] (a scroll writes when it stops, not per
//! frame), through the kill-safe tier like every window row. A session read at its end keeps no
//! place. The disclosures keep the [`OPENED`] most recently toggled. Desktop keeps a detached
//! timeline only while the app runs (`session/timeline/cache.ts`).

use std::collections::HashMap;

use rune::Tx;
use serde_json::{Map, Value, json};
use session_view::jump::Place;

use crate::{RestoreHost, RestoreRoute};

const KEY: &str = "rune.timeline";
/// How long a change holds still before it's written.
const SETTLE: f64 = 0.25;
/// How many opened or shut rows the window keeps.
const OPENED: usize = 200;

/// What the window last wrote, and the change waiting to hold still.
#[derive(Default)]
pub struct Kept {
    written: Option<Value>,
    candidate: Option<(Value, f64)>,
    /// The toggled disclosures, oldest toggle first, for keeping the newest [`OPENED`].
    order: Vec<String>,
}

/// As the window is built: its sessions' places and opened rows from its rows, and the
/// shown session's place due in the first commit.
pub fn restore_places(host: &mut dyn RestoreHost) {
    let window_id = host.window_id().to_owned();
    if window_id.is_empty() {
        return;
    }
    let Some(v) = host
        .storage_window_rows(&window_id)
        .get(KEY)
        .and_then(|t| serde_json::from_str::<Value>(t).ok())
    else {
        return;
    };
    let places: Vec<Place> = v["places"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(session, p)| {
            Some(Place {
                session: session.clone(),
                row: p[0].as_str()?.to_owned(),
                at: p[1].as_f64()? as f32,
                far: p[2].as_bool().unwrap_or(false),
            })
        })
        .collect();
    let touched: Vec<(String, bool)> = v["opened"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| Some((d[0].as_str()?.to_owned(), d[1].as_bool()?)))
        .collect();
    host.kept_places_mut().order = touched.iter().map(|(k, _)| k.clone()).collect();
    host.expanded_mut().restore(touched);
    host.ends_mut().restore(places);
    if let RestoreRoute::Session { id, .. } = host.route() {
        host.ends_mut().reopen(&id);
    }
    host.kept_places_mut().written = Some(v);
}

/// Puts session `id`'s place back in this commit if its transcript has the place's row
/// (`session_view::returning`); false while the row hasn't come.
pub fn land_place(host: &mut dyn RestoreHost, id: &str, tx: &mut Tx) -> bool {
    let Some(row) = host.ends().due_row(id).map(str::to_owned) else { return false };
    if !host.timeline_has_row(id, &row) {
        return false;
    }
    host.ends_mut().land(id, tx);
    true
}

/// Each poll: the shown session's place lands in the first commit its timeline shows in, and
/// a change to the window's places or opened rows is written once it has held still. True
/// when the timeline changes: its place landed or was given up, so its rows show.
pub fn poll_places(host: &mut dyn RestoreHost, now: f64, tx: &mut Tx) -> bool {
    let changed = poll_due(host, now, tx);
    write_places(host, now, tx);
    changed
}

fn poll_due(host: &mut dyn RestoreHost, now: f64, tx: &mut Tx) -> bool {
    if let RestoreRoute::Session { id, .. } = host.route()
        && host.ends().due(&id)
    {
        // The place lands in the first commit whose transcript has its row; until then the
        // timeline holds its loading look and older pages are read on up to it, and a row
        // that never comes is given up (`session_view::returning`).
        if land_place(host, &id, tx) {
            return true;
        }
        if host.ends_mut().wait(&id, now) {
            host.read_toward_place(&id, tx);
            tx.wake_at(now + 0.25);
            return false;
        }
        return true;
    }
    false
}

fn write_places(host: &mut dyn RestoreHost, now: f64, tx: &mut Tx) {
    if !host.persists() || host.window_id().is_empty() {
        return;
    }
    let value = timeline_json(host);
    // A window with nothing kept writes nothing until it has something.
    let empty_unwritten = host.kept_places().written.is_none()
        && value["places"].as_object().is_some_and(Map::is_empty)
        && value["opened"].as_array().is_some_and(Vec::is_empty);
    if empty_unwritten {
        host.kept_places_mut().written = Some(value);
        return;
    }
    if host.kept_places().written.as_ref() == Some(&value) {
        if host.kept_places().candidate.is_some() {
            host.kept_places_mut().candidate = None;
        }
        return;
    }
    let settle_at = match &host.kept_places().candidate {
        Some((v, at)) if *v == value => Some(*at),
        _ => None,
    };
    match settle_at {
        Some(at) if now - at + 1e-9 >= SETTLE => {
            crate::doc::save_window_key(host, KEY, &value);
            {
                let kept = host.kept_places_mut();
                kept.written = Some(value);
                kept.candidate = None;
            }
            // The boot preview follows the place to its turn, so a launch after a crash
            // draws it in its first frame too.
            crate::preview::preview_place(host);
        }
        Some(at) => tx.wake_at(at + SETTLE),
        None => {
            host.kept_places_mut().candidate = Some((value, now));
            tx.wake_at(now + SETTLE);
        }
    }
}

/// The window's places (its tabs' sessions and the shown one, those not on their end) and
/// its newest toggled rows.
fn timeline_json(host: &mut dyn RestoreHost) -> Value {
    let shown = host.route().session_id().map(str::to_owned);
    let tab_ids = host.session_tab_ids();
    let places: Map<String, Value> = host
        .ends()
        .places(tab_ids.iter().map(String::as_str).chain(shown.as_deref()))
        .into_iter()
        .map(|p| (p.session, json!([p.row, p.at, p.far])))
        .collect();
    let touched: HashMap<String, bool> = host.expanded().touched().map(|(k, v)| (k.clone(), v)).collect();
    let opened: Vec<Value> = {
        let order = &mut host.kept_places_mut().order;
        order.retain(|k| touched.contains_key(k));
        let mut fresh: Vec<&String> = touched.keys().filter(|k| !order.contains(k)).collect();
        fresh.sort();
        order.extend(fresh.into_iter().cloned());
        let from = order.len().saturating_sub(OPENED);
        order[from..].iter().filter_map(|k| touched.get(k).map(|&v| json!([k, v]))).collect()
    };
    json!({ "places": places, "opened": opened })
}

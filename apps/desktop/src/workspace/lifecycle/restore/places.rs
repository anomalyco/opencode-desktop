//! Where each of the window's sessions was being read, and which rows its reader opened or shut,
//! kept in the window's own rows (`rune.timeline`) so a relaunch draws them there in the session's
//! first frame, never as a jump after it: the rows are in the boot record, read as the window is
//! built (`restore_ui`), and the place is put back in the first commit whose transcript has its
//! row (`Ends::land`): the boot preview's, which starts at the place's turn when it fits
//! (`preview`), or a page's, the timeline holding its loading look until then
//! (`session_view::returning`).
//!
//! Written once a change has held still for [`SETTLE`] (a scroll writes when it stops, not per
//! frame), through the kill-safe tier like every window row. A session read at its end keeps no
//! place. The disclosures keep the [`OPENED`] most recently toggled. Desktop keeps a detached
//! timeline only while the app runs (`session/timeline/cache.ts`).

use rune::Tx;
use serde_json::{Map, Value, json};
use session_view::jump::Place;

use crate::{App, Route};

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

impl App {
    /// As the window is built: its sessions' places and opened rows from its rows, and the
    /// shown session's place due in the first commit.
    pub(crate) fn restore_places(&mut self) {
        if self.window.id.is_empty() {
            return;
        }
        let Some(v) = crate::storage::window_rows(&self.window.id).get(KEY).and_then(|t| serde_json::from_str::<Value>(t).ok()) else { return };
        let places: Vec<Place> = v["places"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(session, p)| Some(Place { session: session.clone(), row: p[0].as_str()?.to_owned(), at: p[1].as_f64()? as f32, far: p[2].as_bool().unwrap_or(false) }))
            .collect();
        let touched: Vec<(String, bool)> = v["opened"].as_array().into_iter().flatten().filter_map(|d| Some((d[0].as_str()?.to_owned(), d[1].as_bool()?))).collect();
        self.kernel.slots.slot::<Kept>().order = touched.iter().map(|(k, _)| k.clone()).collect();
        self.expanded.restore(touched);
        self.ends.restore(places);
        if let Route::Session { id, .. } = &*self.route {
            let id = id.clone();
            self.ends.reopen(&id);
        }
        self.kernel.slots.slot::<Kept>().written = Some(v);
    }

    /// Puts session `id`'s place back in this commit if its transcript has the place's row
    /// (`session_view::returning`); false while the row hasn't come.
    pub(crate) fn land_place(&mut self, id: &str, tx: &mut Tx) -> bool {
        let Some(row) = self.ends.due_row(id) else { return false };
        if !self.transcripts.get(id).is_some_and(|t| session_view::timeline::has_row(self, t, row)) {
            return false;
        }
        self.ends.land(id, tx);
        true
    }

    /// Each poll: the shown session's place lands in the first commit its timeline shows in, and
    /// a change to the window's places or opened rows is written once it has held still. True
    /// when the timeline changes: its place landed or was given up, so its rows show.
    pub(crate) fn poll_places(&mut self, now: f64, tx: &mut Tx) -> bool {
        let changed = self.poll_due(now, tx);
        self.write_places(now, tx);
        changed
    }

    fn poll_due(&mut self, now: f64, tx: &mut Tx) -> bool {
        if let Route::Session { id, .. } = &*self.route
            && self.ends.due(id)
        {
            // The place lands in the first commit whose transcript has its row; until then the
            // timeline holds its loading look and older pages are read on up to it, and a row
            // that never comes is given up (`session_view::returning`).
            let id = id.clone();
            if self.land_place(&id, tx) {
                return true;
            }
            if self.ends.wait(&id, now) {
                self.read_toward_place(&id, tx);
                tx.wake_at(now + 0.25);
                return false;
            }
            return true;
        }
        false
    }

    fn write_places(&mut self, now: f64, tx: &mut Tx) {
        if !self.persists() || self.window.id.is_empty() {
            return;
        }
        let value = self.timeline_json();
        // A window with nothing kept writes nothing until it has something.
        let kept = self.kernel.slots.slot::<Kept>();
        if kept.written.is_none() && value["places"].as_object().is_some_and(Map::is_empty) && value["opened"].as_array().is_some_and(Vec::is_empty) {
            kept.written = Some(value);
            return;
        }
        if kept.written.as_ref() == Some(&value) {
            kept.candidate = None;
            return;
        }
        let settle_at = match &kept.candidate {
            Some((v, at)) if *v == value => Some(*at),
            _ => None,
        };
        match settle_at {
            Some(at) if now - at + 1e-9 >= SETTLE => {
                crate::windows::store::save(&self.window, KEY, &value);
                kept.written = Some(value);
                kept.candidate = None;
                // The boot preview follows the place to its turn, so a launch after a crash
                // draws it in its first frame too.
                self.preview_place();
            }
            Some(at) => tx.wake_at(at + SETTLE),
            None => {
                kept.candidate = Some((value, now));
                tx.wake_at(now + SETTLE);
            }
        }
    }

    /// The window's places (its tabs' sessions and the shown one, those not on their end) and
    /// its newest toggled rows.
    fn timeline_json(&mut self) -> Value {
        let shown = match &*self.route {
            Route::Session { id, .. } => Some(id.as_str()),
            _ => None,
        };
        let sessions = self.tabs.iter().filter(|t| !t.is_draft()).map(|t| t.id.as_str()).chain(shown);
        let places: Map<String, Value> = self.ends.places(sessions).into_iter().map(|p| (p.session, json!([p.row, p.at, p.far]))).collect();
        let order = &mut self.kernel.slots.slot::<Kept>().order;
        let touched: std::collections::HashMap<&String, bool> = self.expanded.touched().collect();
        order.retain(|k| touched.contains_key(k));
        let mut fresh: Vec<&String> = touched.keys().filter(|k| !order.contains(k)).copied().collect();
        fresh.sort();
        order.extend(fresh.into_iter().cloned());
        let from = order.len().saturating_sub(OPENED);
        let opened: Vec<Value> = order[from..].iter().map(|k| json!([k, touched[k]])).collect();
        json!({ "places": places, "opened": opened })
    }
}

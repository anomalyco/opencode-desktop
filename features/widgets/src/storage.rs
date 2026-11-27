//! `widget.storage` (`specs/v2/widgets-live.md`, Storage) in this window: a live widget template
//! that declares `storage` keeps its data in its server's space `widget.<name>`, which the host
//! follows with its spaces client ([`WidgetsHost::storage_copy`]). A run starts with the space's
//! copy, its writes go to the space's pending writes ([`WidgetsHost::storage_write`]), and every
//! change of the space reaches every running widget of that template on that server, the
//! writer's own included: the window keeps the copy it last told its runs, and tells them each
//! key that moved since.

use std::collections::BTreeMap;

use crate::{Kind, WidgetTemplate, Widgets, WidgetsHost};

/// A space's copy, as the host follows it.
#[derive(Clone, Debug, Default)]
pub struct StorageCopy {
    /// Moves whenever the copy may have changed (a sync's page, an event, a write); never 0.
    pub moved: u64,
    /// The last sync reached its last page: a key missing is missing on the server too.
    pub complete: bool,
    /// JSON text by key, pending writes applied.
    pub entries: BTreeMap<String, String>,
}

/// The copy of a template's space this window last told its runs.
#[derive(Default)]
pub(crate) struct Told {
    moved: u64,
    complete: bool,
    entries: BTreeMap<String, String>,
}

/// Where a widget's storage is: its server (as the host names it) and its widget template's name.
pub(crate) type Place = (String, String);

/// Widget `id`'s storage, when its widget template is live and declares `storage`.
pub(crate) fn place(host: &dyn WidgetsHost, id: &str) -> Option<Place> {
    let w = host.widgets();
    let e = w.entries.get(id)?;
    let Some(WidgetTemplate::Ready(c)) = w.widget_templates.get(&e.widget_template) else { return None };
    let Kind::Live { grants, .. } = &c.kind else { return None };
    if !grants.storage {
        return None;
    }
    let name = e.widget_template.split_once('@').map_or(e.widget_template.as_str(), |(n, _)| n);
    Some((host.widget_server(&e.session), name.to_owned()))
}

/// The keys whose value differs between `old` and `new`, with the new value (`None`: deleted).
fn moved<'a>(old: &'a BTreeMap<String, String>, new: &'a BTreeMap<String, String>) -> Vec<(&'a str, Option<&'a str>)> {
    let gone = old.keys().filter(|k| !new.contains_key(*k)).map(|k| (k.as_str(), None));
    let set = new.iter().filter(|(k, v)| old.get(*k) != Some(*v)).map(|(k, v)| (k.as_str(), Some(v.as_str())));
    gone.chain(set).collect()
}

/// Takes what moved in `at`'s space since the window last looked to every running widget of
/// it. Whether its copy is a whole sync's.
pub(crate) fn refresh(host: &mut dyn WidgetsHost, at: &Place) -> bool {
    let (seen, complete) = host.widgets().told.get(at).map_or((0, false), |t| (t.moved, t.complete));
    let Some(copy) = host.storage_copy(&at.0, &at.1, seen) else { return complete };
    let told = host.widgets_mut().told.remove(at).unwrap_or_default();
    let changes = moved(&told.entries, &copy.entries);
    #[cfg(not(target_arch = "wasm32"))]
    if !changes.is_empty() {
        let runs: Vec<String> = host.widgets().entries.keys().filter(|id| place(host, id).as_ref() == Some(at)).cloned().collect();
        for id in runs {
            if let Some(live) = host.widgets().entries.get(&id).and_then(|e| e.run.live()) {
                for (key, value) in &changes {
                    live.storage(key, *value);
                }
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = changes;
    let complete = copy.complete;
    host.widgets_mut().told.insert(at.clone(), Told { moved: copy.moved, complete, entries: copy.entries });
    complete
}

/// The copy a run of `place`'s starts with: the one the window last told its runs.
pub(crate) fn entries(w: &Widgets, place: &Place) -> BTreeMap<String, String> {
    w.told.get(place).map(|t| t.entries.clone()).unwrap_or_default()
}

/// The places of the window's running storage widgets, each once.
pub(crate) fn followed(host: &dyn WidgetsHost) -> Vec<Place> {
    let mut places: Vec<Place> = host.widgets().entries.iter().filter(|(_, e)| e.run.running()).filter_map(|(id, _)| place(host, id)).collect();
    places.sort();
    places.dedup();
    places
}

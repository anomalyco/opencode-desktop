//! Per-tab element refs for `experimental.browser` v4 (`eN <-> (backendDOMNodeId, cdp_session, frame)`).

use std::collections::{BTreeMap, HashMap};

const MAX_TAB_REFS: usize = 4_096;

/// Element ref mappings for one tab. Invalidated whenever the tab's document generation changes.
#[derive(Default)]
pub(in super::super) struct TabRefs {
    pub ref_to_target: HashMap<String, (i64, Option<String>)>,
    /// The frame each ref's snapshot was taken in, when it named one; the engine checks it.
    pub ref_frames: HashMap<String, String>,
    /// Reused ref lookup keyed by `(cdp_session, frame, backend_node_id)` (official `refs.ts:11`,
    /// `${sessionID}:${frameID}`): an OOPIF renderer's `backendNodeId`s never collide with the main
    /// frame's, and a main-frame snapshot never takes over a child frame's refs.
    pub backend_to_ref: HashMap<(Option<String>, Option<String>, i64), String>,
    pub picked: HashMap<i64, String>,
}

impl TabRefs {
    pub fn clear_on_nav(&mut self) {
        self.ref_to_target.clear();
        self.ref_frames.clear();
        self.backend_to_ref.clear();
        self.picked.clear();
    }

    pub fn previous_for(&self, cdp_session: Option<&str>, frame: Option<&str>) -> HashMap<i64, String> {
        self.backend_to_ref
            .iter()
            .filter_map(|((sid, fid, bid), r)| (sid.as_deref() == cdp_session && fid.as_deref() == frame).then_some((*bid, r.clone())))
            .collect()
    }

    /// Merges newly assigned refs from a snapshot without wiping other frames' refs (matching
    /// official `chromium.ts`, which clears `refs` only on document navigation or close).
    pub fn update_from_snapshot(&mut self, refs: BTreeMap<String, i64>, cdp_session: Option<String>, frame: Option<String>) {
        if self.ref_to_target.len().saturating_add(refs.len()) > MAX_TAB_REFS {
            self.ref_to_target.retain(|k, (_, s)| s != &cdp_session || self.ref_frames.get(k) != frame.as_ref());
            self.ref_frames.retain(|k, _| self.ref_to_target.contains_key(k));
            self.backend_to_ref.retain(|_, r| self.ref_to_target.contains_key(r));
        }
        if self.ref_to_target.len().saturating_add(refs.len()) > MAX_TAB_REFS {
            self.ref_to_target.clear();
            self.ref_frames.clear();
            self.backend_to_ref.clear();
        }
        for (k, id) in refs {
            self.backend_to_ref.insert((cdp_session.clone(), frame.clone(), id), k.clone());
            match &frame {
                Some(f) => {
                    self.ref_frames.insert(k.clone(), f.clone());
                }
                None => {
                    self.ref_frames.remove(&k);
                }
            }
            self.ref_to_target.insert(k, (id, cdp_session.clone()));
        }
    }

    pub fn frame_of(&self, raw: &str) -> Option<String> {
        let norm = opencode_kit::browser_rpc::normalize_ref(raw)?;
        self.ref_frames.get(&norm).cloned()
    }

    pub fn resolve(&self, raw: &str) -> Option<(i64, Option<String>)> {
        let norm = opencode_kit::browser_rpc::normalize_ref(raw)?;
        self.ref_to_target.get(&norm).cloned()
    }
}

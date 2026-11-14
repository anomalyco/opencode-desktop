//! Row lookup and family costs for a large session catalog. Plugin signals consult
//! both for every root on attachment; scanning the catalog per root is quadratic.
use std::cell::RefCell;
use std::collections::HashMap;

use opencode_kit::model::SessionInfo;
use rune::Tracked;

#[derive(Default)]
pub(crate) struct Catalog(RefCell<Option<Snapshot>>);

struct Snapshot {
    key: (u32, u64),
    row: HashMap<String, usize>,
    /// Only families with children need a sum, kept in the scalar path's depth/row order.
    total: HashMap<usize, f64>,
}

impl Catalog {
    pub fn lookup(&self, sessions: &Tracked<Vec<SessionInfo>>, id: &str) -> Option<(usize, f64)> {
        // Boot and the usual single page don't allocate a catalog. A tracked read
        // is intentional: a memo that uses this result must depend on the sessions.
        if sessions.len() < 512 {
            self.0.borrow_mut().take();
            return None;
        }
        let key = (Tracked::id(sessions), Tracked::version(sessions));
        let mut snapshot = self.0.borrow_mut();
        if snapshot.as_ref().is_none_or(|s| s.key != key) {
            let mut row = HashMap::with_capacity(sessions.len());
            for (i, s) in sessions.iter().enumerate() { row.entry(s.id.clone()).or_insert(i); }
            let mut levels: HashMap<usize, Vec<f64>> = HashMap::new();
            for s in sessions.iter().filter(|s| s.parent.is_some()) {
                let mut parent = s.parent.as_ref();
                for depth in 0..32 {
                    let Some(&i) = parent.and_then(|p|row.get(p)) else {break};
                    parent = sessions[i].parent.as_ref();
                    if parent.is_none() {
                        let sums = levels.entry(i).or_default();
                        if sums.len() <= depth { sums.resize(depth+1, 0.0); }
                        sums[depth] += s.cost;
                        break;
                    }
                }
            }
            let total = levels.into_iter().map(|(i, depths)| (i, depths.into_iter().fold(sessions[i].cost, |sum, v|sum+v))).collect();
            *snapshot = Some(Snapshot { key, row, total });
        }
        let snapshot = snapshot.as_ref()?;
        let i = *snapshot.row.get(id)?;
        Some((i, snapshot.total.get(&i).copied().unwrap_or(sessions[i].cost)))
    }
}

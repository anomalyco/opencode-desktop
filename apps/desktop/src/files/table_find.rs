//! Adapts displayed cell identities to the shared file Find (`DocFind` and SEARCH's `SlabFind`).
//! The one ordered result set drives count, marks and reveal. It cannot be applied to a
//! different read of a file, even if both reads have exactly the same path and dimensions.

use std::{rc::Rc, sync::Arc};
use opencode_search::SlabMatch;
use rune::{TextHighlight, Tx};
use crate::{code_view::file_state::{DocFind, DocFindEvent}, palette::Pal};
use super::{delimited::Table, table_view};

#[derive(Default)]
pub(super) enum TableFind {
    #[default]
    Closed,
    Showing { doc: Arc<Table>, nav: DocFind, hits: Rc<Vec<SlabMatch>> },
}

impl TableFind {
    pub(super) fn forget(&mut self, previous: &Arc<Table>) {
        if matches!(self, Self::Showing {doc, ..} if Arc::ptr_eq(doc, previous)) { *self = Self::Closed; }
    }
    pub(super) fn sync(&mut self, doc: &Arc<Table>, query: &str) -> bool {
        let replaced = !matches!(self, Self::Showing { doc: previous, .. } if Arc::ptr_eq(previous, doc));
        if replaced {
            let mut nav = DocFind::default();
            nav.step(DocFindEvent::Open);
            *self = Self::Showing { doc: doc.clone(), nav, hits: Default::default() };
        }
        let Self::Showing { nav, hits, .. } = self else { return false };
        if !nav.step(DocFindEvent::Query(query)) { return replaced; }
        *hits = Rc::new(doc.matches(query));
        true
    }

    pub(super) fn count(&self, doc: &Arc<Table>) -> (usize, usize) {
        match self {
            Self::Showing { doc: known, nav, hits } if Arc::ptr_eq(doc, known) => (nav.active(), hits.len()),
            _ => (0,0),
        }
    }

    pub(super) fn step(&mut self, forward: bool) {
        if let Self::Showing { hits, nav, .. } = self { nav.step(DocFindEvent::Step { forward, count: hits.len() }); }
    }

    pub(super) fn reveal(&self, sid: &str, path: &str, tx: &mut Tx) {
        let Self::Showing { doc, nav, hits } = self else { return };
        let Some((r,c)) = hits.get(nav.active()).and_then(|m| doc.coordinates(m.row)) else { return };
        let scope = table_view::scroll_key(sid, path);
        // The row may be unmounted. Reveal the virtual row and then its cell on both axes.
        tx.scroll_center_in_instant(&scope, table_view::row_key(r));
        tx.scroll_into_view_in(scope, format!("{}/{}", table_view::row_key(r), table_view::cell_key(r,c)));
    }

    pub(super) fn painter(&self, p: &Pal) -> impl Fn(&Arc<Table>, (usize, usize)) -> Vec<TextHighlight> + 'static {
        let snapshot = match self {
            Self::Showing { doc, nav, hits } => Some((doc.clone(), nav.active(), hits.clone())), Self::Closed => None,
        };
        let pal = *p;
        move |doc, (r,c)| {
            let Some((known, active, hits)) = &snapshot else { return Vec::new() };
            if !Arc::ptr_eq(known, doc) { return Vec::new(); }
            let Some(id) = doc.cell_id(r,c) else { return Vec::new() };
            let first = hits.partition_point(|hit| hit.row < id);
            hits[first..].iter().take_while(|hit| hit.row == id).enumerate()
                .map(|(n, hit)| session_view::style::text_mark(&pal, hit.bytes.clone(), first+n, first+n == *active)).collect()
        }
    }
}

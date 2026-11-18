//! Adapts displayed cell identities to the shared file Find (`DocFind` and SEARCH's `SlabFind`).
//! The one ordered result set drives count, marks and reveal. It cannot be applied to a
//! different read of a file, even if both reads have exactly the same path and dimensions.

use std::{rc::Rc, sync::Arc};

use opencode_search::SlabMatch;
use rune::{TextHighlight, Tx};
use session_view::palette::Pal;

use super::delimited::{self, Table};
use crate::file::state::{DocFind, DocFindEvent};

/// A searchable document whose visible labels map to `SlabFind` rows.
///
/// Implement this on the inner document type (`Table`, HTML `Page`, Mermaid `Diagram`);
/// blanket [`Document`] impls for `Arc<T>` and `Rc<T>` supply pointer-identity comparison
/// (`Arc::ptr_eq` / `Rc::ptr_eq`) so downstream crates avoid Rust's orphan rules.
pub trait LabelDoc {
    type Key;
    fn scan(&self, query: &str) -> Vec<SlabMatch>;
    fn index(&self, key: Self::Key) -> Option<usize>;
    fn reveal_hit(&self, sid: &str, path: &str, hit: &SlabMatch, tx: &mut Tx);
}

pub trait Document: Clone {
    type Key;
    fn same(&self, other: &Self) -> bool;
    fn scan(&self, query: &str) -> Vec<SlabMatch>;
    fn index(&self, key: Self::Key) -> Option<usize>;
    fn reveal_hit(&self, sid: &str, path: &str, hit: &SlabMatch, tx: &mut Tx);
}

impl<T: LabelDoc> Document for Arc<T> {
    type Key = T::Key;
    fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(self, other)
    }
    fn scan(&self, query: &str) -> Vec<SlabMatch> {
        (**self).scan(query)
    }
    fn index(&self, key: Self::Key) -> Option<usize> {
        (**self).index(key)
    }
    fn reveal_hit(&self, sid: &str, path: &str, hit: &SlabMatch, tx: &mut Tx) {
        (**self).reveal_hit(sid, path, hit, tx)
    }
}

impl<T: LabelDoc> Document for Rc<T> {
    type Key = T::Key;
    fn same(&self, other: &Self) -> bool {
        Rc::ptr_eq(self, other)
    }
    fn scan(&self, query: &str) -> Vec<SlabMatch> {
        (**self).scan(query)
    }
    fn index(&self, key: Self::Key) -> Option<usize> {
        (**self).index(key)
    }
    fn reveal_hit(&self, sid: &str, path: &str, hit: &SlabMatch, tx: &mut Tx) {
        (**self).reveal_hit(sid, path, hit, tx)
    }
}

impl LabelDoc for Table {
    type Key = (usize, usize);
    fn scan(&self, query: &str) -> Vec<SlabMatch> {
        self.matches(query)
    }
    fn index(&self, (r, c): Self::Key) -> Option<usize> {
        self.cell_id(r, c)
    }
    fn reveal_hit(&self, sid: &str, path: &str, hit: &SlabMatch, tx: &mut Tx) {
        let Some((r, c)) = self.coordinates(hit.row) else { return };
        let scope = delimited::scroll_key(sid, path);
        tx.scroll_center_in_instant(&scope, delimited::row_key(r));
        tx.scroll_into_view_in(scope, format!("{}/{}", delimited::row_key(r), delimited::cell_key(r, c)));
    }
}

pub type TableFind = LabelFind<Arc<Table>>;

#[derive(Default)]
pub enum LabelFind<D> {
    #[default]
    Closed,
    Showing { doc: D, nav: DocFind, hits: Rc<Vec<SlabMatch>> },
}

impl<D: Document + 'static> LabelFind<D> {
    pub fn forget(&mut self, previous: &D) {
        if matches!(self, Self::Showing { doc, .. } if doc.same(previous)) {
            *self = Self::Closed;
        }
    }

    pub fn sync(&mut self, doc: &D, query: &str) -> bool {
        let replaced = !matches!(self, Self::Showing { doc: previous, .. } if previous.same(doc));
        if replaced {
            let mut nav = DocFind::default();
            nav.step(DocFindEvent::Open);
            *self = Self::Showing { doc: doc.clone(), nav, hits: Default::default() };
        }
        let Self::Showing { nav, hits, .. } = self else { return false };
        if !nav.step(DocFindEvent::Query(query)) {
            return replaced;
        }
        *hits = Rc::new(doc.scan(query));
        true
    }

    pub fn count(&self, doc: &D) -> (usize, usize) {
        match self {
            Self::Showing { doc: known, nav, hits } if known.same(doc) => (nav.active(), hits.len()),
            _ => (0, 0),
        }
    }

    pub fn step(&mut self, forward: bool) {
        if let Self::Showing { hits, nav, .. } = self {
            nav.step(DocFindEvent::Step { forward, count: hits.len() });
        }
    }

    pub fn reveal(&self, sid: &str, path: &str, tx: &mut Tx) {
        let Self::Showing { doc, nav, hits } = self else { return };
        if let Some(hit) = hits.get(nav.active()) {
            doc.reveal_hit(sid, path, hit, tx);
        }
    }

    pub fn painter(&self, p: &Pal) -> impl Fn(&D, D::Key) -> Vec<TextHighlight> + 'static {
        let snapshot = match self {
            Self::Showing { doc, nav, hits } => Some((doc.clone(), nav.active(), hits.clone())),
            Self::Closed => None,
        };
        let pal = *p;
        move |doc, key| {
            let Some((known, active, hits)) = &snapshot else { return Vec::new() };
            if !known.same(doc) {
                return Vec::new();
            }
            let Some(id) = doc.index(key) else { return Vec::new() };
            let first = hits.partition_point(|hit| hit.row < id);
            hits[first..]
                .iter()
                .take_while(|hit| hit.row == id)
                .enumerate()
                .map(|(n, hit)| session_view::style::text_mark(&pal, hit.bytes.clone(), first + n, first + n == *active))
                .collect()
        }
    }
}

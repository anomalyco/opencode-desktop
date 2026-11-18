//! The .mmd and .mermaid stage. Index the immutable drawing's actual text rather than node
//! syntax, comments, accessibility descriptions or temporary hover duplicates.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use opencode_code_view::CodeDocState;
use opencode_search::{SlabFind, SlabMatch};
use rune::{El, TextHighlight, Tx, col};
use rune_kit::mermaid::DrawingModel;
use opencode_ui::palette::Pal;
use opencode_ui::style::markdown_style;

use super::markdown;

pub(crate) fn kind(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("mmd") || ext.eq_ignore_ascii_case("mermaid"))
}

pub(crate) struct Diagram {
    model: DrawingModel,
    keys: Vec<String>,
    ids: HashMap<String, usize>,
    slab: SlabFind,
}

impl Diagram {
    fn new(model: DrawingModel) -> Self {
        let mut slab = SlabFind::default();
        let mut keys = Vec::new();
        let mut ids = HashMap::new();
        for (id, value) in model.labels() {
            slab.push(value);
            ids.insert(id.to_owned(), keys.len());
            keys.push(id.to_owned());
        }
        Self { model, keys, ids, slab }
    }

    pub(crate) fn matches(&self, query: &str) -> Vec<SlabMatch> {
        let mut hits = Vec::new();
        self.slab.find(query, |hit| hits.push(hit));
        hits
    }

    pub(crate) fn label_count(&self) -> usize {
        self.keys.len()
    }

    pub(crate) fn reveal(&self, sid: &str, path: &str, hit: &SlabMatch, tx: &mut Tx) {
        if let Some(key) = self.keys.get(hit.row) {
            tx.scroll_center_in_instant(scroll_key(sid, path), key);
        }
    }

    pub(crate) fn view<H: 'static>(
        &self,
        sid: &str,
        path: &str,
        p: &Pal,
        paint: impl Fn(usize) -> Vec<TextHighlight>,
    ) -> El {
        let d = self
            .model
            .draw(|key| self.ids.get(key).map_or_else(Vec::new, |id| paint(*id)));
        opencode_ui::scroller::scroller(scroll_key(sid, path)).build::<H>(
            p,
            col().w_full().grow(1.0).min_h(0.0).fill(p.bg_base),
            col()
                .w_full()
                .min_h(d.size[1])
                .center()
                .child(d.el.role("figure").label(path.to_owned())),
        )
    }
}

impl super::label_find::LabelDoc for Diagram {
    type Key = usize;
    fn scan(&self, query: &str) -> Vec<SlabMatch> {
        self.matches(query)
    }
    fn index(&self, key: usize) -> Option<usize> {
        (key < self.label_count()).then_some(key)
    }
    fn reveal_hit(&self, sid: &str, path: &str, hit: &SlabMatch, tx: &mut Tx) {
        self.reveal(sid, path, hit, tx);
    }
}

fn scroll_key(sid: &str, path: &str) -> String {
    format!("file-diagram-{sid}-{path}-scroll")
}

/// One prepared diagram per session, only for the pane last drawn. Do not retain an index for
/// every source tab or parse a known unsupported drawing again on every frame.
#[derive(Default)]
pub(crate) struct Stage(RefCell<Option<Layout>>);

struct Layout {
    rev: u64,
    width: u32,
    theme: (usize, u64, bool, bool),
    diagram: Option<Rc<Diagram>>,
}

impl Stage {
    pub(crate) fn clear(&mut self, doc: &CodeDocState) {
        if self.0.get_mut().as_ref().is_some_and(|c| c.rev == doc.keys_rev) {
            *self.0.get_mut() = None;
        }
    }

    pub(crate) fn current(&self, doc: &CodeDocState) -> Option<Rc<Diagram>> {
        self.0
            .borrow()
            .as_ref()
            .filter(|c| c.rev == doc.keys_rev)?
            .diagram
            .clone()
    }

    pub(crate) fn prepare(&self, sid: &str, path: &str, doc: &CodeDocState, p: &Pal) {
        if !kind(path) || !doc.ready() || doc.binary() || doc.missing() || doc.size_bytes() > markdown::RENDER_LIMIT {
            return;
        }
        let width = rune::memo::layout_with(&scroll_key(sid, path), |r| r[2])
            .unwrap_or(720.0)
            .clamp(40.0, 896.0);
        let theme = (p.theme, p.generation, p.dark, p.forced);
        if self
            .0
            .borrow()
            .as_ref()
            .is_some_and(|s| s.rev == doc.keys_rev && s.width == width.to_bits() && s.theme == theme)
        {
            return;
        }
        let style = markdown_style(p, p.text, width);
        let diagram = rune_kit::mermaid::prepare(doc.text(), &style, width).map(|m| Rc::new(Diagram::new(m)));
        self.0.replace(Some(Layout {
            rev: doc.keys_rev,
            width: width.to_bits(),
            theme,
            diagram,
        }));
    }
}

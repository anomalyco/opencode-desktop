//! Find in a diff (⌘F in Review): Pierre's `createFileFind` over the diff's rendered lines
//! (`session-ui/src/pierre/file-find.ts`), as Desktop finds in its Review diffs. It searches the
//! lines the diff shows, once each (a context line in both columns of a split diff counts once),
//! folded as every Rune search folds, and lights them in place.

use std::collections::HashMap;
use std::ops::Range;

use rune::{Key, KeyEvent, TextEdit, Tx};
use rune_kit::diff::{LineKind, Side};

use super::state::{DiffDocState, DiffRow};
use crate::KeyMatch;
use crate::file::state::{DocFind, DocFindEvent};

/// A shown line: its file, side and number.
type LineId = (u32, Side, u32);

#[derive(Default)]
pub struct DiffFind {
    pub input: TextEdit,
    state: DocFind,
    /// The shown lines in row order and their folded text, made when a query first needs them
    /// for the rows as built (`keys_rev`) and dropped when Find closes.
    slab: Option<(u64, Vec<(u32, LineId)>, opencode_search::SlabFind)>,
    /// `(row, line, byte range)` in row order.
    hits: Vec<(u32, LineId, Range<usize>)>,
    by_line: HashMap<LineId, Vec<(Range<usize>, bool)>>,
}

impl DiffDocState {
    pub fn find_open(&self) -> bool {
        matches!(self.find.state, DocFind::Open { .. })
    }

    pub fn find_query(&self) -> Option<String> {
        self.find_open().then(|| self.find.input.text.clone())
    }

    pub(super) fn seed_find(&mut self, query: String) {
        self.find.state.step(DocFindEvent::Open);
        self.find.state.step(DocFindEvent::Query(&query));
        self.find.input = TextEdit::new(&query);
    }

    /// Carries the live Find bar state (`focused`, caret, or edits made during a background
    /// refresh) from `prev` onto `self`.
    pub fn carry_find_from(&mut self, prev: &Self) {
        if !prev.find_open() {
            if self.find_open() {
                self.set_find_open(false);
            }
            return;
        }
        if !self.find_open() {
            self.find.state.step(DocFindEvent::Open);
        }
        self.find.input = prev.find.input.clone();
        self.sync_find_query();
    }

    pub fn set_find_open(&mut self, open: bool) {
        let event = if open { DocFindEvent::Open } else { DocFindEvent::Close };
        if self.find.state.step(event) {
            self.find.input = TextEdit::new("");
            self.find.slab = None;
            self.refind();
        }
        self.find.input.focused = open;
    }

    /// The field's text is the query: true when the matches changed.
    pub fn sync_find_query(&mut self) -> bool {
        if !self.find.state.step(DocFindEvent::Query(&self.find.input.text)) {
            return false;
        }
        self.refind();
        true
    }

    pub fn find_count(&self) -> usize {
        self.find.hits.len()
    }

    pub fn find_index(&self) -> usize {
        self.find.state.active()
    }

    /// The lit match moves on (or back), wrapping, and comes into view.
    pub fn step_find(&mut self, forward: bool, tx: &mut Tx) {
        if !self.find.state.step(DocFindEvent::Step { forward, count: self.find.hits.len() }) {
            return;
        }
        self.light();
        tx.instant();
        let row = self.find.hits[self.find.state.active()].0;
        self.reveal_below_header(row, tx);
    }

    /// Line `n` of `side` in file `f`'s matches, and whether each is the lit one.
    pub(crate) fn find_line(&self, f: usize, side: Side, n: u32) -> &[(Range<usize>, bool)] {
        self.find.by_line.get(&(f as u32, side, n)).map_or(&[], Vec::as_slice)
    }

    /// Find's keys while its field has the keyboard: Enter steps (Shift+Enter back), Escape
    /// closes; an input method's own Enter and Escape come first.
    pub(super) fn find_key(&mut self, ev: &KeyEvent, tx: &mut Tx, keys: &dyn KeyMatch) -> bool {
        if keys.is_find(ev, tx) {
            tx.instant();
            self.set_find_open(true);
            return true;
        }
        if !self.find_open() || !self.find.input.focused || self.find.input.is_composing() {
            return false;
        }
        match ev.key {
            Key::Escape => {
                tx.instant();
                self.set_find_open(false);
                true
            }
            Key::Enter => {
                self.sync_find_query();
                self.step_find(!ev.mods.shift, tx);
                true
            }
            _ => false,
        }
    }

    /// Finds the query again over the rows as they are.
    pub(super) fn refind(&mut self) {
        self.rev += 1;
        self.find.hits.clear();
        let query = self.find.state.query().to_owned();
        if query.is_empty() {
            self.find.by_line.clear();
            return;
        }
        if self.find.slab.as_ref().is_none_or(|(rev, ..)| *rev != self.keys_rev) {
            let lines = self.shown_lines();
            let mut slab = opencode_search::SlabFind::default();
            for &(_, (f, side, n)) in &lines {
                slab.push(self.side_line(f as usize, side, n).unwrap_or(""));
            }
            self.find.slab = Some((self.keys_rev, lines, slab));
        }
        if let Some((_, lines, slab)) = &self.find.slab {
            slab.find(&query, |m| {
                let (row, line) = lines[m.row];
                self.find.hits.push((row, line, m.bytes));
            });
        }
        if self.find.state.active() >= self.find.hits.len() {
            self.find.state.step(DocFindEvent::Query(""));
            self.find.state.step(DocFindEvent::Query(&query));
        }
        self.light();
    }

    fn light(&mut self) {
        self.rev += 1;
        let active = self.find.state.active();
        self.find.by_line.clear();
        for (i, (_, line, range)) in self.find.hits.iter().enumerate() {
            self.find.by_line.entry(*line).or_default().push((range.clone(), i == active));
        }
    }

    /// Each line the rows show, once, with its row.
    fn shown_lines(&self) -> Vec<(u32, LineId)> {
        let mut out = Vec::new();
        for (i, &r) in self.rows.iter().enumerate() {
            let i = i as u32;
            match r {
                DiffRow::Ctx { f, new, .. } => out.push((i, (f, Side::New, new))),
                DiffRow::Line { f, h, i: l } => {
                    let line = &self.files[f as usize].hunks[h as usize].lines[l as usize];
                    let id = match (line.kind, line.old, line.new) {
                        (LineKind::Removed, Some(n), _) => (f, Side::Old, n),
                        (_, _, Some(n)) => (f, Side::New, n),
                        _ => continue,
                    };
                    out.push((i, id));
                }
                DiffRow::Pair { f, h, del, add } => {
                    let lines = &self.files[f as usize].hunks[h as usize].lines;
                    out.extend(lines[del as usize].old.map(|n| (i, (f, Side::Old, n))));
                    out.extend(lines[add as usize].new.map(|n| (i, (f, Side::New, n))));
                }
                DiffRow::File(_) | DiffRow::Note(_) | DiffRow::Gap { .. } => {}
            }
        }
        out
    }
}

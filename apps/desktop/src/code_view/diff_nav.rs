//! Selection, scroll reveal, and keyboard navigation (`j`/`k`, `]`/`[`, `←`/`→`, `⌘C`) for [`DiffDocState`].

use rune::{Key, KeyEvent, Tx};
use rune_kit::diff::{DiffLine, Expand, LineRange, LineRef, Side};
use rune_kit::line_selection::{Column, LineEvent};

use super::HEADER_H;
use super::diff_state::{DiffDocState, DiffRow};

impl DiffDocState {
    pub fn selection(&self) -> Option<LineRange> {
        self.lines.selected().map(LineRange::from)
    }

    /// Selects `sel`, ending any press on the line numbers.
    pub fn set_selection(&mut self, sel: Option<LineRange>) {
        let _ = self.lines.step(LineEvent::Set(sel.map(LineRange::lines)), |_| None);
    }

    pub fn selected_text(&self) -> Option<String> {
        let sel = self.selection()?;
        let side = sel.start.side;
        let (a, b) = self.selected_rows(&sel)?;
        let mut out = Vec::new();
        for r in a..=b {
            if let Some(n) = self.row_line(self.rows[r as usize], side) {
                out.push(self.side_line(sel.file, side, n).unwrap_or_default().to_owned());
            }
        }
        Some(out.join("\n"))
    }

    fn selected_rows(&self, sel: &LineRange) -> Option<(u32, u32)> {
        let a = *self.row_of.get(&(sel.file as u32, sel.start.side, sel.start.line))?;
        let b = *self.row_of.get(&(sel.file as u32, sel.end.side, sel.end.line))?;
        Some((a.min(b), a.max(b)))
    }

    pub(crate) fn row_selected(&self, r: usize) -> bool {
        let Some(sel) = self.selection() else { return false };
        let Some((a, b)) = self.selected_rows(&sel) else { return false };
        self.rows[r].file() == sel.file && (a as usize..=b as usize).contains(&r)
    }

    /// A press on line `at`'s number (rune-kit's shared gesture, as Pierre's line numbers).
    pub(crate) fn press_line(&mut self, file: usize, at: LineRef, shift: bool) {
        let event = LineEvent::Press { at: DiffLine { file, at }, column: Column::Numbers, shift };
        let row_of = &self.row_of;
        let _ = self.lines.step(event, |p| row_of.get(&(p.file as u32, p.at.side, p.at.line)).copied());
    }

    /// The held press reached line `at`; a row without one (a fold) sends nothing.
    pub(crate) fn drag_to(&mut self, file: usize, at: LineRef) {
        let _ = self.lines.step(LineEvent::Move { at: DiffLine { file, at } }, |_| None);
    }

    /// A click on line `at`'s code: it selects that line, or clears it when it was the
    /// selection (`selection-machine.ts`, the `text` column).
    pub(crate) fn click_code(&mut self, file: usize, at: LineRef) {
        let row_of = &self.row_of;
        let _ = self.lines.step(LineEvent::Press { at: DiffLine { file, at }, column: Column::Text, shift: false }, |p| row_of.get(&(p.file as u32, p.at.side, p.at.line)).copied());
        let _ = self.lines.step(LineEvent::Release, |_| None);
    }

    /// The one selected line, while a press on its number waits to see whether it drags.
    pub(crate) fn unselecting(&self) -> Option<LineRange> {
        match self.lines {
            rune_kit::line_selection::LineSelection::Unselecting { line } => Some(LineRange::from(line)),
            _ => None,
        }
    }

    pub(crate) fn release_line(&mut self) -> Option<LineRange> {
        let _ = self.lines.step(LineEvent::Release, |_| None);
        self.selection()
    }

    pub(crate) fn cancel_line(&mut self) {
        let _ = self.lines.step(LineEvent::Cancel, |_| None);
    }

    pub(crate) fn copy_side(&self) -> Side {
        self.copy_side
    }

    pub(crate) fn set_copy_side(&mut self, side: Side) {
        self.copy_side = side;
    }

    pub fn visible_file(&self) -> usize {
        self.visible
    }

    pub fn scroll_to_file(&mut self, file: usize, tx: &mut Tx) {
        let Some(&r) = self.file_rows.get(file) else { return };
        tx.instant();
        self.reveal_at_top(r, tx);
        self.visible = file;
        self.nav = Some(r);
    }

    pub fn scroll_to_line(&mut self, file: usize, at: LineRef, tx: &mut Tx) {
        if self.collapsed(file) {
            self.set_collapsed(file, false);
        }
        if !self.row_of.contains_key(&(file as u32, at.side, at.line))
            && let Some(gap) = self.gaps.get(file).and_then(|gs| {
                gs.iter().position(|g| {
                    let first = if at.side == Side::Old { g.old } else { g.new };
                    at.line >= first && g.len.is_none_or(|n| at.line < first + n)
                })
            })
        {
            let _ = self.expand(file, gap, Expand::All);
        }
        let Some(&r) = self.row_of.get(&(file as u32, at.side, at.line)) else { return };
        tx.instant();
        self.reveal_below_header(r, tx);
        self.visible = file;
        self.nav = Some(r);
    }

    pub fn next_hunk(&mut self, forward: bool, tx: &mut Tx) -> bool {
        let from = self.position();
        let target = if forward {
            self.hunk_rows.iter().copied().find(|&r| r > from)
        } else {
            self.hunk_rows.iter().rev().copied().find(|&r| r < from)
        };
        let Some(r) = target else { return false };
        tx.instant();
        self.reveal_below_header(r, tx);
        self.visible = self.rows[r as usize].file();
        self.nav = Some(r);
        true
    }

    pub fn next_file(&mut self, forward: bool, tx: &mut Tx) -> bool {
        if self.files.is_empty() {
            return false;
        }
        let at = self.visible.min(self.files.len() - 1);
        let len = self.files.len();
        let target = if forward {
            (at + 1) % len
        } else {
            (at + len - 1) % len
        };
        self.scroll_to_file(target, tx);
        true
    }

    fn position(&self) -> u32 {
        if let Some(n) = self.nav
            && self.visible_rows.contains(&(n as usize))
        {
            return n;
        }
        let top = self.row_below_header();
        match (self.nav, top) {
            (_, Some(t)) => t,
            (Some(n), None) => n,
            (None, None) => 0,
        }
    }

    fn row_below_header(&self) -> Option<u32> {
        if self.visible_rows.is_empty() {
            return None;
        }
        let mut r = self.visible_rows.start;
        if self.sticky().is_some() {
            let mut room = 0.0;
            while r + 1 < self.rows.len() && room < HEADER_H {
                room += self.estimate(r);
                r += 1;
            }
        }
        Some(r as u32)
    }

    /// Whether row `r` is a line of code (not a file's header, a note or a fold).
    pub(super) fn is_line(&self, r: usize) -> bool {
        matches!(self.rows.get(r), Some(DiffRow::Line { .. } | DiffRow::Ctx { .. } | DiffRow::Pair { .. }))
    }

    /// The line row that is the diff's Tab stop: the cursor, else the first line.
    pub(super) fn cursor_row(&self) -> Option<usize> {
        self.cursor.map(|c| c as usize).filter(|&c| self.is_line(c)).or_else(|| (0..self.rows.len()).find(|&r| self.is_line(r)))
    }

    /// Up, Down, Home, End, Page Up and Page Down with focus on a line: the cursor moves to
    /// the next line (past headers and folds), takes focus and comes into view. True when the
    /// key was the cursor's.
    pub fn line_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.shift {
            return false;
        }
        let Some(at) = tx.focused().and_then(|k| self.keys.iter().position(|x| x == k)).filter(|&r| self.is_line(r)) else { return false };
        let lines = |range: Box<dyn Iterator<Item = usize>>| range.filter(|&r| self.is_line(r)).collect::<Vec<_>>();
        let to = match ev.key {
            Key::Down => (at + 1..self.rows.len()).find(|&r| self.is_line(r)),
            Key::Up => (0..at).rev().find(|&r| self.is_line(r)),
            Key::Home => (0..self.rows.len()).find(|&r| self.is_line(r)),
            Key::End => (0..self.rows.len()).rev().find(|&r| self.is_line(r)),
            Key::PageDown => lines(Box::new(at + 1..self.rows.len())).into_iter().take(20).last(),
            Key::PageUp => lines(Box::new((0..at).rev())).into_iter().take(20).last(),
            _ => return false,
        };
        if let Some(to) = to {
            tx.instant();
            self.cursor = Some(to as u32);
            tx.scroll_into_view(self.keys[to].clone());
            tx.focus(self.keys[to].clone(), true);
        }
        true
    }

    pub fn handle_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        match &ev.key {
            _ if tx.focused().is_some() && !crate::keymap::Bind::CopySelection.matches_tx(ev, tx) => false,
            Key::Left if !ev.mods.cmd && !ev.mods.ctrl && !ev.mods.alt && !ev.mods.shift => {
                self.next_file(false, tx)
            }
            Key::Right if !ev.mods.cmd && !ev.mods.ctrl && !ev.mods.alt && !ev.mods.shift => {
                self.next_file(true, tx)
            }
            Key::Text(t) if !ev.mods.cmd && !ev.mods.ctrl && !ev.mods.alt => match t.as_str() {
                "j" => self.next_hunk(true, tx),
                "k" => self.next_hunk(false, tx),
                "]" => self.next_file(true, tx),
                "[" => self.next_file(false, tx),
                _ => false,
            },
            Key::Escape if self.selection().is_some() => {
                tx.instant();
                self.set_selection(None);
                true
            }
            _ if crate::keymap::Bind::CopySelection.matches_tx(ev, tx) => match self.selected_text() {
                Some(text) => {
                    tx.copy(text);
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    pub(crate) fn reveal_at_top(&self, r: u32, tx: &mut Tx) {
        if let Some(last) = self.keys.last() {
            tx.scroll_into_view(last.clone());
        }
        if let Some(k) = self.keys.get(r as usize) {
            tx.scroll_into_view(k.clone());
        }
    }

    pub(crate) fn reveal_below_header(&self, r: u32, tx: &mut Tx) {
        let mut top = r as usize;
        if !self.hide_file_headers {
            let mut room = 0.0;
            while top > 0 && room < HEADER_H {
                if matches!(self.rows[top], DiffRow::File(_)) {
                    break;
                }
                top -= 1;
                room += self.estimate(top);
            }
        }
        self.reveal_at_top(top as u32, tx);
    }
}

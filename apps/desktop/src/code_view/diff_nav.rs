//! Selection, scroll reveal, and keyboard navigation (`j`/`k`, `]`/`[`, `←`/`→`, `⌘C`) for [`DiffDocState`].

use rune::{Key, KeyEvent, Modifier, Tx};
use rune_kit::diff::{Expand, LineRange, LineRef, Side};

use super::HEADER_H;
use super::diff_state::{DiffDocState, DiffRow};

impl DiffDocState {
    pub fn selection(&self) -> Option<LineRange> {
        self.selection
    }

    pub fn set_selection(&mut self, sel: Option<LineRange>) {
        self.selection = sel;
    }

    pub fn selected_text(&self) -> Option<String> {
        let sel = self.selection?;
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

    pub(crate) fn row_selected(&self, r: usize, side: Option<Side>) -> bool {
        let Some(sel) = self.selection else { return false };
        let Some((a, b)) = self.selected_rows(&sel) else { return false };
        if self.rows[r].file() != sel.file || !(a as usize..=b as usize).contains(&r) {
            return false;
        }
        side.is_none_or(|s| s == sel.start.side || s == sel.end.side)
    }

    pub(crate) fn press_line(&mut self, file: usize, at: LineRef) {
        self.selection_at_press = self.selection;
        let sel = match self.selection {
            Some(s) if self.shift && s.file == file => LineRange { end: at, ..s },
            _ => LineRange::line(file, at),
        };
        self.unselect = !self.shift && self.selection == Some(sel);
        self.selecting = true;
        self.selection = Some(sel);
    }

    pub(crate) fn drag_to(&mut self, file: usize, at: LineRef) {
        if let Some(s) = self.selection.filter(|s| self.selecting && s.file == file && s.end != at)
        {
            self.unselect = false;
            self.selection = Some(LineRange { end: at, ..s });
        }
    }

    pub(crate) fn release_line(&mut self) -> Option<LineRange> {
        self.selection_at_press = None;
        if std::mem::take(&mut self.unselect) {
            self.selection = None;
        }
        self.selecting = false;
        self.selection
    }

    pub(crate) fn cancel_line(&mut self) {
        if self.selecting {
            self.selection = self.selection_at_press.take();
        }
        self.selecting = false;
        self.unselect = false;
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

    pub fn handle_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        match &ev.key {
            Key::Modifier { modifier: Modifier::ShiftLeft | Modifier::ShiftRight, .. } => {
                self.shift = ev.mods.shift;
                false
            }
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
            Key::Escape if self.selection.is_some() => {
                tx.instant();
                self.selection = None;
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

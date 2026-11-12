//! Finding text in a terminal's screen and scrollback, on libghostty-vt's own search
//! (`vt/search.h`): it follows the live screens through output, resizes, reflow, the alternate
//! screen and scrollback pruning, so the worker only sets the needle, moves the selected match
//! and reads the matches in view with each snapshot.

use std::ffi::c_void;

use crate::core::VtCore;
use crate::ffi::{self, *};
use crate::types::{FindHit, FindView};

impl VtCore {
    /// Starts finding `needle` (ASCII letters match either case), or stops with `None` or "".
    pub fn find(&mut self, needle: Option<&str>) {
        self.mutated_since_snapshot = true;
        let Some(needle) = needle.filter(|n| !n.is_empty()) else {
            if !self.search.is_null() {
                unsafe { ffi::ghostty_search_set(self.search, GHOSTTY_SEARCH_OPT_NEEDLE, std::ptr::null()) };
            }
            self.finding = false;
            return;
        };
        if self.search.is_null()
            && unsafe { ffi::ghostty_search_new(std::ptr::null(), &mut self.search, self.term) } != GHOSTTY_SUCCESS
        {
            self.search = std::ptr::null_mut();
            return;
        }
        let s = GhosttyString { ptr: needle.as_ptr(), len: needle.len() };
        unsafe { ffi::ghostty_search_set(self.search, GHOSTTY_SEARCH_OPT_NEEDLE, (&s as *const GhosttyString).cast()) };
        self.finding = true;
        // The newest match is the one to show first, as a find bar opened at the prompt does.
        self.find_step(true);
    }

    /// Selects the next match toward older output (`older`) or toward newer, wrapping around,
    /// and scrolls it into view.
    pub fn find_step(&mut self, older: bool) {
        if !self.finding {
            return;
        }
        let opt = if older { GHOSTTY_SEARCH_OPT_SELECT_NEXT } else { GHOSTTY_SEARCH_OPT_SELECT_PREV };
        unsafe { ffi::ghostty_search_set(self.search, opt, std::ptr::null()) };
        self.mutated_since_snapshot = true;
    }

    /// What the find bar and the highlights show, caught up with the terminal; `None` while not
    /// finding.
    pub(crate) fn find_view(&mut self, cols: u16, rows: u16) -> Option<FindView> {
        if !self.finding {
            return None;
        }
        unsafe { ffi::ghostty_search_run(self.search) };
        let mut total = 0usize;
        let mut index = 0usize;
        let mut selected = GhosttySelection::default();
        let (has_index, has_selected) = unsafe {
            ffi::ghostty_search_get(self.search, GHOSTTY_SEARCH_DATA_TOTAL_MATCHES, (&mut total as *mut usize).cast());
            (
                ffi::ghostty_search_get(self.search, GHOSTTY_SEARCH_DATA_SELECTED_INDEX, (&mut index as *mut usize).cast()) == GHOSTTY_SUCCESS,
                ffi::ghostty_search_get(self.search, GHOSTTY_SEARCH_DATA_SELECTED_MATCH, (&mut selected as *mut GhosttySelection).cast())
                    == GHOSTTY_SUCCESS,
            )
        };
        let mut buf = GhosttySelectionBuffer { ptr: std::ptr::null_mut(), cap: 0, len: 0 };
        let mut matches = Vec::new();
        let rc = unsafe { ffi::ghostty_search_get(self.search, GHOSTTY_SEARCH_DATA_VIEWPORT_MATCHES, (&mut buf as *mut GhosttySelectionBuffer).cast::<c_void>()) };
        if rc == GHOSTTY_OUT_OF_SPACE && buf.len > 0 {
            matches = vec![GhosttySelection::default(); buf.len];
            buf = GhosttySelectionBuffer { ptr: matches.as_mut_ptr(), cap: matches.len(), len: 0 };
            if unsafe { ffi::ghostty_search_get(self.search, GHOSTTY_SEARCH_DATA_VIEWPORT_MATCHES, (&mut buf as *mut GhosttySelectionBuffer).cast::<c_void>()) } == GHOSTTY_SUCCESS {
                matches.truncate(buf.len);
            } else {
                matches.clear();
            }
        }
        let point = |r: &GhosttyGridRef| {
            let mut out = GhosttyPointCoordinate::default();
            (unsafe { ffi::ghostty_terminal_point_from_grid_ref(self.term, r, GHOSTTY_POINT_TAG_VIEWPORT, &mut out) } == GHOSTTY_SUCCESS).then_some(out)
        };
        let chosen = has_selected.then(|| (point(&selected.start), point(&selected.end))).filter(|(a, b)| a.is_some() && b.is_some());
        let key = |p: Option<GhosttyPointCoordinate>| p.map(|p| (p.x, p.y));
        let mut hits = Vec::new();
        for m in &matches {
            let (Some(a), Some(b)) = (point(&m.start), point(&m.end)) else { continue };
            let is_selected = chosen.is_some_and(|(s, e)| key(s) == Some((a.x, a.y)) && key(e) == Some((b.x, b.y)));
            for y in a.y..=b.y.min(u32::from(rows).saturating_sub(1)) {
                let start = if y == a.y { a.x } else { 0 };
                let end = if y == b.y { b.x } else { cols.saturating_sub(1) };
                hits.push(FindHit { row: y as u16, start, end, selected: is_selected });
            }
        }
        Some(FindView { total, selected: has_index.then_some(index), hits })
    }
}

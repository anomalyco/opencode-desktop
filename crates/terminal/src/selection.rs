//! Selection operations and selection text/HTML formatting for [`VtCore`].

use std::ffi::c_int;

use crate::core::VtCore;
use crate::ffi::{self, *};
use crate::types::{GridPoint, SelectionOp};

struct SelectionPoints {
    start: (u16, u32),
    end: (u16, u32),
    rectangular: bool,
}

impl VtCore {
    pub fn select(&mut self, op: SelectionOp) {
        self.selection_changed = true;
        match op {
            SelectionOp::Clear => {
                // SAFETY: `self.term` was created in `VtCore::new` and is freed only in `Drop`;
                // `NULL` clears the active selection for `GHOSTTY_TERMINAL_OPT_SELECTION`.
                unsafe {
                    let _ = ffi::ghostty_terminal_set(
                        self.term,
                        GHOSTTY_TERMINAL_OPT_SELECTION,
                        std::ptr::null(),
                    );
                }
                self.has_selection = false;
            }
            SelectionOp::All => {
                let mut sel = GhosttySelection::default();
                // SAFETY: `self.term` is live and `sel` has `size` initialized via `Default`.
                if unsafe { ffi::ghostty_terminal_select_all(self.term, &mut sel) }
                    == GHOSTTY_SUCCESS
                {
                    self.apply_selection(&sel);
                }
            }
            SelectionOp::Word(pt) => {
                const WORD_BOUNDARIES: &[u32] = &[
                    ' ' as u32, '\t' as u32, '\'' as u32, '"' as u32, ';' as u32, ':' as u32,
                    ',' as u32, '(' as u32, ')' as u32, '[' as u32, ']' as u32, '{' as u32,
                    '}' as u32, '<' as u32, '>' as u32, '|' as u32, '&' as u32, '!' as u32,
                    '?' as u32, '`' as u32, '$' as u32,
                ];
                let mut gref = GhosttyGridRef::default();
                // SAFETY: `self.term` is live and `gref` has `size` initialized via `Default`.
                if unsafe { ffi::ghostty_terminal_grid_ref(self.term, pt.to_ffi(), &mut gref) }
                    == GHOSTTY_SUCCESS
                {
                    let opts = GhosttyTerminalSelectWordOptions {
                        size: std::mem::size_of::<GhosttyTerminalSelectWordOptions>(),
                        grid_ref: gref,
                        boundary_codepoints: WORD_BOUNDARIES.as_ptr(),
                        boundary_codepoints_len: WORD_BOUNDARIES.len(),
                    };
                    let mut sel = GhosttySelection::default();
                    // SAFETY: `self.term` is live, `opts.grid_ref` was just produced by
                    // `self.term` with no intervening mutation, `WORD_BOUNDARIES` is valid for
                    // `WORD_BOUNDARIES.len()` `u32` elements, and `opts`/`sel` have `size` set.
                    if unsafe { ffi::ghostty_terminal_select_word(self.term, &opts, &mut sel) }
                        == GHOSTTY_SUCCESS
                    {
                        self.apply_selection(&sel);
                    }
                }
            }
            SelectionOp::Line(pt) => {
                let mut gref = GhosttyGridRef::default();
                // SAFETY: `self.term` is live and `gref` has `size` initialized via `Default`.
                if unsafe { ffi::ghostty_terminal_grid_ref(self.term, pt.to_ffi(), &mut gref) }
                    == GHOSTTY_SUCCESS
                {
                    let opts = GhosttyTerminalSelectLineOptions {
                        size: std::mem::size_of::<GhosttyTerminalSelectLineOptions>(),
                        grid_ref: gref,
                        whitespace: std::ptr::null(),
                        whitespace_len: 0,
                        semantic_prompt_boundary: false,
                    };
                    let mut sel = GhosttySelection::default();
                    // SAFETY: `self.term` is live, `opts.grid_ref` was just produced by
                    // `self.term` with no intervening mutation, `whitespace == NULL` with
                    // `whitespace_len == 0` selects Ghostty's defaults, and `opts`/`sel` are sized.
                    if unsafe { ffi::ghostty_terminal_select_line(self.term, &opts, &mut sel) }
                        == GHOSTTY_SUCCESS
                    {
                        self.apply_selection(&sel);
                    }
                }
            }
            SelectionOp::Range {
                start,
                end,
                rectangular,
            } => {
                let mut sref = GhosttyGridRef::default();
                let mut eref = GhosttyGridRef::default();
                // SAFETY: `self.term` is live and `sref` has `size` initialized via `Default`.
                let ok_s =
                    unsafe { ffi::ghostty_terminal_grid_ref(self.term, start.to_ffi(), &mut sref) }
                        == GHOSTTY_SUCCESS;
                // SAFETY: `self.term` is live and `eref` has `size` initialized via `Default`.
                let ok_e =
                    unsafe { ffi::ghostty_terminal_grid_ref(self.term, end.to_ffi(), &mut eref) }
                        == GHOSTTY_SUCCESS;
                if ok_s && ok_e {
                    let sel = GhosttySelection {
                        size: std::mem::size_of::<GhosttySelection>(),
                        start: sref,
                        end: eref,
                        rectangle: rectangular,
                    };
                    self.apply_selection(&sel);
                }
            }
            SelectionOp::Extend(to) => {
                let (Some(sel), Some(p)) = (self.selection_points(), self.screen_point(to)) else { return };
                let at = |q: (u16, u32)| (q.1, q.0);
                let (first, last) = if at(sel.start) <= at(sel.end) { (sel.start, sel.end) } else { (sel.end, sel.start) };
                let anchor = if at(p) < at(first) { last } else { first };
                self.select(SelectionOp::Range {
                    start: GridPoint::Screen { col: anchor.0, row: anchor.1 },
                    end: GridPoint::Screen { col: p.0, row: p.1 },
                    rectangular: sel.rectangular,
                });
            }
            SelectionOp::Nudge { dx, dy } => {
                let Some(sel) = self.selection_points() else { return };
                let (mut cols, mut bar) = (0u16, GhosttyTerminalScrollbar::default());
                // SAFETY: `self.term` is live and each out-pointer matches its key's C type
                // (`uint16_t*`, `GhosttyTerminalScrollbar*`).
                unsafe {
                    let _ = ffi::ghostty_terminal_get(self.term, GHOSTTY_TERMINAL_DATA_COLS, (&mut cols as *mut u16).cast());
                    let _ = ffi::ghostty_terminal_get(self.term, GHOSTTY_TERMINAL_DATA_SCROLLBAR, (&mut bar as *mut GhosttyTerminalScrollbar).cast());
                }
                let (cols, total) = (i64::from(cols.max(1)), (bar.total as i64).max(1));
                let cell = (i64::from(sel.end.1) * cols + i64::from(sel.end.0) + i64::from(dx) + i64::from(dy) * cols).clamp(0, total * cols - 1);
                let end = GridPoint::Screen { col: (cell % cols) as u16, row: (cell / cols) as u32 };
                self.select(SelectionOp::Range {
                    start: GridPoint::Screen { col: sel.start.0, row: sel.start.1 },
                    end,
                    rectangular: sel.rectangular,
                });
            }
        }
    }

    /// The selection's two ends as (column, row from the top of the scrollback), and whether
    /// it's a block.
    fn selection_points(&self) -> Option<SelectionPoints> {
        if !self.has_selection {
            return None;
        }
        let mut sel = GhosttySelection::default();
        // SAFETY: `self.term` is live and `sel` has `size` initialized via `Default` for
        // `GHOSTTY_TERMINAL_DATA_SELECTION`.
        if unsafe { ffi::ghostty_terminal_get(self.term, GHOSTTY_TERMINAL_DATA_SELECTION, (&mut sel as *mut GhosttySelection).cast()) } != GHOSTTY_SUCCESS {
            return None;
        }
        Some(SelectionPoints {
            start: self.ref_point(&sel.start)?,
            end: self.ref_point(&sel.end)?,
            rectangular: sel.rectangle,
        })
    }

    fn screen_point(&self, p: GridPoint) -> Option<(u16, u32)> {
        let mut gref = GhosttyGridRef::default();
        // SAFETY: `self.term` is live and `gref` has `size` initialized via `Default`.
        (unsafe { ffi::ghostty_terminal_grid_ref(self.term, p.to_ffi(), &mut gref) } == GHOSTTY_SUCCESS).then_some(())?;
        self.ref_point(&gref)
    }

    fn ref_point(&self, r: &GhosttyGridRef) -> Option<(u16, u32)> {
        let mut c = GhosttyPointCoordinate::default();
        // SAFETY: `self.term` is live, `r` is a fresh untracked `GhosttyGridRef` obtained from
        // `self.term` with no intervening mutation, and `&mut c` is a valid out-pointer.
        (unsafe { ffi::ghostty_terminal_point_from_grid_ref(self.term, r, GHOSTTY_POINT_TAG_SCREEN, &mut c) } == GHOSTTY_SUCCESS).then_some((c.x, c.y))
    }

    fn apply_selection(&mut self, sel: &GhosttySelection) {
        // SAFETY: `self.term` is live, `sel` contains fresh untracked grid refs from `self.term`
        // with no intervening mutation, and `ghostty_terminal_set` copies them immediately into
        // tracked state.
        unsafe {
            let _ = ffi::ghostty_terminal_set(
                self.term,
                GHOSTTY_TERMINAL_OPT_SELECTION,
                (sel as *const GhosttySelection).cast(),
            );
        }
        self.has_selection = true;
    }

    pub fn selected_text(&self) -> Option<String> {
        self.selection_as(GHOSTTY_FORMATTER_FORMAT_PLAIN)
    }

    /// The selection as HTML, its colours and styles inline, for pasting into documents.
    pub fn selected_html(&self) -> Option<String> {
        self.selection_as(GHOSTTY_FORMATTER_FORMAT_HTML)
    }

    fn selection_as(&self, emit: c_int) -> Option<String> {
        if !self.has_selection {
            return None;
        }
        let mut sel = GhosttySelection::default();
        // SAFETY: `self.term` is live and `sel` has `size` initialized via `Default` for
        // `GHOSTTY_TERMINAL_DATA_SELECTION`.
        if unsafe {
            ffi::ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_SELECTION,
                (&mut sel as *mut GhosttySelection).cast(),
            )
        } != GHOSTTY_SUCCESS
        {
            return None;
        }
        let opts = GhosttyTerminalSelectionFormatOptions {
            size: std::mem::size_of::<GhosttyTerminalSelectionFormatOptions>(),
            emit,
            // A line the terminal wrapped copies as the one line the program wrote.
            unwrap: true,
            trim: true,
            selection: &sel,
        };
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let mut len = 0usize;
        // SAFETY: `self.term` is live, `opts.selection` points to the fresh stack `sel` with no
        // intervening mutation, `NULL` selects the default allocator, and `&mut ptr`/`&mut len`
        // are valid out-pointers.
        if unsafe {
            ffi::ghostty_terminal_selection_format_alloc(
                self.term,
                std::ptr::null(),
                opts,
                &mut ptr,
                &mut len,
            )
        } != GHOSTTY_SUCCESS
            || ptr.is_null()
        {
            return None;
        }
        // SAFETY: `ghostty_terminal_selection_format_alloc` succeeded with non-null `ptr`, so
        // `ptr` points to `len` initialized bytes allocated with the default allocator (`NULL`);
        // we copy them before releasing once via `ghostty_free(NULL, ptr, len)`.
        let text = unsafe {
            let s = String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).into_owned();
            ffi::ghostty_free(std::ptr::null(), ptr, len);
            s
        };
        (!text.is_empty()).then_some(text)
    }
}

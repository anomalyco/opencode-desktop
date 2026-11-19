//! Incremental `GhosttyRenderState` viewport extraction and `RowId`-keyed row cache for [`VtCore`].

use std::ffi::c_int;
use std::sync::Arc;

use crate::core::VtCore;
use crate::ffi::{self, *};
use crate::theme::Rgb;
use crate::grapheme::Grapheme;
use crate::types::*;

const ROW_CACHE_CAP: usize = 512;

#[repr(C)]
#[derive(Clone, Copy)]
struct RowSelectionRaw {
    size: usize,
    start_x: u16,
    end_x: u16,
}

impl VtCore {
    /// Updates `GhosttyRenderState` and produces an immutable [`TerminalSnapshot`].
    /// Clean rows (`dirty == false`) and scrolled scrollback rows reuse their `Arc<TerminalRow>`.
    pub fn snapshot(&mut self) -> Arc<TerminalSnapshot> {
        // SAFETY: `self.render_state` and `self.term` were created in `VtCore::new` and are freed
        // only in `Drop`; `&mut self` ensures exclusive access during the update.
        unsafe {
            let _ = ffi::ghostty_render_state_update(self.render_state, self.term);
        }

        let mut cols: u16 = 0;
        let mut rows: u16 = 0;
        let mut colors = GhosttyRenderStateColors::default();
        let mut cursor_raw = GhosttyRenderStateCursor::default();
        let mut scrollbar_raw = GhosttyTerminalScrollbar::default();
        let mut viewport_active = true;
        let mut active_screen: c_int = GHOSTTY_TERMINAL_SCREEN_PRIMARY;
        let mut mouse_tracking = false;
        let mut kitty_flags: u8 = 0;

        // SAFETY: `self.render_state`, `self.row_iter`, and `self.term` were created in
        // `VtCore::new` and are freed only in `Drop`; every out-pointer matches its key's C type
        // (`colors` and `cursor_raw` have their `size` fields set via `Default`).
        unsafe {
            let _ = ffi::ghostty_render_state_get(
                self.render_state,
                GHOSTTY_RENDER_STATE_DATA_COLS,
                (&mut cols as *mut u16).cast(),
            );
            let _ = ffi::ghostty_render_state_get(
                self.render_state,
                GHOSTTY_RENDER_STATE_DATA_ROWS,
                (&mut rows as *mut u16).cast(),
            );
            let _ = ffi::ghostty_render_state_get(
                self.render_state,
                GHOSTTY_RENDER_STATE_DATA_COLORS,
                (&mut colors as *mut GhosttyRenderStateColors).cast(),
            );
            let _ = ffi::ghostty_render_state_get(
                self.render_state,
                GHOSTTY_RENDER_STATE_DATA_CURSOR,
                (&mut cursor_raw as *mut GhosttyRenderStateCursor).cast(),
            );
            let _ = ffi::ghostty_render_state_get(
                self.render_state,
                GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                (&mut self.row_iter as *mut GhosttyRenderStateRowIterator).cast(),
            );

            let _ = ffi::ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_SCROLLBAR,
                (&mut scrollbar_raw as *mut GhosttyTerminalScrollbar).cast(),
            );
            let _ = ffi::ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_VIEWPORT_ACTIVE,
                (&mut viewport_active as *mut bool).cast(),
            );
            let _ = ffi::ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN,
                (&mut active_screen as *mut c_int).cast(),
            );
            let _ = ffi::ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING,
                (&mut mouse_tracking as *mut bool).cast(),
            );
            let _ = ffi::ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS,
                (&mut kitty_flags as *mut u8).cast(),
            );
        }

        let scroll_only = !self.mutated_since_snapshot && !self.selection_changed;
        let default_fg = Rgb::from_ffi(colors.foreground);
        let mut screen_rows = Vec::with_capacity(usize::from(rows));

        let mut row_idx = 0usize;
        // SAFETY: `self.row_iter` was populated above from `self.render_state` with no
        // intervening render-state update.
        while unsafe { ffi::ghostty_render_state_row_iterator_next(self.row_iter) } {
            let mut row_id = GhosttyRenderStateRowId::default();
            let mut row_dirty = true;
            // SAFETY: `self.row_iter` is positioned on a row (`next` returned `true`) and each
            // out-pointer matches its key's C type (`GhosttyRenderStateRowId*`, `bool*`).
            unsafe {
                let _ = ffi::ghostty_render_state_row_get(
                    self.row_iter,
                    GHOSTTY_RENDER_STATE_ROW_DATA_ID,
                    (&mut row_id as *mut GhosttyRenderStateRowId).cast(),
                );
                let _ = ffi::ghostty_render_state_row_get(
                    self.row_iter,
                    GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY,
                    (&mut row_dirty as *mut bool).cast(),
                );
            }
            let id = row_id.as_u128();

            let reused = if !row_dirty {
                self.last_viewport_rows
                    .get(row_idx)
                    .filter(|r| r.id == id)
                    .cloned()
                    .or_else(|| self.row_cache.get(&id).cloned())
            } else if scroll_only {
                self.row_cache.get(&id).cloned()
            } else {
                None
            };

            let arc_row = match reused {
                Some(r) => r,
                None => {
                    let built = Arc::new(self.extract_current_row(id, cols, &colors, default_fg));
                    if !self.has_selection {
                        self.cache_row(id, built.clone());
                    }
                    built
                }
            };
            screen_rows.push(arc_row);
            row_idx += 1;
        }

        // SAFETY: `self.render_state` was created in `VtCore::new` and is freed only in `Drop`.
        unsafe {
            let _ = ffi::ghostty_render_state_clean(self.render_state);
        }

        self.last_viewport_rows.clone_from(&screen_rows);
        self.mutated_since_snapshot = false;
        self.selection_changed = false;

        let find = self.find_view(cols, rows);
        let marks = screen_rows.iter().enumerate().filter_map(|(i, r)| self.marks.on_row(r.id, r.content_hash).map(|m| (i as u16, m))).collect();
        let cursor_color = if colors.cursor_has_value {
            Rgb::from_ffi(colors.cursor)
        } else {
            self.theme.cursor
        };

        Arc::new(TerminalSnapshot {
            cols,
            rows,
            screen_rows,
            cursor: CursorSnapshot {
                in_viewport: cursor_raw.viewport_has_value,
                col: cursor_raw.viewport_x,
                row: cursor_raw.viewport_y,
                wide_tail: cursor_raw.wide_tail,
                visible: cursor_raw.visible,
                blinking: cursor_raw.blinking,
                password_input: cursor_raw.password_input,
                shape: CursorShape::from_render_style(cursor_raw.visual_style),
            },
            scrollbar: ScrollbarSnapshot {
                total: scrollbar_raw.total,
                offset: scrollbar_raw.offset,
                len: scrollbar_raw.len,
                at_bottom: viewport_active,
            },
            fg: default_fg,
            bg: Rgb::from_ffi(colors.background),
            cursor_color,
            selection_alpha: self.theme.selection_alpha,
            alt_screen: active_screen == GHOSTTY_TERMINAL_SCREEN_ALTERNATE,
            mouse_tracking,
            bracketed_paste: self.mode_enabled(GHOSTTY_MODE_BRACKETED_PASTE),
            focus_reporting: self.mode_enabled(GHOSTTY_MODE_FOCUS_EVENT),
            kitty_keyboard_flags: kitty_flags,
            title: self.title.clone(),
            pwd: self.pwd.clone(),
            bell_count: self.cb.bell_count,
            find,
            marks,
        })
    }

    fn cache_row(&mut self, id: u128, row: Arc<TerminalRow>) {
        if !self.row_cache.contains_key(&id) {
            if self.row_lru.len() >= ROW_CACHE_CAP
                && let Some(evicted) = self.row_lru.pop_front()
            {
                self.row_cache.remove(&evicted);
            }
            self.row_lru.push_back(id);
        }
        self.row_cache.insert(id, row);
    }

    fn extract_current_row(
        &mut self,
        id: u128,
        cols: u16,
        colors: &GhosttyRenderStateColors,
        default_fg: Rgb,
    ) -> TerminalRow {
        let mut raw_row: GhosttyRow = 0;
        let mut wrapped = false;
        let mut sel_raw = RowSelectionRaw {
            size: std::mem::size_of::<RowSelectionRaw>(),
            start_x: 0,
            end_x: 0,
        };
        // SAFETY: `extract_current_row` is called only while `self.row_iter` is positioned on a
        // row of the current render state; `self.row_cells` is pre-allocated in `VtCore::new`,
        // `sel_raw.size` is initialized, and every out-pointer matches its key's C type.
        let has_row_sel = unsafe {
            let _ = ffi::ghostty_render_state_row_get(
                self.row_iter,
                GHOSTTY_RENDER_STATE_ROW_DATA_RAW,
                (&mut raw_row as *mut GhosttyRow).cast(),
            );
            let _ = ffi::ghostty_row_get(
                raw_row,
                GHOSTTY_ROW_DATA_WRAP,
                (&mut wrapped as *mut bool).cast(),
            );
            let _ = ffi::ghostty_render_state_row_get(
                self.row_iter,
                GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                (&mut self.row_cells as *mut GhosttyRenderStateRowCells).cast(),
            );
            ffi::ghostty_render_state_row_get(
                self.row_iter,
                GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION,
                (&mut sel_raw as *mut RowSelectionRaw).cast(),
            ) == GHOSTTY_SUCCESS
        };

        let mut cells = Vec::with_capacity(usize::from(cols));
        let mut bg_spans: Vec<BgSpan> = Vec::new();
        let mut text = String::with_capacity(usize::from(cols));
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mix = |h: &mut u64, v: u64| {
            *h = (*h ^ v).wrapping_mul(0x0100_0000_01b3);
        };

        let mut col = 0u16;
        // SAFETY: `self.row_cells` was populated above from `self.row_iter` for the current row
        // with no intervening render-state update.
        while unsafe { ffi::ghostty_render_state_row_cells_next(self.row_cells) } {
            let selected = has_row_sel && col >= sel_raw.start_x && col <= sel_raw.end_x;
            let mut raw_cell: GhosttyCell = 0;
            let mut wide_tag: c_int = GHOSTTY_CELL_WIDE_NARROW;
            let mut has_hyperlink = false;
            let mut grapheme_len: u32 = 0;
            let mut has_styling = false;
            let mut bg_rgb = GhosttyColorRgb::default();
            let mut fg_rgb = GhosttyColorRgb::default();

            // SAFETY: `self.row_cells` is positioned on a cell (`next` returned `true`) and every
            // out-pointer matches its key's C type (`GhosttyCell*`, `c_int*`, `bool*`, `u32*`,
            // `GhosttyColorRgb*`).
            let (has_bg, has_fg) = unsafe {
                let _ = ffi::ghostty_render_state_row_cells_get(
                    self.row_cells,
                    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
                    (&mut raw_cell as *mut GhosttyCell).cast(),
                );
                let _ = ffi::ghostty_cell_get(
                    raw_cell,
                    GHOSTTY_CELL_DATA_WIDE,
                    (&mut wide_tag as *mut c_int).cast(),
                );
                let _ = ffi::ghostty_cell_get(
                    raw_cell,
                    GHOSTTY_CELL_DATA_HAS_HYPERLINK,
                    (&mut has_hyperlink as *mut bool).cast(),
                );
                let _ = ffi::ghostty_render_state_row_cells_get(
                    self.row_cells,
                    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN,
                    (&mut grapheme_len as *mut u32).cast(),
                );
                let _ = ffi::ghostty_render_state_row_cells_get(
                    self.row_cells,
                    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HAS_STYLING,
                    (&mut has_styling as *mut bool).cast(),
                );
                let hb = ffi::ghostty_render_state_row_cells_get(
                    self.row_cells,
                    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
                    (&mut bg_rgb as *mut GhosttyColorRgb).cast(),
                ) == GHOSTTY_SUCCESS;
                let hf = ffi::ghostty_render_state_row_cells_get(
                    self.row_cells,
                    GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
                    (&mut fg_rgb as *mut GhosttyColorRgb).cast(),
                ) == GHOSTTY_SUCCESS;
                (hb, hf)
            };

            let mut flags = CellFlags(0);
            let mut underline = UnderlineKind::None;
            let mut underline_color = None;

            if wide_tag == GHOSTTY_CELL_WIDE_WIDE {
                flags.0 |= CellFlags::WIDE.0;
            } else if wide_tag == GHOSTTY_CELL_WIDE_SPACER_TAIL
                || wide_tag == GHOSTTY_CELL_WIDE_SPACER_HEAD
            {
                flags.0 |= CellFlags::WIDE_SPACER_TAIL.0;
            }
            if has_hyperlink {
                flags.0 |= CellFlags::HYPERLINK.0;
            }

            if has_styling {
                let mut style = GhosttyStyle::default();
                // SAFETY: `self.row_cells` is positioned on a cell and `style` has its `size`
                // field set via `Default` for `GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE`.
                unsafe {
                    let _ = ffi::ghostty_render_state_row_cells_get(
                        self.row_cells,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                        (&mut style as *mut GhosttyStyle).cast(),
                    );
                }
                if style.bold {
                    flags.0 |= CellFlags::BOLD.0;
                }
                if style.italic {
                    flags.0 |= CellFlags::ITALIC.0;
                }
                if style.faint {
                    flags.0 |= CellFlags::FAINT.0;
                }
                if style.blink {
                    flags.0 |= CellFlags::BLINK.0;
                }
                if style.inverse {
                    flags.0 |= CellFlags::INVERSE.0;
                }
                if style.invisible {
                    flags.0 |= CellFlags::INVISIBLE.0;
                }
                if style.strikethrough {
                    flags.0 |= CellFlags::STRIKETHROUGH.0;
                }
                if style.overline {
                    flags.0 |= CellFlags::OVERLINE.0;
                }
                underline = UnderlineKind::from_ffi(style.underline);
                underline_color = resolve_style_color(style.underline_color, colors);
            }

            let mut fg = if has_fg { Rgb::from_ffi(fg_rgb) } else { default_fg };
            let mut bg = has_bg.then(|| Rgb::from_ffi(bg_rgb));
            if flags.contains(CellFlags::INVERSE) {
                let resolved_bg = bg.unwrap_or_else(|| Rgb::from_ffi(colors.background));
                bg = Some(fg);
                fg = resolved_bg;
            }

            // The cell's text straight as UTF-8, inline for nearly every cell (`Grapheme`).
            let grapheme = if grapheme_len > 0 && !flags.contains(CellFlags::WIDE_SPACER_TAIL) {
                let mut small = [0u8; 15];
                let mut buf = GhosttyBuffer { ptr: small.as_mut_ptr(), cap: small.len(), len: 0 };
                // SAFETY: `self.row_cells` is positioned on a cell and `buf.ptr` points to
                // `buf.cap` writable bytes on the stack (`small`).
                let mut rc = unsafe {
                    ffi::ghostty_render_state_row_cells_get(
                        self.row_cells,
                        GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
                        (&mut buf as *mut GhosttyBuffer).cast(),
                    )
                };
                let mut big = Vec::new();
                if rc == GHOSTTY_OUT_OF_SPACE {
                    big.resize(buf.len, 0);
                    buf = GhosttyBuffer { ptr: big.as_mut_ptr(), cap: big.len(), len: 0 };
                    // SAFETY: `self.row_cells` is positioned on the same cell and `buf.ptr`
                    // points to `buf.cap` writable bytes owned by `big`.
                    rc = unsafe {
                        ffi::ghostty_render_state_row_cells_get(
                            self.row_cells,
                            GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
                            (&mut buf as *mut GhosttyBuffer).cast(),
                        )
                    };
                }
                let bytes = if big.is_empty() { &small[..buf.len.min(small.len())] } else { &big[..buf.len.min(big.len())] };
                let s = if rc == GHOSTTY_SUCCESS { std::str::from_utf8(bytes).unwrap_or_default() } else { "" };
                for &b in s.as_bytes() {
                    mix(&mut hash, u64::from(b));
                }
                text.push_str(s);
                Grapheme::from(s)
            } else {
                if !flags.contains(CellFlags::WIDE_SPACER_TAIL) {
                    text.push(' ');
                    mix(&mut hash, 0x20);
                }
                Grapheme::EMPTY
            };
            let width = if flags.contains(CellFlags::WIDE) {
                2
            } else if flags.contains(CellFlags::WIDE_SPACER_TAIL) {
                0
            } else {
                1
            };

            mix(
                &mut hash,
                (u64::from(fg.r) << 40)
                    | (u64::from(fg.g) << 32)
                    | (u64::from(fg.b) << 24)
                    | (u64::from(flags.0) << 8)
                    | (u64::from(underline as u8) << 2)
                    | u64::from(selected),
            );
            if let Some(b) = bg {
                mix(&mut hash, (u64::from(b.r) << 16) | (u64::from(b.g) << 8) | u64::from(b.b));
            }

            if bg.is_some() || selected {
                match bg_spans.last_mut() {
                    Some(last)
                        if last.end_col == col && last.color == bg && last.selected == selected =>
                    {
                        last.end_col = col + 1;
                    }
                    _ => bg_spans.push(BgSpan {
                        start_col: col,
                        end_col: col + 1,
                        color: bg,
                        selected,
                    }),
                }
            }

            cells.push(RenderedCell {
                grapheme,
                width,
                fg,
                bg,
                underline_color,
                underline,
                flags,
                selected,
            });
            col = col.saturating_add(1);
        }

        TerminalRow {
            id,
            content_hash: hash,
            wrapped,
            cells,
            bg_spans,
            text,
        }
    }
}

fn resolve_style_color(c: GhosttyStyleColor, colors: &GhosttyRenderStateColors) -> Option<Rgb> {
    // SAFETY: `c` was populated by `libghostty-vt` from a zeroed `GhosttyStyle`, and `c.tag`
    // selects the active variant (`rgb` or `palette`) of the `#[repr(C)]` POD union `c.value`.
    unsafe {
        match c.tag {
            GHOSTTY_STYLE_COLOR_RGB => Some(Rgb::from_ffi(c.value.rgb)),
            GHOSTTY_STYLE_COLOR_PALETTE => {
                Some(Rgb::from_ffi(colors.palette[usize::from(c.value.palette)]))
            }
            _ => None,
        }
    }
}

//! Safe headless `libghostty-vt` emulator (`VtCore`): lifecycle, VT byte feeding, resize reflow,
//! and scrollback. Selection lives in `selection.rs`; input encoding in `input.rs`; render
//! extraction in `render_extract.rs`.

use std::collections::{HashMap, VecDeque};
use std::ffi::{c_char, c_int, c_void};
use std::sync::Arc;

use crate::ffi::{self, *};
use crate::theme::TerminalTheme;
use crate::types::*;

pub(crate) struct CallbackState {
    pub(crate) pty_writes: Vec<u8>,
    pub(crate) bell_count: u64,
    pub(crate) title_dirty: bool,
    pub(crate) pwd_dirty: bool,
    pub(crate) render_held: bool,
    pub(crate) dark_scheme: bool,
    pub(crate) size: GhosttySizeReportSize,
    pub(crate) events: Vec<VtEvent>,
}

unsafe extern "C" fn on_write_pty(
    _term: GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    if userdata.is_null() || data.is_null() || len == 0 {
        return;
    }
    // SAFETY: `userdata` is non-null and is the `Box::into_raw` `VtCore::cb` pointer registered in
    // `VtCore::new`; `libghostty-vt` invokes callbacks synchronously during FFI calls where no Rust
    // reference to `*cb` is live, callbacks do not re-enter `libghostty-vt`, and `Drop` frees
    // `term` before reclaiming `cb`.
    let state = unsafe { &mut *(userdata as *mut CallbackState) };
    // SAFETY: `data` is checked non-null and `len > 0` above, and `GhosttyTerminalWritePtyFn`
    // guarantees `data` points to `len` readable bytes for the duration of the callback.
    let slice = unsafe { std::slice::from_raw_parts(data, len) };
    state.pty_writes.extend_from_slice(slice);
}

unsafe extern "C" fn on_bell(_term: GhosttyTerminal, userdata: *mut c_void) {
    if !userdata.is_null() {
        // SAFETY: `userdata` is the non-null `Box::into_raw` `VtCore::cb` pointer; invoked
        // synchronously by `libghostty-vt` while no Rust reference to `*cb` is live, without
        // re-entrancy, and before `Drop` frees `term` and `cb`.
        let state = unsafe { &mut *(userdata as *mut CallbackState) };
        state.bell_count = state.bell_count.saturating_add(1);
        state.events.push(VtEvent::Bell);
    }
}

unsafe extern "C" fn on_title_changed(_term: GhosttyTerminal, userdata: *mut c_void) {
    if !userdata.is_null() {
        // SAFETY: `userdata` is the non-null `Box::into_raw` `VtCore::cb` pointer; invoked
        // synchronously by `libghostty-vt` while no Rust reference to `*cb` is live, without
        // re-entrancy, and before `Drop` frees `term` and `cb`.
        unsafe { (&mut *(userdata as *mut CallbackState)).title_dirty = true };
    }
}

unsafe extern "C" fn on_pwd_changed(_term: GhosttyTerminal, userdata: *mut c_void) {
    if !userdata.is_null() {
        // SAFETY: `userdata` is the non-null `Box::into_raw` `VtCore::cb` pointer; invoked
        // synchronously by `libghostty-vt` while no Rust reference to `*cb` is live, without
        // re-entrancy, and before `Drop` frees `term` and `cb`.
        unsafe { (&mut *(userdata as *mut CallbackState)).pwd_dirty = true };
    }
}

unsafe extern "C" fn on_color_scheme(
    _term: GhosttyTerminal,
    userdata: *mut c_void,
    out_scheme: *mut c_int,
) -> bool {
    if userdata.is_null() || out_scheme.is_null() {
        return false;
    }
    // SAFETY: `userdata` is the non-null `Box::into_raw` `VtCore::cb` pointer; invoked
    // synchronously by `libghostty-vt` while no Rust reference to `*cb` is live, without
    // re-entrancy, and before `Drop` frees `term` and `cb`.
    let state = unsafe { &*(userdata as *const CallbackState) };
    // SAFETY: `out_scheme` is checked non-null above, and `GhosttyTerminalColorSchemeFn`
    // guarantees it is a valid, aligned out-pointer for `GhosttyColorScheme` (`c_int`).
    unsafe {
        *out_scheme = if state.dark_scheme {
            GHOSTTY_COLOR_SCHEME_DARK
        } else {
            GHOSTTY_COLOR_SCHEME_LIGHT
        };
    }
    true
}

unsafe extern "C" fn on_size(
    _term: GhosttyTerminal,
    userdata: *mut c_void,
    out_size: *mut GhosttySizeReportSize,
) -> bool {
    if userdata.is_null() || out_size.is_null() {
        return false;
    }
    // SAFETY: `userdata` is the non-null `Box::into_raw` `VtCore::cb` pointer; invoked
    // synchronously by `libghostty-vt` while no Rust reference to `*cb` is live, without
    // re-entrancy, and before `Drop` frees `term` and `cb`.
    let state = unsafe { &*(userdata as *const CallbackState) };
    // SAFETY: `out_size` is checked non-null above, and `GhosttyTerminalSizeFn` guarantees it
    // is a valid, aligned out-pointer for `GhosttySizeReportSize`.
    unsafe {
        *out_size = state.size;
    }
    true
}

unsafe extern "C" fn on_render_hold(_term: GhosttyTerminal, userdata: *mut c_void, held: bool) {
    if !userdata.is_null() {
        // SAFETY: `userdata` is the non-null `Box::into_raw` `VtCore::cb` pointer; invoked
        // synchronously by `libghostty-vt` while no Rust reference to `*cb` is live, without
        // re-entrancy, and before `Drop` frees `term` and `cb`.
        let state = unsafe { &mut *(userdata as *mut CallbackState) };
        state.render_held = held;
        state.events.push(VtEvent::RenderHold(held));
    }
}

pub struct VtCore {
    pub(crate) term: GhosttyTerminal,
    pub(crate) render_state: GhosttyRenderState,
    pub(crate) row_iter: GhosttyRenderStateRowIterator,
    pub(crate) row_cells: GhosttyRenderStateRowCells,
    pub(crate) key_encoder: GhosttyKeyEncoder,
    pub(crate) key_event: GhosttyKeyEvent,
    pub(crate) mouse_encoder: GhosttyMouseEncoder,
    pub(crate) mouse_event: GhosttyMouseEvent,
    /// Owned heap allocation (`Box::into_raw`) shared with `libghostty-vt`'s `userdata`. Stored as
    /// a raw pointer so moving or borrowing `VtCore` never retags `*cb` across C callbacks; freed
    /// in `Drop` after `ghostty_terminal_free(self.term)`.
    cb: *mut CallbackState,
    pub(crate) theme: TerminalTheme,
    pub(crate) title: String,
    pub(crate) pwd: String,
    pub(crate) has_selection: bool,
    pub(crate) mutated_since_snapshot: bool,
    pub(crate) selection_changed: bool,
    pub(crate) row_cache: HashMap<u128, Arc<TerminalRow>>,
    pub(crate) row_lru: VecDeque<u128>,
    pub(crate) last_viewport_rows: Vec<Arc<TerminalRow>>,
    /// The find bar's search, made the first time it opens (`find.rs`).
    pub(crate) search: GhosttySearch,
    pub(crate) finding: bool,
    /// Finished commands, from the shell's OSC 133 marks (`marks.rs`).
    pub(crate) marks: crate::marks::Marks,
}

impl VtCore {
    pub fn new(
        cols: u16,
        rows: u16,
        scrollback_lines: usize,
        theme: TerminalTheme,
    ) -> Result<Self, String> {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let mut term: GhosttyTerminal = std::ptr::null_mut();
        let mut render_state: GhosttyRenderState = std::ptr::null_mut();
        let mut row_iter: GhosttyRenderStateRowIterator = std::ptr::null_mut();
        let mut row_cells: GhosttyRenderStateRowCells = std::ptr::null_mut();
        let mut key_encoder: GhosttyKeyEncoder = std::ptr::null_mut();
        let mut key_event: GhosttyKeyEvent = std::ptr::null_mut();
        let mut mouse_encoder: GhosttyMouseEncoder = std::ptr::null_mut();
        let mut mouse_event: GhosttyMouseEvent = std::ptr::null_mut();

        // SAFETY: `cols` and `rows` are clamped `>= 1`, `NULL` selects the default allocator,
        // each out-pointer is a valid stack slot, and on partial failure every non-null handle
        // already created is freed once before returning `Err`.
        unsafe {
            check(ffi::ghostty_terminal_new(std::ptr::null(), &mut term, cols, rows), "terminal_new")?;
            if let Err(e) = (|| -> Result<(), String> {
                check(ffi::ghostty_render_state_new(std::ptr::null(), &mut render_state), "render_state_new")?;
                check(
                    ffi::ghostty_render_state_row_iterator_new(std::ptr::null(), &mut row_iter),
                    "row_iterator_new",
                )?;
                check(
                    ffi::ghostty_render_state_row_cells_new(std::ptr::null(), &mut row_cells),
                    "row_cells_new",
                )?;
                check(ffi::ghostty_key_encoder_new(std::ptr::null(), &mut key_encoder), "key_encoder_new")?;
                check(ffi::ghostty_key_event_new(std::ptr::null(), &mut key_event), "key_event_new")?;
                check(
                    ffi::ghostty_mouse_encoder_new(std::ptr::null(), &mut mouse_encoder),
                    "mouse_encoder_new",
                )?;
                check(ffi::ghostty_mouse_event_new(std::ptr::null(), &mut mouse_event), "mouse_event_new")?;
                Ok(())
            })() {
                if !mouse_event.is_null() {
                    ffi::ghostty_mouse_event_free(mouse_event);
                }
                if !mouse_encoder.is_null() {
                    ffi::ghostty_mouse_encoder_free(mouse_encoder);
                }
                if !key_event.is_null() {
                    ffi::ghostty_key_event_free(key_event);
                }
                if !key_encoder.is_null() {
                    ffi::ghostty_key_encoder_free(key_encoder);
                }
                if !row_cells.is_null() {
                    ffi::ghostty_render_state_row_cells_free(row_cells);
                }
                if !row_iter.is_null() {
                    ffi::ghostty_render_state_row_iterator_free(row_iter);
                }
                if !render_state.is_null() {
                    ffi::ghostty_render_state_free(render_state);
                }
                ffi::ghostty_terminal_free(term);
                return Err(e);
            }
        }

        let cb = Box::into_raw(Box::new(CallbackState {
            pty_writes: Vec::new(),
            bell_count: 0,
            title_dirty: false,
            pwd_dirty: false,
            render_held: false,
            dark_scheme: theme.dark,
            size: GhosttySizeReportSize {
                rows,
                columns: cols,
                cell_width: 8,
                cell_height: 18,
            },
            events: Vec::new(),
        }));

        // SAFETY: `term` and `key_encoder` are freshly allocated live handles; `cb` was produced by
        // `Box::into_raw` with no live Rust reference to `*cb` during registration or the initial
        // `ghostty_terminal_vt_write` call; and every option pointer matches its key's C type.
        unsafe {
            let userdata_ptr = cb.cast::<c_void>();
            let _ = ffi::ghostty_terminal_set(term, GHOSTTY_TERMINAL_OPT_USERDATA, userdata_ptr);
            let _ = ffi::ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                (on_write_pty as GhosttyTerminalWritePtyFn) as *const c_void,
            );
            let _ = ffi::ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_BELL,
                (on_bell as GhosttyTerminalBellFn) as *const c_void,
            );
            let _ = ffi::ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_TITLE_CHANGED,
                (on_title_changed as GhosttyTerminalTitleChangedFn) as *const c_void,
            );
            let _ = ffi::ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_PWD_CHANGED,
                (on_pwd_changed as GhosttyTerminalPwdChangedFn) as *const c_void,
            );
            let _ = ffi::ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_COLOR_SCHEME,
                (on_color_scheme as GhosttyTerminalColorSchemeFn) as *const c_void,
            );
            let _ = ffi::ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_SIZE,
                (on_size as GhosttyTerminalSizeFn) as *const c_void,
            );
            let _ = ffi::ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_RENDER_HOLD,
                (on_render_hold as GhosttyTerminalRenderHoldFn) as *const c_void,
            );

            let bar_style = GHOSTTY_TERMINAL_CURSOR_STYLE_BAR;
            let _ = ffi::ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_STYLE,
                (&bar_style as *const c_int).cast(),
            );
            let blink = true;
            let _ = ffi::ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_BLINK,
                (&blink as *const bool).cast(),
            );
            // Apply DECSCUSR 0 so the initial screen cursor picks up default Bar + blink immediately.
            ffi::ghostty_terminal_vt_write(term, b"\x1b[0 q".as_ptr(), 5);
            // Scrollback is capped by bytes only: about 1 KB a 120-column line, so this keeps at
            // least scrollback_lines up to ~240 columns. A line cap makes Ghostty free and zero
            // a page each time the oldest one ages out, which was 59% of feeding plain output
            // (69 MB/s against 306 MB/s with the byte cap alone); the byte cap reuses the page.
            let max_bytes = scrollback_lines.saturating_mul(2048);
            let _ = ffi::ghostty_terminal_set(
                term,
                GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,
                (&max_bytes as *const usize).cast(),
            );

            // Enable cursor blinking (mode 12) and Unicode grapheme clustering (mode 2027) by default.
            for mode in [GHOSTTY_MODE_CURSOR_BLINKING, GHOSTTY_MODE_GRAPHEME_CLUSTER] {
                let m = GhosttyTerminalModeQuery { mode, value: true };
                let _ = ffi::ghostty_terminal_set(
                    term,
                    GHOSTTY_TERMINAL_OPT_MODE_DEFAULT,
                    (&m as *const GhosttyTerminalModeQuery).cast(),
                );
                let _ = ffi::ghostty_terminal_set(
                    term,
                    GHOSTTY_TERMINAL_OPT_MODE,
                    (&m as *const GhosttyTerminalModeQuery).cast(),
                );
            }
            let opt_alt = GHOSTTY_OPTION_AS_ALT_TRUE;
            ffi::ghostty_key_encoder_setopt(
                key_encoder,
                GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT,
                (&opt_alt as *const c_int).cast(),
            );
        }

        let mut core = Self {
            term,
            render_state,
            row_iter,
            row_cells,
            key_encoder,
            key_event,
            mouse_encoder,
            mouse_event,
            cb,
            theme: theme.clone(),
            title: String::new(),
            pwd: String::new(),
            has_selection: false,
            mutated_since_snapshot: true,
            selection_changed: false,
            row_cache: HashMap::with_capacity(128),
            row_lru: VecDeque::with_capacity(128),
            last_viewport_rows: Vec::new(),
            search: std::ptr::null_mut(),
            finding: false,
            marks: Default::default(),
        };
        core.apply_theme_colors(&theme);
        Ok(core)
    }

    /// Shared access to the callback state while no FFI call into `libghostty-vt` is active.
    pub(crate) fn cb(&self) -> &CallbackState {
        // SAFETY: `self.cb` was allocated with `Box::into_raw` in `VtCore::new` and is freed only
        // in `Drop` after `self.term` is freed. `VtCore` is `!Sync`, and no `libghostty-vt`
        // callback runs during `&self` methods or while this borrow is held.
        unsafe { &*self.cb }
    }

    /// Exclusive access to the callback state while no FFI call into `libghostty-vt` is active.
    pub(crate) fn cb_mut(&mut self) -> &mut CallbackState {
        // SAFETY: `self.cb` was allocated with `Box::into_raw` in `VtCore::new` and is freed only
        // in `Drop` after `self.term` is freed. `&mut self` guarantees exclusive access and prevents
        // any `libghostty-vt` FFI call (and thus any callback) while this borrow is live.
        unsafe { &mut *self.cb }
    }

    fn apply_theme_colors(&mut self, theme: &TerminalTheme) {
        let fg = theme.fg.to_ffi();
        let bg = theme.bg.to_ffi();
        let cursor = theme.cursor.to_ffi();
        let palette = theme.full_palette_ffi();
        // SAFETY: `self.term` was created in `VtCore::new` and is freed only in `Drop`; `fg`,
        // `bg`, and `cursor` are valid `GhosttyColorRgb` values, and `palette` is a 256-element
        // `GhosttyColorRgb` array as required by `GHOSTTY_TERMINAL_OPT_COLOR_PALETTE`.
        unsafe {
            let _ = ffi::ghostty_terminal_set(
                self.term,
                GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND,
                (&fg as *const GhosttyColorRgb).cast(),
            );
            let _ = ffi::ghostty_terminal_set(
                self.term,
                GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND,
                (&bg as *const GhosttyColorRgb).cast(),
            );
            let _ = ffi::ghostty_terminal_set(
                self.term,
                GHOSTTY_TERMINAL_OPT_COLOR_CURSOR,
                (&cursor as *const GhosttyColorRgb).cast(),
            );
            let _ = ffi::ghostty_terminal_set(
                self.term,
                GHOSTTY_TERMINAL_OPT_COLOR_PALETTE,
                palette.as_ptr().cast(),
            );
        }
    }

    pub fn set_theme(&mut self, theme: TerminalTheme) {
        let scheme_changed = self.theme.dark != theme.dark;
        self.cb_mut().dark_scheme = theme.dark;
        self.apply_theme_colors(&theme);
        self.theme = theme;
        self.mutated_since_snapshot = true;
        self.row_cache.clear();
        self.row_lru.clear();
        if scheme_changed
            && self.mode_enabled(GHOSTTY_MODE_COLOR_SCHEME_REPORT)
            && let Some(report) = self.color_scheme_report()
        {
            self.cb_mut().pty_writes.extend_from_slice(&report);
        }
    }

    pub fn color_scheme_report(&self) -> Option<Vec<u8>> {
        let scheme = if self.theme.dark {
            GHOSTTY_COLOR_SCHEME_DARK
        } else {
            GHOSTTY_COLOR_SCHEME_LIGHT
        };
        let mut buf = [0u8; 32];
        let mut written = 0usize;
        // SAFETY: `buf` is valid writable stack storage of `buf.len()` bytes and `&mut written`
        // is a valid out-pointer.
        let rc = unsafe {
            ffi::ghostty_color_scheme_report_encode(
                scheme,
                buf.as_mut_ptr().cast::<c_char>(),
                buf.len(),
                &mut written,
            )
        };
        (rc == GHOSTTY_SUCCESS && written > 0).then(|| buf[..written].to_vec())
    }

    /// Feeds a program's output, stopping at each OSC 133 mark to note it where the cursor is.
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut rest = bytes;
        while let Some((end, body)) = self.marks.next(rest) {
            self.feed_vt(&rest[..end]);
            let prompt_row = if body.first() == Some(&b'A') { self.cursor_row_id() } else { None };
            self.marks.step(&body, prompt_row);
            rest = &rest[end..];
        }
        self.feed_vt(rest);
    }

    /// The emulator's id for the row the cursor is on, while it's in view.
    fn cursor_row_id(&mut self) -> Option<u128> {
        let mut cursor = GhosttyRenderStateCursor::default();
        // SAFETY: `self.render_state`, `self.term`, and `self.row_iter` were created in
        // `VtCore::new` and are freed only in `Drop`; `&mut self` ensures exclusive access,
        // `cursor` has `size` initialized via `Default`, and each out-pointer matches its key.
        unsafe {
            let _ = ffi::ghostty_render_state_update(self.render_state, self.term);
            let _ = ffi::ghostty_render_state_get(self.render_state, GHOSTTY_RENDER_STATE_DATA_CURSOR, (&mut cursor as *mut GhosttyRenderStateCursor).cast());
            let _ = ffi::ghostty_render_state_get(self.render_state, GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, (&mut self.row_iter as *mut GhosttyRenderStateRowIterator).cast());
        }
        if !cursor.viewport_has_value {
            return None;
        }
        let mut row = 0;
        // SAFETY: `self.row_iter` was just populated from `self.render_state` with no intervening
        // render-state update.
        while unsafe { ffi::ghostty_render_state_row_iterator_next(self.row_iter) } {
            if row == cursor.viewport_y {
                let mut id = GhosttyRenderStateRowId::default();
                // SAFETY: `self.row_iter` is positioned on a row (`next` returned `true`) and
                // `&mut id` matches `GhosttyRenderStateRowId*`.
                unsafe {
                    let _ = ffi::ghostty_render_state_row_get(self.row_iter, GHOSTTY_RENDER_STATE_ROW_DATA_ID, (&mut id as *mut GhosttyRenderStateRowId).cast());
                }
                return Some(id.as_u128());
            }
            row += 1;
        }
        None
    }

    fn feed_vt(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        // SAFETY: `self.term` is live, `&mut self` serializes terminal access, `bytes` is valid for
        // `bytes.len()` readable bytes, and `self.cb` is stored as a raw pointer with no live Rust
        // reference to `*self.cb` across this FFI call, so synchronous callbacks may access it.
        unsafe {
            ffi::ghostty_terminal_vt_write(self.term, bytes.as_ptr(), bytes.len());
        }
        self.mutated_since_snapshot = true;
        if std::mem::take(&mut self.cb_mut().title_dirty) {
            self.title = self.query_string(GHOSTTY_TERMINAL_DATA_TITLE);
            let title = self.title.clone();
            self.cb_mut().events.push(VtEvent::TitleChanged(title));
        }
        if std::mem::take(&mut self.cb_mut().pwd_dirty) {
            self.pwd = self.query_string(GHOSTTY_TERMINAL_DATA_PWD);
            let pwd = self.pwd.clone();
            self.cb_mut().events.push(VtEvent::PwdChanged(pwd));
        }
    }

    pub fn take_pty_writes(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.cb_mut().pty_writes)
    }

    pub fn take_events(&mut self) -> Vec<VtEvent> {
        std::mem::take(&mut self.cb_mut().events)
    }

    pub fn render_held(&self) -> bool {
        self.cb().render_held
    }

    pub fn clear_render_hold(&mut self) {
        self.cb_mut().render_held = false;
    }

    pub fn resize(&mut self, cols: u16, rows: u16, cell_w_px: u32, cell_h_px: u32) {
        let (cols, rows, cw, ch) = (cols.max(1), rows.max(1), cell_w_px.max(1), cell_h_px.max(1));
        self.cb_mut().size = GhosttySizeReportSize {
            rows,
            columns: cols,
            cell_width: cw,
            cell_height: ch,
        };
        // SAFETY: `self.term` is live, `&mut self` serializes terminal access, and no Rust
        // reference to `*self.cb` is live across this FFI call (the temporary `cb_mut()` borrow
        // above has ended), so synchronous callbacks may access `CallbackState` via `userdata`.
        unsafe {
            let _ = ffi::ghostty_terminal_resize(self.term, cols, rows, cw, ch);
        }
        self.mutated_since_snapshot = true;
        self.row_cache.clear();
        self.row_lru.clear();
    }

    pub fn scroll(&mut self, op: ScrollOp) {
        // SAFETY: `GhosttyTerminalScrollViewport` is a `#[repr(C)]` POD struct of an integer tag
        // and integer union for which all-zeros is a valid bit pattern.
        let mut sv: GhosttyTerminalScrollViewport = unsafe { std::mem::zeroed() };
        match op {
            ScrollOp::Top => sv.tag = GHOSTTY_SCROLL_VIEWPORT_TOP,
            ScrollOp::Bottom => sv.tag = GHOSTTY_SCROLL_VIEWPORT_BOTTOM,
            ScrollOp::Delta(delta) => {
                sv.tag = GHOSTTY_SCROLL_VIEWPORT_DELTA;
                sv.value.delta = delta;
            }
            ScrollOp::Row(row) => {
                sv.tag = GHOSTTY_SCROLL_VIEWPORT_ROW;
                sv.value.row = row;
            }
            ScrollOp::Prompt(delta) => {
                sv.tag = GHOSTTY_SCROLL_VIEWPORT_DELTA_PROMPT;
                sv.value.delta_prompt = delta;
            }
        }
        // SAFETY: `self.term` was created in `VtCore::new` and is freed only in `Drop`, and `sv`
        // has its tag and matching union payload initialized above.
        unsafe {
            ffi::ghostty_terminal_scroll_viewport(self.term, sv);
        }
    }

    pub fn hyperlink_at(&self, col: u16, row: u16) -> Option<String> {
        let mut gref = GhosttyGridRef::default();
        let pt = GhosttyPoint::new(GHOSTTY_POINT_TAG_VIEWPORT, col, u32::from(row));
        // SAFETY: `self.term` was created in `VtCore::new` and is freed only in `Drop`, and
        // `gref` has `size` initialized via `Default`.
        if unsafe { ffi::ghostty_terminal_grid_ref(self.term, pt, &mut gref) } != GHOSTTY_SUCCESS {
            return None;
        }
        let mut buf = vec![0u8; 512];
        let mut len = 0usize;
        // SAFETY: `gref` was just produced by `self.term` with no intervening mutation, `buf` is
        // writable for `buf.len()` bytes, and `&mut len` is a valid out-pointer.
        let rc = unsafe {
            ffi::ghostty_grid_ref_hyperlink_uri(&gref, buf.as_mut_ptr(), buf.len(), &mut len)
        };
        if rc == GHOSTTY_OUT_OF_SPACE && len > buf.len() {
            buf.resize(len, 0);
            // SAFETY: `gref` is still valid (no terminal mutation occurred), `buf` was resized to
            // the required `len` writable bytes, and `&mut len` is a valid out-pointer.
            let rc2 = unsafe {
                ffi::ghostty_grid_ref_hyperlink_uri(&gref, buf.as_mut_ptr(), buf.len(), &mut len)
            };
            if rc2 != GHOSTTY_SUCCESS || len == 0 {
                return None;
            }
        } else if rc != GHOSTTY_SUCCESS || len == 0 {
            return None;
        }
        String::from_utf8(buf[..len].to_vec()).ok()
    }

    pub fn mode_enabled(&self, mode: GhosttyMode) -> bool {
        let mut q = GhosttyTerminalModeQuery { mode, value: false };
        // SAFETY: `self.term` was created in `VtCore::new` and is freed only in `Drop`, and `&mut q`
        // matches `GhosttyTerminalModeConfig*` for `GHOSTTY_TERMINAL_DATA_MODE`.
        unsafe {
            ffi::ghostty_terminal_get(
                self.term,
                GHOSTTY_TERMINAL_DATA_MODE,
                (&mut q as *mut GhosttyTerminalModeQuery).cast(),
            ) == GHOSTTY_SUCCESS
                && q.value
        }
    }

    fn query_string(&self, kind: c_int) -> String {
        let mut gs = GhosttyString { ptr: std::ptr::null(), len: 0 };
        // SAFETY: `self.term` is live, and `query_string` is only called with
        // `GHOSTTY_TERMINAL_DATA_TITLE` / `GHOSTTY_TERMINAL_DATA_PWD`, which both write a
        // `GhosttyString` into `out`.
        let rc = unsafe {
            ffi::ghostty_terminal_get(self.term, kind, (&mut gs as *mut GhosttyString).cast())
        };
        if rc != GHOSTTY_SUCCESS || gs.ptr.is_null() || gs.len == 0 {
            return String::new();
        }
        // SAFETY: `ghostty_terminal_get` succeeded with non-null `gs.ptr` and `gs.len > 0`;
        // `terminal.h` guarantees the borrowed bytes stay valid until the next mutating call on
        // `self.term` (`&self` prevents mutation while copying).
        unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(gs.ptr, gs.len)).into_owned() }
    }
}

impl Drop for VtCore {
    fn drop(&mut self) {
        // SAFETY: every handle except `self.search` was created in `VtCore::new` and is freed
        // only here; `self.search` is either null or created in `find`, and `ghostty_search_free`
        // treats `NULL` as a no-op (`search.h`). Freeing `self.search` before `self.term` detaches
        // tracked state cleanly, and `self.term` is freed before reclaiming `self.cb` (allocated
        // via `Box::into_raw` in `VtCore::new`) so `libghostty-vt` can never invoke a callback into
        // freed `CallbackState`.
        unsafe {
            ffi::ghostty_search_free(self.search);
            ffi::ghostty_mouse_event_free(self.mouse_event);
            ffi::ghostty_mouse_encoder_free(self.mouse_encoder);
            ffi::ghostty_key_event_free(self.key_event);
            ffi::ghostty_key_encoder_free(self.key_encoder);
            ffi::ghostty_render_state_row_cells_free(self.row_cells);
            ffi::ghostty_render_state_row_iterator_free(self.row_iter);
            ffi::ghostty_render_state_free(self.render_state);
            ffi::ghostty_terminal_free(self.term);
            drop(Box::from_raw(self.cb));
        }
    }
}

fn check(rc: GhosttyResult, op: &str) -> Result<(), String> {
    if rc == GHOSTTY_SUCCESS {
        Ok(())
    } else {
        Err(format!("libghostty-vt `{op}` failed with code {rc}"))
    }
}

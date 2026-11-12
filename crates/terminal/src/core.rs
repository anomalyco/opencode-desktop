//! Safe headless `libghostty-vt` emulator (`VtCore`): lifecycle, VT byte feeding, resize reflow,
//! scrollback, and selection. Input encoding lives in `input.rs`; render extraction in `render_extract.rs`.

use std::collections::{HashMap, VecDeque};
use std::ffi::{c_char, c_int, c_void};
use std::pin::Pin;
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
    let state = unsafe { &mut *(userdata as *mut CallbackState) };
    let slice = unsafe { std::slice::from_raw_parts(data, len) };
    state.pty_writes.extend_from_slice(slice);
}

unsafe extern "C" fn on_bell(_term: GhosttyTerminal, userdata: *mut c_void) {
    if !userdata.is_null() {
        let state = unsafe { &mut *(userdata as *mut CallbackState) };
        state.bell_count = state.bell_count.saturating_add(1);
        state.events.push(VtEvent::Bell);
    }
}

unsafe extern "C" fn on_title_changed(_term: GhosttyTerminal, userdata: *mut c_void) {
    if !userdata.is_null() {
        unsafe { (&mut *(userdata as *mut CallbackState)).title_dirty = true };
    }
}

unsafe extern "C" fn on_pwd_changed(_term: GhosttyTerminal, userdata: *mut c_void) {
    if !userdata.is_null() {
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
    let state = unsafe { &*(userdata as *const CallbackState) };
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
    let state = unsafe { &*(userdata as *const CallbackState) };
    unsafe {
        *out_size = state.size;
    }
    true
}

unsafe extern "C" fn on_render_hold(_term: GhosttyTerminal, userdata: *mut c_void, held: bool) {
    if !userdata.is_null() {
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
    pub(crate) cb: Pin<Box<CallbackState>>,
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

        let mut cb = Box::pin(CallbackState {
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
        });

        unsafe {
            let userdata_ptr = (&mut *cb as *mut CallbackState).cast::<c_void>();
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
        };
        core.apply_theme_colors(&theme);
        Ok(core)
    }

    fn apply_theme_colors(&mut self, theme: &TerminalTheme) {
        let fg = theme.fg.to_ffi();
        let bg = theme.bg.to_ffi();
        let cursor = theme.cursor.to_ffi();
        let palette = theme.full_palette_ffi();
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
        self.cb.dark_scheme = theme.dark;
        self.apply_theme_colors(&theme);
        self.theme = theme;
        self.mutated_since_snapshot = true;
        self.row_cache.clear();
        self.row_lru.clear();
        if scheme_changed
            && self.mode_enabled(GHOSTTY_MODE_COLOR_SCHEME_REPORT)
            && let Some(report) = self.color_scheme_report()
        {
            self.cb.pty_writes.extend_from_slice(&report);
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

    pub fn feed(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        unsafe {
            ffi::ghostty_terminal_vt_write(self.term, bytes.as_ptr(), bytes.len());
        }
        self.mutated_since_snapshot = true;
        if self.cb.title_dirty {
            self.cb.title_dirty = false;
            self.title = self.query_string(GHOSTTY_TERMINAL_DATA_TITLE);
            self.cb.events.push(VtEvent::TitleChanged(self.title.clone()));
        }
        if self.cb.pwd_dirty {
            self.cb.pwd_dirty = false;
            self.pwd = self.query_string(GHOSTTY_TERMINAL_DATA_PWD);
            self.cb.events.push(VtEvent::PwdChanged(self.pwd.clone()));
        }
    }

    pub fn take_pty_writes(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.cb.pty_writes)
    }

    pub fn take_events(&mut self) -> Vec<VtEvent> {
        std::mem::take(&mut self.cb.events)
    }

    pub fn render_held(&self) -> bool {
        self.cb.render_held
    }

    pub fn clear_render_hold(&mut self) {
        self.cb.render_held = false;
    }

    pub fn resize(&mut self, cols: u16, rows: u16, cell_w_px: u32, cell_h_px: u32) {
        let (cols, rows, cw, ch) = (cols.max(1), rows.max(1), cell_w_px.max(1), cell_h_px.max(1));
        self.cb.size = GhosttySizeReportSize {
            rows,
            columns: cols,
            cell_width: cw,
            cell_height: ch,
        };
        unsafe {
            let _ = ffi::ghostty_terminal_resize(self.term, cols, rows, cw, ch);
        }
        self.mutated_since_snapshot = true;
        self.row_cache.clear();
        self.row_lru.clear();
    }

    pub fn scroll(&mut self, op: ScrollOp) {
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
        unsafe {
            ffi::ghostty_terminal_scroll_viewport(self.term, sv);
        }
    }

    pub fn select(&mut self, op: SelectionOp) {
        self.selection_changed = true;
        match op {
            SelectionOp::Clear => {
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
                    if unsafe { ffi::ghostty_terminal_select_word(self.term, &opts, &mut sel) }
                        == GHOSTTY_SUCCESS
                    {
                        self.apply_selection(&sel);
                    }
                }
            }
            SelectionOp::Line(pt) => {
                let mut gref = GhosttyGridRef::default();
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
                let ok_s =
                    unsafe { ffi::ghostty_terminal_grid_ref(self.term, start.to_ffi(), &mut sref) }
                        == GHOSTTY_SUCCESS;
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
        }
    }

    fn apply_selection(&mut self, sel: &GhosttySelection) {
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
        let text = unsafe {
            let s = String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).into_owned();
            ffi::ghostty_free(std::ptr::null(), ptr, len);
            s
        };
        (!text.is_empty()).then_some(text)
    }

    pub fn hyperlink_at(&self, col: u16, row: u16) -> Option<String> {
        let mut gref = GhosttyGridRef::default();
        let pt = GhosttyPoint::new(GHOSTTY_POINT_TAG_VIEWPORT, col, u32::from(row));
        if unsafe { ffi::ghostty_terminal_grid_ref(self.term, pt, &mut gref) } != GHOSTTY_SUCCESS {
            return None;
        }
        let mut buf = vec![0u8; 512];
        let mut len = 0usize;
        let rc = unsafe {
            ffi::ghostty_grid_ref_hyperlink_uri(&gref, buf.as_mut_ptr(), buf.len(), &mut len)
        };
        if rc == GHOSTTY_OUT_OF_SPACE && len > buf.len() {
            buf.resize(len, 0);
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
        let rc = unsafe {
            ffi::ghostty_terminal_get(self.term, kind, (&mut gs as *mut GhosttyString).cast())
        };
        if rc != GHOSTTY_SUCCESS || gs.ptr.is_null() || gs.len == 0 {
            return String::new();
        }
        unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(gs.ptr, gs.len)).into_owned() }
    }
}

impl Drop for VtCore {
    fn drop(&mut self) {
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

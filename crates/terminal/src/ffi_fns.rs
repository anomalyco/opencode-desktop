//! Raw `extern "C"` function declarations and effect callback types for `libghostty-vt`.

#![allow(non_camel_case_types, dead_code)]

use std::ffi::{c_char, c_int, c_void};

use crate::ffi::*;

pub type GhosttyTerminalWritePtyFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void, data: *const u8, len: usize);
pub type GhosttyTerminalBellFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void);
pub type GhosttyTerminalTitleChangedFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void);
pub type GhosttyTerminalPwdChangedFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void);
pub type GhosttyTerminalColorSchemeFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void, out_scheme: *mut c_int) -> bool;
pub type GhosttyTerminalSizeFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    out_size: *mut GhosttySizeReportSize,
) -> bool;
pub type GhosttyTerminalRenderHoldFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void, held: bool);

unsafe extern "C" {
    pub fn ghostty_type_json() -> *const c_char;
    pub fn ghostty_free(allocator: *const GhosttyAllocator, ptr: *mut u8, len: usize);

    pub fn ghostty_color_palette_default(out: *mut GhosttyColorRgb);
    pub fn ghostty_color_scheme_report_encode(
        scheme: c_int,
        buf: *mut c_char,
        buf_len: usize,
        out_written: *mut usize,
    ) -> GhosttyResult;

    pub fn ghostty_terminal_new(
        allocator: *const GhosttyAllocator,
        terminal: *mut GhosttyTerminal,
        cols: u16,
        rows: u16,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_free(terminal: GhosttyTerminal);
    pub fn ghostty_terminal_reset(terminal: GhosttyTerminal);
    pub fn ghostty_terminal_resize(
        terminal: GhosttyTerminal,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_set(
        terminal: GhosttyTerminal,
        option: c_int,
        value: *const c_void,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_get(
        terminal: GhosttyTerminal,
        data: c_int,
        out: *mut c_void,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_vt_write(terminal: GhosttyTerminal, data: *const u8, len: usize);
    pub fn ghostty_terminal_scroll_viewport(
        terminal: GhosttyTerminal,
        behavior: GhosttyTerminalScrollViewport,
    );
    pub fn ghostty_terminal_grid_ref(
        terminal: GhosttyTerminal,
        point: GhosttyPoint,
        out_ref: *mut GhosttyGridRef,
    ) -> GhosttyResult;

    pub fn ghostty_terminal_select_word(
        terminal: GhosttyTerminal,
        options: *const GhosttyTerminalSelectWordOptions,
        out_selection: *mut GhosttySelection,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_select_line(
        terminal: GhosttyTerminal,
        options: *const GhosttyTerminalSelectLineOptions,
        out_selection: *mut GhosttySelection,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_select_all(
        terminal: GhosttyTerminal,
        out_selection: *mut GhosttySelection,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_selection_format_alloc(
        terminal: GhosttyTerminal,
        allocator: *const GhosttyAllocator,
        options: GhosttyTerminalSelectionFormatOptions,
        out_ptr: *mut *mut u8,
        out_len: *mut usize,
    ) -> GhosttyResult;

    pub fn ghostty_formatter_terminal_new(
        allocator: *const GhosttyAllocator,
        formatter: *mut GhosttyFormatter,
        terminal: GhosttyTerminal,
        options: GhosttyFormatterTerminalOptions,
    ) -> GhosttyResult;
    pub fn ghostty_formatter_format_alloc(
        formatter: GhosttyFormatter,
        allocator: *const GhosttyAllocator,
        out_ptr: *mut *mut u8,
        out_len: *mut usize,
    ) -> GhosttyResult;
    pub fn ghostty_formatter_free(formatter: GhosttyFormatter);

    pub fn ghostty_render_state_new(
        allocator: *const GhosttyAllocator,
        state: *mut GhosttyRenderState,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_free(state: GhosttyRenderState);
    pub fn ghostty_render_state_update(
        state: GhosttyRenderState,
        terminal: GhosttyTerminal,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_clean(state: GhosttyRenderState) -> GhosttyResult;
    pub fn ghostty_render_state_get(
        state: GhosttyRenderState,
        data: c_int,
        out: *mut c_void,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_iterator_new(
        allocator: *const GhosttyAllocator,
        out_iterator: *mut GhosttyRenderStateRowIterator,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_iterator_free(iterator: GhosttyRenderStateRowIterator);
    pub fn ghostty_render_state_row_iterator_next(iterator: GhosttyRenderStateRowIterator) -> bool;
    pub fn ghostty_render_state_row_get(
        iterator: GhosttyRenderStateRowIterator,
        data: c_int,
        out: *mut c_void,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_cells_new(
        allocator: *const GhosttyAllocator,
        out_cells: *mut GhosttyRenderStateRowCells,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_cells_free(cells: GhosttyRenderStateRowCells);
    pub fn ghostty_render_state_row_cells_next(cells: GhosttyRenderStateRowCells) -> bool;
    pub fn ghostty_render_state_row_cells_get(
        cells: GhosttyRenderStateRowCells,
        data: c_int,
        out: *mut c_void,
    ) -> GhosttyResult;

    pub fn ghostty_cell_get(cell: GhosttyCell, data: c_int, out: *mut c_void) -> GhosttyResult;
    pub fn ghostty_row_get(row: GhosttyRow, data: c_int, out: *mut c_void) -> GhosttyResult;
    pub fn ghostty_grid_ref_hyperlink_uri(
        grid_ref: *const GhosttyGridRef,
        buf: *mut u8,
        buf_len: usize,
        out_len: *mut usize,
    ) -> GhosttyResult;

    pub fn ghostty_key_event_new(
        allocator: *const GhosttyAllocator,
        event: *mut GhosttyKeyEvent,
    ) -> GhosttyResult;
    pub fn ghostty_key_event_free(event: GhosttyKeyEvent);
    pub fn ghostty_key_event_set_action(event: GhosttyKeyEvent, action: c_int);
    pub fn ghostty_key_event_set_key(event: GhosttyKeyEvent, key: GhosttyKey);
    pub fn ghostty_key_event_set_mods(event: GhosttyKeyEvent, mods: GhosttyMods);
    pub fn ghostty_key_event_set_consumed_mods(event: GhosttyKeyEvent, consumed: GhosttyMods);
    pub fn ghostty_key_event_set_composing(event: GhosttyKeyEvent, composing: bool);
    pub fn ghostty_key_event_set_utf8(event: GhosttyKeyEvent, utf8: *const c_char, len: usize);
    pub fn ghostty_key_event_set_unshifted_codepoint(event: GhosttyKeyEvent, codepoint: u32);

    pub fn ghostty_key_encoder_new(
        allocator: *const GhosttyAllocator,
        encoder: *mut GhosttyKeyEncoder,
    ) -> GhosttyResult;
    pub fn ghostty_key_encoder_free(encoder: GhosttyKeyEncoder);
    pub fn ghostty_key_encoder_setopt(
        encoder: GhosttyKeyEncoder,
        option: c_int,
        value: *const c_void,
    );
    pub fn ghostty_key_encoder_setopt_from_terminal(
        encoder: GhosttyKeyEncoder,
        terminal: GhosttyTerminal,
    );
    pub fn ghostty_key_encoder_encode(
        encoder: GhosttyKeyEncoder,
        event: GhosttyKeyEvent,
        out_buf: *mut c_char,
        out_buf_size: usize,
        out_len: *mut usize,
    ) -> GhosttyResult;

    pub fn ghostty_mouse_event_new(
        allocator: *const GhosttyAllocator,
        event: *mut GhosttyMouseEvent,
    ) -> GhosttyResult;
    pub fn ghostty_mouse_event_free(event: GhosttyMouseEvent);
    pub fn ghostty_mouse_event_set_action(event: GhosttyMouseEvent, action: c_int);
    pub fn ghostty_mouse_event_set_button(event: GhosttyMouseEvent, button: c_int);
    pub fn ghostty_mouse_event_clear_button(event: GhosttyMouseEvent);
    pub fn ghostty_mouse_event_set_mods(event: GhosttyMouseEvent, mods: GhosttyMods);
    pub fn ghostty_mouse_event_set_position(event: GhosttyMouseEvent, pos: GhosttyMousePosition);

    pub fn ghostty_mouse_encoder_new(
        allocator: *const GhosttyAllocator,
        encoder: *mut GhosttyMouseEncoder,
    ) -> GhosttyResult;
    pub fn ghostty_mouse_encoder_free(encoder: GhosttyMouseEncoder);
    pub fn ghostty_mouse_encoder_setopt(
        encoder: GhosttyMouseEncoder,
        option: c_int,
        value: *const c_void,
    );
    pub fn ghostty_mouse_encoder_setopt_from_terminal(
        encoder: GhosttyMouseEncoder,
        terminal: GhosttyTerminal,
    );
    pub fn ghostty_mouse_encoder_encode(
        encoder: GhosttyMouseEncoder,
        event: GhosttyMouseEvent,
        out_buf: *mut c_char,
        out_buf_size: usize,
        out_len: *mut usize,
    ) -> GhosttyResult;

    pub fn ghostty_paste_is_safe(data: *const c_char, len: usize) -> bool;
    pub fn ghostty_paste_encode(
        data: *mut c_char,
        data_len: usize,
        bracketed: bool,
        buf: *mut c_char,
        buf_len: usize,
        out_written: *mut usize,
    ) -> GhosttyResult;
}

//! The web build has no Ghostty engine linked in yet: every `libghostty-vt` entry point answers
//! as a failed call would, so a terminal fails to start (`VtCore::new` reports it) and the rest
//! of the crate builds and runs unchanged.

#![allow(clippy::missing_safety_doc, unsafe_code)]

use std::ffi::{c_char, c_int, c_void};

use crate::ffi::*;
use crate::ffi_fns::*;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_type_json() -> *const c_char {
    std::ptr::null()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_free(_: *const GhosttyAllocator, _: *mut u8, _: usize) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_color_palette_default(_: *mut GhosttyColorRgb) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_color_palette_generate(_: *const GhosttyColorRgb, _: *const GhosttyColorPaletteMask, _: *const GhosttyColorRgb, _: *const GhosttyColorRgb, _: bool, _: *mut GhosttyColorRgb) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_color_scheme_report_encode(_: c_int, _: *mut c_char, _: usize, _: *mut usize) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_terminal_new(_: *const GhosttyAllocator, _: *mut GhosttyTerminal, _: u16, _: u16) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_terminal_free(_: GhosttyTerminal) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_terminal_reset(_: GhosttyTerminal) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_terminal_resize(_: GhosttyTerminal, _: u16, _: u16, _: u32, _: u32) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_terminal_set(_: GhosttyTerminal, _: c_int, _: *const c_void) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_terminal_get(_: GhosttyTerminal, _: c_int, _: *mut c_void) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_terminal_vt_write(_: GhosttyTerminal, _: *const u8, _: usize) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_terminal_scroll_viewport(_: GhosttyTerminal, _: GhosttyTerminalScrollViewport) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_terminal_grid_ref(_: GhosttyTerminal, _: GhosttyPoint, _: *mut GhosttyGridRef) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_terminal_select_word(_: GhosttyTerminal, _: *const GhosttyTerminalSelectWordOptions, _: *mut GhosttySelection) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_terminal_select_line(_: GhosttyTerminal, _: *const GhosttyTerminalSelectLineOptions, _: *mut GhosttySelection) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_terminal_select_all(_: GhosttyTerminal, _: *mut GhosttySelection) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_terminal_selection_format_alloc(_: GhosttyTerminal, _: *const GhosttyAllocator, _: GhosttyTerminalSelectionFormatOptions, _: *mut *mut u8, _: *mut usize) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_formatter_terminal_new(_: *const GhosttyAllocator, _: *mut GhosttyFormatter, _: GhosttyTerminal, _: GhosttyFormatterTerminalOptions) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_formatter_format_alloc(_: GhosttyFormatter, _: *const GhosttyAllocator, _: *mut *mut u8, _: *mut usize) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_formatter_free(_: GhosttyFormatter) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_render_state_new(_: *const GhosttyAllocator, _: *mut GhosttyRenderState) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_render_state_free(_: GhosttyRenderState) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_render_state_update(_: GhosttyRenderState, _: GhosttyTerminal) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_render_state_clean(_: GhosttyRenderState) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_render_state_get(_: GhosttyRenderState, _: c_int, _: *mut c_void) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_render_state_row_iterator_new(_: *const GhosttyAllocator, _: *mut GhosttyRenderStateRowIterator) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_render_state_row_iterator_free(_: GhosttyRenderStateRowIterator) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_render_state_row_iterator_next(_: GhosttyRenderStateRowIterator) -> bool {
    false
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_render_state_row_get(_: GhosttyRenderStateRowIterator, _: c_int, _: *mut c_void) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_render_state_row_cells_new(_: *const GhosttyAllocator, _: *mut GhosttyRenderStateRowCells) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_render_state_row_cells_free(_: GhosttyRenderStateRowCells) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_render_state_row_cells_next(_: GhosttyRenderStateRowCells) -> bool {
    false
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_render_state_row_cells_get(_: GhosttyRenderStateRowCells, _: c_int, _: *mut c_void) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_cell_get(_: GhosttyCell, _: c_int, _: *mut c_void) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_row_get(_: GhosttyRow, _: c_int, _: *mut c_void) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_grid_ref_hyperlink_uri(_: *const GhosttyGridRef, _: *mut u8, _: usize, _: *mut usize) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_key_event_new(_: *const GhosttyAllocator, _: *mut GhosttyKeyEvent) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_key_event_free(_: GhosttyKeyEvent) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_key_event_set_action(_: GhosttyKeyEvent, _: c_int) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_key_event_set_key(_: GhosttyKeyEvent, _: GhosttyKey) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_key_event_set_mods(_: GhosttyKeyEvent, _: GhosttyMods) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_key_event_set_consumed_mods(_: GhosttyKeyEvent, _: GhosttyMods) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_key_event_set_composing(_: GhosttyKeyEvent, _: bool) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_key_event_set_utf8(_: GhosttyKeyEvent, _: *const c_char, _: usize) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_key_event_set_unshifted_codepoint(_: GhosttyKeyEvent, _: u32) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_key_encoder_new(_: *const GhosttyAllocator, _: *mut GhosttyKeyEncoder) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_key_encoder_free(_: GhosttyKeyEncoder) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_key_encoder_setopt(_: GhosttyKeyEncoder, _: c_int, _: *const c_void) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_key_encoder_setopt_from_terminal(_: GhosttyKeyEncoder, _: GhosttyTerminal) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_key_encoder_encode(_: GhosttyKeyEncoder, _: GhosttyKeyEvent, _: *mut c_char, _: usize, _: *mut usize) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_mouse_event_new(_: *const GhosttyAllocator, _: *mut GhosttyMouseEvent) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_mouse_event_free(_: GhosttyMouseEvent) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_mouse_event_set_action(_: GhosttyMouseEvent, _: c_int) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_mouse_event_set_button(_: GhosttyMouseEvent, _: c_int) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_mouse_event_clear_button(_: GhosttyMouseEvent) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_mouse_event_set_mods(_: GhosttyMouseEvent, _: GhosttyMods) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_mouse_event_set_position(_: GhosttyMouseEvent, _: GhosttyMousePosition) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_mouse_encoder_new(_: *const GhosttyAllocator, _: *mut GhosttyMouseEncoder) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_mouse_encoder_free(_: GhosttyMouseEncoder) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_mouse_encoder_setopt(_: GhosttyMouseEncoder, _: c_int, _: *const c_void) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_mouse_encoder_setopt_from_terminal(_: GhosttyMouseEncoder, _: GhosttyTerminal) {
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_mouse_encoder_encode(_: GhosttyMouseEncoder, _: GhosttyMouseEvent, _: *mut c_char, _: usize, _: *mut usize) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_paste_is_safe(_: *const c_char, _: usize) -> bool {
    false
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ghostty_paste_encode(_: *mut c_char, _: usize, _: bool, _: *mut c_char, _: usize, _: *mut usize) -> GhosttyResult {
    GHOSTTY_INVALID_VALUE
}

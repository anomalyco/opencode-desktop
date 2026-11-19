//! Raw C ABI types and constants for `libghostty-vt` (`v1.4.0`, commit `83fb95624b`).
//! Function declarations live in [`crate::ffi_fns`].

#![allow(non_camel_case_types, dead_code)]

use std::ffi::{c_int, c_void};

pub use crate::ffi_fns::*;

pub type GhosttyResult = c_int;
pub const GHOSTTY_SUCCESS: GhosttyResult = 0;
pub const GHOSTTY_OUT_OF_MEMORY: GhosttyResult = -1;
pub const GHOSTTY_INVALID_VALUE: GhosttyResult = -2;
pub const GHOSTTY_OUT_OF_SPACE: GhosttyResult = -3;
pub const GHOSTTY_NO_VALUE: GhosttyResult = -4;
pub const GHOSTTY_IO_ERROR: GhosttyResult = -5;
pub const GHOSTTY_LIMIT_EXCEEDED: GhosttyResult = -6;
pub const GHOSTTY_REJECTED: GhosttyResult = -7;

pub type GhosttyTerminal = *mut c_void;
pub type GhosttyRenderState = *mut c_void;
pub type GhosttyRenderStateRowIterator = *mut c_void;
pub type GhosttyRenderStateRowCells = *mut c_void;
pub type GhosttyKeyEncoder = *mut c_void;
pub type GhosttyKeyEvent = *mut c_void;
pub type GhosttyMouseEncoder = *mut c_void;
pub type GhosttyMouseEvent = *mut c_void;
pub type GhosttyFormatter = *mut c_void;
pub type GhosttyAllocator = c_void;

pub type GhosttyCell = u64;
pub type GhosttyRow = u64;
pub type GhosttyMode = u16;
pub type GhosttyMods = u16;

pub const fn ghostty_mode_new(value: u16, ansi: bool) -> GhosttyMode {
    (value & 0x7FFF) | ((ansi as u16) << 15)
}

pub const GHOSTTY_MODE_DECCKM: GhosttyMode = ghostty_mode_new(1, false);
pub const GHOSTTY_MODE_CURSOR_BLINKING: GhosttyMode = ghostty_mode_new(12, false);
pub const GHOSTTY_MODE_CURSOR_VISIBLE: GhosttyMode = ghostty_mode_new(25, false);
pub const GHOSTTY_MODE_FOCUS_EVENT: GhosttyMode = ghostty_mode_new(1004, false);
pub const GHOSTTY_MODE_ALT_SCROLL: GhosttyMode = ghostty_mode_new(1007, false);
pub const GHOSTTY_MODE_ALT_SCREEN: GhosttyMode = ghostty_mode_new(1047, false);
pub const GHOSTTY_MODE_ALT_SCREEN_SAVE: GhosttyMode = ghostty_mode_new(1049, false);
pub const GHOSTTY_MODE_BRACKETED_PASTE: GhosttyMode = ghostty_mode_new(2004, false);
pub const GHOSTTY_MODE_SYNC_OUTPUT: GhosttyMode = ghostty_mode_new(2026, false);
pub const GHOSTTY_MODE_GRAPHEME_CLUSTER: GhosttyMode = ghostty_mode_new(2027, false);
pub const GHOSTTY_MODE_COLOR_SCHEME_REPORT: GhosttyMode = ghostty_mode_new(2031, false);
pub const GHOSTTY_MODE_IN_BAND_RESIZE: GhosttyMode = ghostty_mode_new(2048, false);
pub const GHOSTTY_MODE_PASTE_EVENTS: GhosttyMode = ghostty_mode_new(5522, false);

pub const GHOSTTY_MODS_SHIFT: GhosttyMods = 1 << 0;
pub const GHOSTTY_MODS_CTRL: GhosttyMods = 1 << 1;
pub const GHOSTTY_MODS_ALT: GhosttyMods = 1 << 2;
pub const GHOSTTY_MODS_SUPER: GhosttyMods = 1 << 3;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyString {
    pub ptr: *const u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyBuffer {
    pub ptr: *mut u8,
    pub cap: usize,
    pub len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GhosttyColorRgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

pub const GHOSTTY_STYLE_COLOR_NONE: c_int = 0;
pub const GHOSTTY_STYLE_COLOR_PALETTE: c_int = 1;
pub const GHOSTTY_STYLE_COLOR_RGB: c_int = 2;

#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyStyleColorValue {
    pub palette: u8,
    pub rgb: GhosttyColorRgb,
    pub _padding: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyStyleColor {
    pub tag: c_int,
    pub value: GhosttyStyleColorValue,
}

pub const GHOSTTY_SGR_UNDERLINE_NONE: c_int = 0;
pub const GHOSTTY_SGR_UNDERLINE_SINGLE: c_int = 1;
pub const GHOSTTY_SGR_UNDERLINE_DOUBLE: c_int = 2;
pub const GHOSTTY_SGR_UNDERLINE_CURLY: c_int = 3;
pub const GHOSTTY_SGR_UNDERLINE_DOTTED: c_int = 4;
pub const GHOSTTY_SGR_UNDERLINE_DASHED: c_int = 5;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyStyle {
    pub size: usize,
    pub fg_color: GhosttyStyleColor,
    pub bg_color: GhosttyStyleColor,
    pub underline_color: GhosttyStyleColor,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub underline: c_int,
}

impl Default for GhosttyStyle {
    fn default() -> Self {
        // SAFETY: `GhosttyStyle` is a `#[repr(C)]` POD struct of integers, bools, and C unions
        // for which all-zeros is a valid bit pattern.
        let mut s: Self = unsafe { std::mem::zeroed() };
        s.size = std::mem::size_of::<Self>();
        s
    }
}

pub const GHOSTTY_POINT_TAG_ACTIVE: c_int = 0;
pub const GHOSTTY_POINT_TAG_VIEWPORT: c_int = 1;
pub const GHOSTTY_POINT_TAG_SCREEN: c_int = 2;
pub const GHOSTTY_POINT_TAG_HISTORY: c_int = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GhosttyPointCoordinate {
    pub x: u16,
    pub y: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyPointValue {
    pub coordinate: GhosttyPointCoordinate,
    pub _padding: [u64; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyPoint {
    pub tag: c_int,
    pub value: GhosttyPointValue,
}

impl GhosttyPoint {
    pub fn new(tag: c_int, x: u16, y: u32) -> Self {
        // SAFETY: `GhosttyPoint` is a `#[repr(C)]` POD struct of integers and a C union for
        // which all-zeros is a valid bit pattern.
        let mut p: Self = unsafe { std::mem::zeroed() };
        p.tag = tag;
        p.value.coordinate = GhosttyPointCoordinate { x, y };
        p
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyGridRef {
    pub size: usize,
    pub node: *mut c_void,
    pub x: u16,
    pub y: u16,
}

impl Default for GhosttyGridRef {
    fn default() -> Self {
        Self {
            size: std::mem::size_of::<Self>(),
            node: std::ptr::null_mut(),
            x: 0,
            y: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttySelection {
    pub size: usize,
    pub start: GhosttyGridRef,
    pub end: GhosttyGridRef,
    pub rectangle: bool,
}

impl Default for GhosttySelection {
    fn default() -> Self {
        Self {
            size: std::mem::size_of::<Self>(),
            start: GhosttyGridRef::default(),
            end: GhosttyGridRef::default(),
            rectangle: false,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalSelectWordOptions {
    pub size: usize,
    pub grid_ref: GhosttyGridRef,
    pub boundary_codepoints: *const u32,
    pub boundary_codepoints_len: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalSelectLineOptions {
    pub size: usize,
    pub grid_ref: GhosttyGridRef,
    pub whitespace: *const u32,
    pub whitespace_len: usize,
    pub semantic_prompt_boundary: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalSelectionFormatOptions {
    pub size: usize,
    pub emit: c_int,
    pub unwrap: bool,
    pub trim: bool,
    pub selection: *const GhosttySelection,
}

pub const GHOSTTY_FORMATTER_FORMAT_PLAIN: c_int = 0;
pub const GHOSTTY_FORMATTER_FORMAT_VT: c_int = 1;
pub const GHOSTTY_FORMATTER_FORMAT_HTML: c_int = 2;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyFormatterScreenExtra {
    pub size: usize,
    pub cursor: bool,
    pub style: bool,
    pub hyperlink: bool,
    pub protection: bool,
    pub kitty_keyboard: bool,
    pub charsets: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyFormatterTerminalExtra {
    pub size: usize,
    pub palette: bool,
    pub modes: bool,
    pub scrolling_region: bool,
    pub tabstops: bool,
    pub pwd: bool,
    pub keyboard: bool,
    pub screen: GhosttyFormatterScreenExtra,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyFormatterTerminalOptions {
    pub size: usize,
    pub emit: c_int,
    pub unwrap: bool,
    pub trim: bool,
    pub extra: GhosttyFormatterTerminalExtra,
    pub selection: *const GhosttySelection,
}

impl GhosttyFormatterTerminalOptions {
    pub fn vt_full() -> Self {
        let screen = GhosttyFormatterScreenExtra {
            size: std::mem::size_of::<GhosttyFormatterScreenExtra>(),
            cursor: true,
            style: true,
            hyperlink: true,
            protection: false,
            kitty_keyboard: true,
            charsets: true,
        };
        let extra = GhosttyFormatterTerminalExtra {
            size: std::mem::size_of::<GhosttyFormatterTerminalExtra>(),
            palette: false,
            modes: true,
            scrolling_region: true,
            tabstops: false,
            pwd: true,
            keyboard: true,
            screen,
        };
        Self {
            size: std::mem::size_of::<Self>(),
            emit: GHOSTTY_FORMATTER_FORMAT_VT,
            unwrap: false,
            trim: true,
            extra,
            selection: std::ptr::null(),
        }
    }

    pub fn plain_trimmed() -> Self {
        let screen = GhosttyFormatterScreenExtra {
            size: std::mem::size_of::<GhosttyFormatterScreenExtra>(),
            cursor: false,
            style: false,
            hyperlink: false,
            protection: false,
            kitty_keyboard: false,
            charsets: false,
        };
        let extra = GhosttyFormatterTerminalExtra {
            size: std::mem::size_of::<GhosttyFormatterTerminalExtra>(),
            palette: false,
            modes: false,
            scrolling_region: false,
            tabstops: false,
            pwd: false,
            keyboard: false,
            screen,
        };
        Self {
            size: std::mem::size_of::<Self>(),
            emit: GHOSTTY_FORMATTER_FORMAT_PLAIN,
            unwrap: false,
            trim: true,
            extra,
            selection: std::ptr::null(),
        }
    }
}

pub const GHOSTTY_SCROLL_VIEWPORT_TOP: c_int = 0;
pub const GHOSTTY_SCROLL_VIEWPORT_BOTTOM: c_int = 1;
pub const GHOSTTY_SCROLL_VIEWPORT_DELTA: c_int = 2;
pub const GHOSTTY_SCROLL_VIEWPORT_ROW: c_int = 3;
pub const GHOSTTY_SCROLL_VIEWPORT_DELTA_PROMPT: c_int = 4;

#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyTerminalScrollViewportValue {
    pub delta: isize,
    pub row: usize,
    pub delta_prompt: isize,
    pub _padding: [u64; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalScrollViewport {
    pub tag: c_int,
    pub value: GhosttyTerminalScrollViewportValue,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GhosttyTerminalScrollbar {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
}

pub const GHOSTTY_TERMINAL_SCREEN_PRIMARY: c_int = 0;
pub const GHOSTTY_TERMINAL_SCREEN_ALTERNATE: c_int = 1;

pub const GHOSTTY_TERMINAL_CURSOR_STYLE_BAR: c_int = 0;
pub const GHOSTTY_TERMINAL_CURSOR_STYLE_BLOCK: c_int = 1;
pub const GHOSTTY_TERMINAL_CURSOR_STYLE_UNDERLINE: c_int = 2;
pub const GHOSTTY_TERMINAL_CURSOR_STYLE_BLOCK_HOLLOW: c_int = 3;

pub const GHOSTTY_COLOR_SCHEME_LIGHT: c_int = 0;
pub const GHOSTTY_COLOR_SCHEME_DARK: c_int = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GhosttySizeReportSize {
    pub rows: u16,
    pub columns: u16,
    pub cell_width: u32,
    pub cell_height: u32,
}

pub const GHOSTTY_TERMINAL_OPT_USERDATA: c_int = 0;
pub const GHOSTTY_TERMINAL_OPT_WRITE_PTY: c_int = 1;
pub const GHOSTTY_TERMINAL_OPT_BELL: c_int = 2;
pub const GHOSTTY_TERMINAL_OPT_TITLE_CHANGED: c_int = 5;
pub const GHOSTTY_TERMINAL_OPT_SIZE: c_int = 6;
pub const GHOSTTY_TERMINAL_OPT_COLOR_SCHEME: c_int = 7;
pub const GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND: c_int = 11;
pub const GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND: c_int = 12;
pub const GHOSTTY_TERMINAL_OPT_COLOR_CURSOR: c_int = 13;
pub const GHOSTTY_TERMINAL_OPT_COLOR_PALETTE: c_int = 14;
pub const GHOSTTY_TERMINAL_OPT_SELECTION: c_int = 21;
pub const GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_STYLE: c_int = 22;
pub const GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_BLINK: c_int = 23;
pub const GHOSTTY_TERMINAL_OPT_PWD_CHANGED: c_int = 25;
pub const GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES: c_int = 27;
pub const GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES: c_int = 28;
pub const GHOSTTY_TERMINAL_OPT_MODE_DEFAULT: c_int = 33;
pub const GHOSTTY_TERMINAL_OPT_MODE: c_int = 34;
pub const GHOSTTY_TERMINAL_OPT_RENDER_HOLD: c_int = 41;

pub const GHOSTTY_TERMINAL_DATA_COLS: c_int = 1;
pub const GHOSTTY_TERMINAL_DATA_ROWS: c_int = 2;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_X: c_int = 3;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_Y: c_int = 4;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_PENDING_WRAP: c_int = 5;
pub const GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN: c_int = 6;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE: c_int = 7;
pub const GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS: c_int = 8;
pub const GHOSTTY_TERMINAL_DATA_SCROLLBAR: c_int = 9;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_STYLE: c_int = 10;
pub const GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING: c_int = 11;
pub const GHOSTTY_TERMINAL_DATA_TITLE: c_int = 12;
pub const GHOSTTY_TERMINAL_DATA_PWD: c_int = 13;
pub const GHOSTTY_TERMINAL_DATA_TOTAL_ROWS: c_int = 14;
pub const GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS: c_int = 15;
pub const GHOSTTY_TERMINAL_DATA_SELECTION: c_int = 31;
pub const GHOSTTY_TERMINAL_DATA_VIEWPORT_ACTIVE: c_int = 32;
pub const GHOSTTY_TERMINAL_DATA_MODE: c_int = 37;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalModeQuery {
    pub mode: GhosttyMode,
    pub value: bool,
}

pub const GHOSTTY_RENDER_STATE_DIRTY_FALSE: c_int = 0;
pub const GHOSTTY_RENDER_STATE_DIRTY_PARTIAL: c_int = 1;
pub const GHOSTTY_RENDER_STATE_DIRTY_FULL: c_int = 2;

pub const GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR: c_int = 0;
pub const GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK: c_int = 1;
pub const GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE: c_int = 2;
pub const GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW: c_int = 3;

pub const GHOSTTY_RENDER_STATE_DATA_COLS: c_int = 1;
pub const GHOSTTY_RENDER_STATE_DATA_ROWS: c_int = 2;
pub const GHOSTTY_RENDER_STATE_DATA_DIRTY: c_int = 3;
pub const GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR: c_int = 4;
pub const GHOSTTY_RENDER_STATE_DATA_CURSOR: c_int = 18;
pub const GHOSTTY_RENDER_STATE_DATA_COLORS: c_int = 19;

pub const GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY: c_int = 1;
pub const GHOSTTY_RENDER_STATE_ROW_DATA_RAW: c_int = 2;
pub const GHOSTTY_RENDER_STATE_ROW_DATA_CELLS: c_int = 3;
pub const GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION: c_int = 4;
pub const GHOSTTY_RENDER_STATE_ROW_DATA_VIEWPORT_Y: c_int = 6;
pub const GHOSTTY_RENDER_STATE_ROW_DATA_ID: c_int = 7;

pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW: c_int = 1;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE: c_int = 2;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN: c_int = 3;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF: c_int = 4;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8: c_int = 9;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR: c_int = 5;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR: c_int = 6;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_SELECTED: c_int = 7;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HAS_STYLING: c_int = 8;

pub const GHOSTTY_CELL_WIDE_NARROW: c_int = 0;
pub const GHOSTTY_CELL_WIDE_WIDE: c_int = 1;
pub const GHOSTTY_CELL_WIDE_SPACER_TAIL: c_int = 2;
pub const GHOSTTY_CELL_WIDE_SPACER_HEAD: c_int = 3;

pub const GHOSTTY_CELL_DATA_WIDE: c_int = 3;
pub const GHOSTTY_CELL_DATA_HAS_HYPERLINK: c_int = 7;
pub const GHOSTTY_ROW_DATA_WRAP: c_int = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GhosttyRenderStateRowId {
    pub bits: [u64; 2],
}

impl GhosttyRenderStateRowId {
    pub fn as_u128(self) -> u128 {
        (self.bits[0] as u128) | ((self.bits[1] as u128) << 64)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyRenderStateCursor {
    pub size: usize,
    pub viewport_has_value: bool,
    pub viewport_x: u16,
    pub viewport_y: u16,
    pub wide_tail: bool,
    pub visible: bool,
    pub blinking: bool,
    pub password_input: bool,
    pub visual_style: c_int,
}

impl Default for GhosttyRenderStateCursor {
    fn default() -> Self {
        // SAFETY: `GhosttyRenderStateCursor` is a `#[repr(C)]` POD struct of integers and bools
        // for which all-zeros is a valid bit pattern.
        let mut c: Self = unsafe { std::mem::zeroed() };
        c.size = std::mem::size_of::<Self>();
        c
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyRenderStateColors {
    pub size: usize,
    pub background: GhosttyColorRgb,
    pub foreground: GhosttyColorRgb,
    pub cursor: GhosttyColorRgb,
    pub cursor_has_value: bool,
    pub palette: [GhosttyColorRgb; 256],
}

impl Default for GhosttyRenderStateColors {
    fn default() -> Self {
        // SAFETY: `GhosttyRenderStateColors` is a `#[repr(C)]` POD struct of integers and bools
        // for which all-zeros is a valid bit pattern.
        let mut c: Self = unsafe { std::mem::zeroed() };
        c.size = std::mem::size_of::<Self>();
        c
    }
}

pub const GHOSTTY_KEY_ACTION_RELEASE: c_int = 0;
pub const GHOSTTY_KEY_ACTION_PRESS: c_int = 1;
pub const GHOSTTY_KEY_ACTION_REPEAT: c_int = 2;
pub const GHOSTTY_OPTION_AS_ALT_FALSE: c_int = 0;
pub const GHOSTTY_OPTION_AS_ALT_TRUE: c_int = 1;
pub const GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT: c_int = 6;

#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GhosttyKey {
    Unidentified = 0,
    Backquote = 1,
    Backslash = 2,
    BracketLeft = 3,
    BracketRight = 4,
    Comma = 5,
    Digit0 = 6,
    Digit1 = 7,
    Digit2 = 8,
    Digit3 = 9,
    Digit4 = 10,
    Digit5 = 11,
    Digit6 = 12,
    Digit7 = 13,
    Digit8 = 14,
    Digit9 = 15,
    Equal = 16,
    IntlBackslash = 17,
    IntlRo = 18,
    IntlYen = 19,
    A = 20,
    B = 21,
    C = 22,
    D = 23,
    E = 24,
    F = 25,
    G = 26,
    H = 27,
    I = 28,
    J = 29,
    K = 30,
    L = 31,
    M = 32,
    N = 33,
    O = 34,
    P = 35,
    Q = 36,
    R = 37,
    S = 38,
    T = 39,
    U = 40,
    V = 41,
    W = 42,
    X = 43,
    Y = 44,
    Z = 45,
    Minus = 46,
    Period = 47,
    Quote = 48,
    Semicolon = 49,
    Slash = 50,
    AltLeft = 51,
    AltRight = 52,
    Backspace = 53,
    CapsLock = 54,
    ContextMenu = 55,
    ControlLeft = 56,
    ControlRight = 57,
    Enter = 58,
    MetaLeft = 59,
    MetaRight = 60,
    ShiftLeft = 61,
    ShiftRight = 62,
    Space = 63,
    Tab = 64,
    Convert = 65,
    KanaMode = 66,
    NonConvert = 67,
    Delete = 68,
    End = 69,
    Help = 70,
    Home = 71,
    Insert = 72,
    PageDown = 73,
    PageUp = 74,
    ArrowDown = 75,
    ArrowLeft = 76,
    ArrowRight = 77,
    ArrowUp = 78,
    Escape = 120,
    F1 = 121,
    F2 = 122,
    F3 = 123,
    F4 = 124,
    F5 = 125,
    F6 = 126,
    F7 = 127,
    F8 = 128,
    F9 = 129,
    F10 = 130,
    F11 = 131,
    F12 = 132,
}

pub const GHOSTTY_MOUSE_ACTION_PRESS: c_int = 0;
pub const GHOSTTY_MOUSE_ACTION_RELEASE: c_int = 1;
pub const GHOSTTY_MOUSE_ACTION_MOTION: c_int = 2;
pub const GHOSTTY_MOUSE_BUTTON_UNKNOWN: c_int = 0;
pub const GHOSTTY_MOUSE_BUTTON_LEFT: c_int = 1;
pub const GHOSTTY_MOUSE_BUTTON_RIGHT: c_int = 2;
pub const GHOSTTY_MOUSE_BUTTON_MIDDLE: c_int = 3;
pub const GHOSTTY_MOUSE_BUTTON_FOUR: c_int = 4;
pub const GHOSTTY_MOUSE_BUTTON_FIVE: c_int = 5;
pub const GHOSTTY_MOUSE_BUTTON_SIX: c_int = 6;
pub const GHOSTTY_MOUSE_BUTTON_SEVEN: c_int = 7;
pub const GHOSTTY_MOUSE_ENCODER_OPT_SIZE: c_int = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyMousePosition {
    pub x: f32,
    pub y: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyMouseEncoderSize {
    pub size: usize,
    pub screen_width: u32,
    pub screen_height: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub padding_top: u32,
    pub padding_bottom: u32,
    pub padding_right: u32,
    pub padding_left: u32,
}

/// A search over a terminal's screens and scrollback (`vt/search.h`).
pub type GhosttySearch = *mut c_void;

pub const GHOSTTY_SEARCH_DATA_TOTAL_MATCHES: c_int = 2;
pub const GHOSTTY_SEARCH_DATA_SELECTED_INDEX: c_int = 3;
pub const GHOSTTY_SEARCH_DATA_SELECTED_MATCH: c_int = 4;
pub const GHOSTTY_SEARCH_DATA_VIEWPORT_MATCHES: c_int = 6;
pub const GHOSTTY_SEARCH_OPT_NEEDLE: c_int = 0;
pub const GHOSTTY_SEARCH_OPT_SELECT_NEXT: c_int = 1;
pub const GHOSTTY_SEARCH_OPT_SELECT_PREV: c_int = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttySelectionBuffer {
    pub ptr: *mut GhosttySelection,
    pub cap: usize,
    pub len: usize,
}

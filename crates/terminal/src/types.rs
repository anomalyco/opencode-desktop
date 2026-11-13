//! Shared snapshot, cell, cursor, scrollback, selection, and input types for `crates/terminal`.

use std::ffi::c_int;
use std::sync::Arc;

use crate::ffi::*;
use crate::theme::Rgb;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CursorShape {
    Bar,
    Block,
    Underline,
    HollowBlock,
}

impl CursorShape {
    pub(crate) fn from_render_style(v: c_int) -> Self {
        match v {
            GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK => Self::Block,
            GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => Self::Underline,
            GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW => Self::HollowBlock,
            _ => Self::Bar,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorSnapshot {
    pub in_viewport: bool,
    pub col: u16,
    pub row: u16,
    pub wide_tail: bool,
    pub visible: bool,
    pub blinking: bool,
    pub password_input: bool,
    pub shape: CursorShape,
}

impl Default for CursorSnapshot {
    fn default() -> Self {
        Self {
            in_viewport: true,
            col: 0,
            row: 0,
            wide_tail: false,
            visible: true,
            blinking: true,
            password_input: false,
            shape: CursorShape::Bar,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScrollbarSnapshot {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
    pub at_bottom: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum UnderlineKind {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

impl UnderlineKind {
    pub(crate) fn from_ffi(v: c_int) -> Self {
        match v {
            GHOSTTY_SGR_UNDERLINE_SINGLE => Self::Single,
            GHOSTTY_SGR_UNDERLINE_DOUBLE => Self::Double,
            GHOSTTY_SGR_UNDERLINE_CURLY => Self::Curly,
            GHOSTTY_SGR_UNDERLINE_DOTTED => Self::Dotted,
            GHOSTTY_SGR_UNDERLINE_DASHED => Self::Dashed,
            _ => Self::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CellFlags(pub u16);

impl CellFlags {
    pub const BOLD: Self = Self(1 << 0);
    pub const ITALIC: Self = Self(1 << 1);
    pub const FAINT: Self = Self(1 << 2);
    pub const BLINK: Self = Self(1 << 3);
    pub const INVERSE: Self = Self(1 << 4);
    pub const INVISIBLE: Self = Self(1 << 5);
    pub const STRIKETHROUGH: Self = Self(1 << 6);
    pub const OVERLINE: Self = Self(1 << 7);
    pub const WIDE: Self = Self(1 << 8);
    pub const WIDE_SPACER_TAIL: Self = Self(1 << 9);
    pub const HYPERLINK: Self = Self(1 << 10);

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedCell {
    pub grapheme: crate::grapheme::Grapheme,
    pub width: u8,
    pub fg: Rgb,
    pub bg: Option<Rgb>,
    pub underline_color: Option<Rgb>,
    pub underline: UnderlineKind,
    pub flags: CellFlags,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BgSpan {
    pub start_col: u16,
    pub end_col: u16,
    pub color: Option<Rgb>,
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalRow {
    pub id: u128,
    pub content_hash: u64,
    pub wrapped: bool,
    pub cells: Vec<RenderedCell>,
    pub bg_spans: Vec<BgSpan>,
    pub text: String,
}

impl TerminalRow {
    pub fn trimmed_text(&self) -> &str {
        self.text.trim_end_matches(' ')
    }
}

#[derive(Clone, Debug)]
pub struct TerminalSnapshot {
    pub cols: u16,
    pub rows: u16,
    pub screen_rows: Vec<Arc<TerminalRow>>,
    pub cursor: CursorSnapshot,
    pub scrollbar: ScrollbarSnapshot,
    pub fg: Rgb,
    pub bg: Rgb,
    pub cursor_color: Rgb,
    pub selection_alpha: f32,
    pub alt_screen: bool,
    pub mouse_tracking: bool,
    pub bracketed_paste: bool,
    pub focus_reporting: bool,
    pub kitty_keyboard_flags: u8,
    pub title: String,
    pub pwd: String,
    pub bell_count: u64,
    /// The find bar's matches, while it finds.
    pub find: Option<FindView>,
    /// Finished commands on the rows in view (OSC 133): each its prompt's row and how it ended.
    pub marks: Vec<(u16, crate::CommandMark)>,
}

/// What finding shows: the matches in all, which one is selected (0 is the newest), and the
/// cells of those in view.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FindView {
    pub total: usize,
    pub selected: Option<usize>,
    pub hits: Vec<FindHit>,
}

/// One row's run of a match in view: columns `start..=end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FindHit {
    pub row: u16,
    pub start: u16,
    pub end: u16,
    pub selected: bool,
}

impl TerminalSnapshot {
    /// An empty screen of `cols` by `rows` in `theme`, before any output.
    pub fn blank(cols: u16, rows: u16, theme: &crate::theme::TerminalTheme) -> TerminalSnapshot {
        TerminalSnapshot {
            cols,
            rows,
            screen_rows: Vec::new(),
            cursor: CursorSnapshot::default(),
            scrollbar: ScrollbarSnapshot { total: u64::from(rows), offset: 0, len: u64::from(rows), at_bottom: true },
            fg: theme.fg,
            bg: theme.bg,
            cursor_color: theme.cursor,
            selection_alpha: theme.selection_alpha,
            alt_screen: false,
            mouse_tracking: false,
            bracketed_paste: false,
            focus_reporting: false,
            kitty_keyboard_flags: 0,
            title: String::new(),
            pwd: String::new(),
            bell_count: 0,
            find: None,
            marks: Vec::new(),
        }
    }

    pub fn line_text(&self, row: usize) -> &str {
        self.screen_rows.get(row).map_or("", |r| r.trimmed_text())
    }

    pub fn viewport_text(&self) -> String {
        let mut lines: Vec<&str> = self.screen_rows.iter().map(|r| r.trimmed_text()).collect();
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollOp {
    Delta(isize),
    Top,
    Bottom,
    Row(usize),
    /// To the `n`th shell prompt above (negative) or below, from the shell's OSC 133 marks.
    Prompt(isize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridPoint {
    Viewport { col: u16, row: u16 },
    Screen { col: u16, row: u32 },
    Active { col: u16, row: u16 },
}

impl GridPoint {
    pub(crate) fn to_ffi(self) -> GhosttyPoint {
        match self {
            Self::Viewport { col, row } => {
                GhosttyPoint::new(GHOSTTY_POINT_TAG_VIEWPORT, col, u32::from(row))
            }
            Self::Screen { col, row } => GhosttyPoint::new(GHOSTTY_POINT_TAG_SCREEN, col, row),
            Self::Active { col, row } => {
                GhosttyPoint::new(GHOSTTY_POINT_TAG_ACTIVE, col, u32::from(row))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionOp {
    Range {
        start: GridPoint,
        end: GridPoint,
        rectangular: bool,
    },
    Word(GridPoint),
    Line(GridPoint),
    All,
    Clear,
    /// Moves the selection's end to the point (Shift+click): from whichever end stays put, so a
    /// point before the selection grows it backwards.
    Extend(GridPoint),
    /// Moves the selection's end by cells and rows (Shift+arrows), across line ends.
    Nudge { dx: i16, dy: i16 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    Release,
    Press,
    Repeat,
}

#[derive(Clone, Debug)]
pub struct KeyInput {
    pub key: GhosttyKey,
    pub action: KeyAction,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub cmd: bool,
    pub text: Option<String>,
    pub unshifted_codepoint: Option<u32>,
}

impl KeyInput {
    pub fn press(key: GhosttyKey) -> Self {
        Self {
            key,
            action: KeyAction::Press,
            shift: false,
            ctrl: false,
            alt: false,
            cmd: false,
            text: None,
            unshifted_codepoint: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    Press,
    Release,
    Move,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseBtn {
    None,
    Left,
    Right,
    Middle,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
}

#[derive(Clone, Copy, Debug)]
pub struct MouseInput {
    pub kind: MouseKind,
    pub button: MouseBtn,
    pub x_px: f32,
    pub y_px: f32,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub cmd: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VtEvent {
    Bell,
    TitleChanged(String),
    PwdChanged(String),
    RenderHold(bool),
}

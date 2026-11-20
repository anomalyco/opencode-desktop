//! Desktop's three dialog sizes (`overlays/dialog/dialog.css`, `dialog.tsx`; anomalyco/opencode
//! #63432, #64206, #64882). Every dialog takes one; none sets its own width or height (`scripts/dialog-size.sh`).
//!
//! - `Normal`: 480 wide, 368 tall, for confirmations, sign-ins and small forms.
//! - `Large`: 680 × 600, for palettes, pickers and lists.
//! - `XLarge`: 880 × 680, for long documents and settings-like panels.
//!
//! Each clamps to the window. A dialog that fits its content grows up to its size's height; a
//! large one, or an anchored normal one, keeps its top edge where the full height would put it,
//! so only its bottom edge moves as the content changes.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DialogSize {
    #[default]
    Normal,
    Large,
    XLarge,
}

impl DialogSize {
    /// `width: min(calc(100vw - 32px), …)`.
    pub fn width(self, win_w: f32) -> f32 {
        let w = match self {
            DialogSize::Normal => 480.0,
            DialogSize::Large => 680.0,
            DialogSize::XLarge => 880.0,
        };
        (win_w - 32.0).min(w).max(0.0)
    }

    /// `--dialog-height`: the fixed height, and the most a fitting dialog grows to.
    pub fn height(self, win_h: f32) -> f32 {
        match self {
            DialogSize::Normal => (win_h - 32.0).min(368.0),
            DialogSize::Large => (win_h - 96.0).min(600.0),
            DialogSize::XLarge => (win_h - 96.0).min(680.0),
        }
        .max(0.0)
    }

    /// The most a fitting dialog grows to: its height when it keeps its top edge, else the
    /// window less 32 (`max-height: calc(100vh - 32px)`).
    pub fn max_height(self, win_h: f32, anchored: bool) -> f32 {
        if anchored || self != DialogSize::Normal { self.height(win_h) } else { (win_h - 32.0).max(0.0) }
    }

    /// The kept top edge (`margin-top: calc((100vh - var(--dialog-height)) / 2)`).
    pub fn top(self, win_h: f32) -> f32 {
        ((win_h - self.height(win_h)) / 2.0).max(0.0)
    }
}

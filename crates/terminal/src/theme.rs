//! Terminal color types and theme palette generation (`libghostty-vt` harmonious 256-color ramp).

use rune::color::{srgb_decode, srgb_encode};

use crate::ffi::{self, GhosttyColorRgb};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn to_rune(self) -> rune::Color {
        rune::Color {
            r: srgb_decode(f32::from(self.r) / 255.0),
            g: srgb_decode(f32::from(self.g) / 255.0),
            b: srgb_decode(f32::from(self.b) / 255.0),
            a: 1.0,
        }
    }

    pub fn from_rune(c: rune::Color) -> Self {
        let enc = |v: f32| (srgb_encode(v.clamp(0.0, 1.0)) * 255.0).round() as u8;
        Self {
            r: enc(c.r),
            g: enc(c.g),
            b: enc(c.b),
        }
    }

    pub(crate) fn to_ffi(self) -> GhosttyColorRgb {
        GhosttyColorRgb { r: self.r, g: self.g, b: self.b }
    }

    pub(crate) fn from_ffi(c: GhosttyColorRgb) -> Self {
        Self { r: c.r, g: c.g, b: c.b }
    }
}

/// Terminal theme derived from the host app's palette (`packages/gui-extensions/src/terminal/terminal.tsx`).
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalTheme {
    pub dark: bool,
    pub fg: Rgb,
    pub bg: Rgb,
    pub cursor: Rgb,
    /// Selection alpha in `[0.0, 1.0]` (`0.25` in dark, `0.20` in light, matching Desktop).
    pub selection_alpha: f32,
    pub ansi_base: [Rgb; 16],
}

impl Default for TerminalTheme {
    fn default() -> Self {
        Self::opencode_dark()
    }
}

impl TerminalTheme {
    /// OpenCode Desktop's dark fallback (`#191515` bg, `#d4d4d4` fg) + balanced ANSI 16 seeds.
    pub fn opencode_dark() -> Self {
        Self {
            dark: true,
            bg: Rgb::new(0x19, 0x15, 0x15),
            fg: Rgb::new(0xd4, 0xd4, 0xd4),
            cursor: Rgb::new(0xd4, 0xd4, 0xd4),
            selection_alpha: 0.25,
            ansi_base: [
                Rgb::new(0x1d, 0x1f, 0x21),
                Rgb::new(0xe0, 0x6c, 0x75),
                Rgb::new(0x98, 0xc3, 0x79),
                Rgb::new(0xe5, 0xc0, 0x7b),
                Rgb::new(0x61, 0xaf, 0xef),
                Rgb::new(0xc6, 0x78, 0xdd),
                Rgb::new(0x56, 0xb6, 0xc2),
                Rgb::new(0xc8, 0xcc, 0xd4),
                Rgb::new(0x5c, 0x63, 0x70),
                Rgb::new(0xf0, 0x80, 0x88),
                Rgb::new(0xb5, 0xd6, 0x9c),
                Rgb::new(0xf0, 0xd1, 0x97),
                Rgb::new(0x82, 0xc2, 0xf7),
                Rgb::new(0xd7, 0x99, 0xea),
                Rgb::new(0x78, 0xc9, 0xd3),
                Rgb::new(0xf2, 0xf4, 0xf8),
            ],
        }
    }

    /// OpenCode Desktop's light fallback (`#fcfcfc` bg, `#211e1e` fg) + readable light ANSI 16 seeds.
    pub fn opencode_light() -> Self {
        Self {
            dark: false,
            bg: Rgb::new(0xfc, 0xfc, 0xfc),
            fg: Rgb::new(0x21, 0x1e, 0x1e),
            cursor: Rgb::new(0x21, 0x1e, 0x1e),
            selection_alpha: 0.20,
            ansi_base: [
                Rgb::new(0x21, 0x1e, 0x1e),
                Rgb::new(0xc9, 0x3b, 0x43),
                Rgb::new(0x2e, 0x7d, 0x32),
                Rgb::new(0x9e, 0x6a, 0x03),
                Rgb::new(0x1f, 0x6f, 0xeb),
                Rgb::new(0x89, 0x3f, 0xb2),
                Rgb::new(0x0b, 0x7c, 0x8c),
                Rgb::new(0x6e, 0x77, 0x81),
                Rgb::new(0x6e, 0x77, 0x81),
                Rgb::new(0xd9, 0x48, 0x50),
                Rgb::new(0x38, 0x8e, 0x3c),
                Rgb::new(0xb5, 0x7b, 0x05),
                Rgb::new(0x2b, 0x7f, 0xff),
                Rgb::new(0x9c, 0x51, 0xc7),
                Rgb::new(0x15, 0x90, 0xa2),
                Rgb::new(0x24, 0x29, 0x2f),
            ],
        }
    }

    /// The 256-colour table: the theme's 16 colours, then xterm's 6x6x6 cube and grey ramp, as
    /// Desktop's terminal (ghostty-web) and every xterm-compatible terminal have them, so a
    /// program's `38;5;196` is the red it asked for.
    pub(crate) fn full_palette_ffi(&self) -> [GhosttyColorRgb; 256] {
        let mut out = [GhosttyColorRgb::default(); 256];
        // SAFETY: `out` is a valid, aligned 256-element `GhosttyColorRgb` array as required by
        // `ghostty_color_palette_default`.
        unsafe { ffi::ghostty_color_palette_default(out.as_mut_ptr()) };
        for (i, c) in self.ansi_base.iter().enumerate() {
            out[i] = c.to_ffi();
        }
        out
    }
}

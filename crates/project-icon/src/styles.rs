//! Procedural marks for projects without an image. Each is seeded by the project's name,
//! built from the palette's avatar colours, and drawn on the CPU into a rounded tile at the
//! device size it shows at.

use rune::Color;

use crate::Palette;
use crate::pixels::Buf;

/// The mark for projects without an image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    /// Desktop's look: the first letter on the avatar colour (gray unless one is chosen), lit
    /// from above.
    Desktop,
    /// A two-stop diagonal blend between neighbouring avatar hues.
    Gradient,
    /// Soft colour waves on a gently folded, lit plane (Desktop's experimental generated mark).
    Mesh,
    /// Three hues swept around a point.
    Conic,
    /// A diagonal blend under coarse film grain.
    Grain,
    /// Soft blobs of neighbouring hues on the avatar colour.
    Blob,
    /// The first letter on a diagonal blend.
    Monogram,
    /// One small geometric shape on the avatar colour.
    Glyph,
    /// A mirrored 3×3 pattern on the avatar colour.
    Quilt,
    /// Concentric rings from a corner.
    Rings,
    /// A slanted band across the avatar colour.
    Band,
    /// A low sun on a horizon.
    Horizon,
    /// A mirrored 5×5 pixel sprite on a bright tile, snapped to whole device pixels.
    Pixel,
    /// A wobbly blob with a white die-cut edge and a hard shadow, like a laptop sticker.
    Sticker,
    /// A heavy first letter with a hard offset shadow, halftone dots when it's big.
    Pop,
    /// Four Bauhaus blocks: quarter circles, discs and triangles in red, yellow, blue and ink.
    Bauhaus,
    /// A glossy two-hue ball with a highlight and a contact shadow.
    Orb,
    /// A hand-drawn wave, zigzag or loop and a dot on a bright tile.
    Squiggle,
    /// A mirrored little invader with eyes, on whole device pixels.
    Sprite,
    /// A 1-bit ordered-dither (Bayer) glow in a dark and a light tone of the hue.
    Dither,
    /// The first letter in a 5×7 bitmap font, with a hard drop shadow.
    Bitmap,
    /// Quarter-circle Truchet tiles, 3×3.
    Truchet,
    /// A tiny face from seeded eyes and mouth.
    Face,
    /// Lopsided lava-lamp metaballs.
    Lava,
    /// A glowing tube shape on a dark tile, with scan lines.
    Neon,
    /// A Tamagotchi-style critter: seeded body, ears or antennae, eyes, mouth and feet.
    Critter,
    /// A mushroom, ghost, slime or robot in PICO-8 colours.
    Monster,
    /// A potion, gem, heart or sword in NES colours.
    Loot,
    /// A critter in the Game Boy's four greens, on a dithered ground.
    GameBoy,
    /// A space invader in CGA cyan and magenta on black.
    Cga,
    /// Two initials in a 3×5 pixel font, light on the hue, with a pixel shadow.
    Duo,
    /// A bold 5×7 initial in the hue with an ink outline, on a pale tile.
    Chunky,
    /// A 1-bit dithered landscape: sky, sun and hills in two tones of the hue.
    Dusk,
    /// An 8×8 Bayer-dithered blend between two PICO-8 colours.
    Blend,
    /// Error-diffused (Floyd–Steinberg) 1-bit grain of a seeded glow.
    Static,
}

/// A group of styles in the playground.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Gradient,
    Geometric,
    Pixel,
    Playful,
}

impl Family {
    pub const ALL: [Family; 4] = [Family::Gradient, Family::Geometric, Family::Pixel, Family::Playful];

    pub fn name(self) -> &'static str {
        match self {
            Family::Gradient => "Gradient",
            Family::Geometric => "Geometric",
            Family::Pixel => "Pixel",
            Family::Playful => "Playful",
        }
    }

    pub fn styles(self) -> impl Iterator<Item = Style> {
        Style::ALL.into_iter().filter(move |s| s.family() == self)
    }
}

impl Style {
    pub const ALL: [Style; 35] = [
        Style::Desktop,
        Style::Gradient,
        Style::Mesh,
        Style::Conic,
        Style::Grain,
        Style::Blob,
        Style::Monogram,
        Style::Glyph,
        Style::Quilt,
        Style::Rings,
        Style::Band,
        Style::Horizon,
        Style::Pixel,
        Style::Sticker,
        Style::Pop,
        Style::Bauhaus,
        Style::Orb,
        Style::Squiggle,
        Style::Sprite,
        Style::Dither,
        Style::Bitmap,
        Style::Truchet,
        Style::Face,
        Style::Lava,
        Style::Neon,
        Style::Critter,
        Style::Monster,
        Style::Loot,
        Style::GameBoy,
        Style::Cga,
        Style::Duo,
        Style::Chunky,
        Style::Dusk,
        Style::Blend,
        Style::Static,
    ];

    pub const NAMES: [&str; 35] = [
        "Desktop", "Gradient", "Mesh", "Conic", "Grain", "Blob", "Monogram", "Glyph", "Quilt", "Rings", "Band", "Horizon", "Pixel", "Sticker", "Pop", "Bauhaus", "Orb",
        "Squiggle", "Sprite", "Dither", "Bitmap", "Truchet", "Face", "Lava", "Neon",
        "Critter", "Monster", "Loot", "GameBoy", "CGA", "Duo", "Chunky", "Dusk", "Blend", "Static",
    ];

    pub fn family(self) -> Family {
        match self {
            Style::Desktop | Style::Gradient | Style::Mesh | Style::Conic | Style::Grain | Style::Blob | Style::Monogram | Style::Orb => Family::Gradient,
            Style::Glyph | Style::Quilt | Style::Rings | Style::Band | Style::Horizon | Style::Bauhaus | Style::Truchet => Family::Geometric,
            Style::Pixel
            | Style::Sprite
            | Style::Dither
            | Style::Bitmap
            | Style::Critter
            | Style::Monster
            | Style::Loot
            | Style::GameBoy
            | Style::Cga
            | Style::Duo
            | Style::Chunky
            | Style::Dusk
            | Style::Blend
            | Style::Static => Family::Pixel,
            Style::Sticker | Style::Pop | Style::Squiggle | Style::Face | Style::Lava | Style::Neon => Family::Playful,
        }
    }

    pub fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }

    /// A style by name, ignoring case.
    pub fn from_name(name: &str) -> Option<Style> {
        Self::ALL.into_iter().find(|s| s.name().eq_ignore_ascii_case(name))
    }

    /// Drawn with a letter on top.
    pub fn lettered(self) -> bool {
        matches!(self, Style::Desktop | Style::Monogram)
    }
}

/// A style and the knobs that shape it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub style: Style,
    /// 1 keeps the palette's colours, 0 is gray, above 1 is more vivid.
    pub saturation: f32,
    /// Film grain strength for Grain (Mesh takes half).
    pub grain: f32,
}

impl Default for Look {
    fn default() -> Look {
        Look { style: Style::Gradient, saturation: 1.0, grain: 0.12 }
    }
}

impl Look {
    pub fn style(style: Style) -> Look {
        Look { style, ..Look::default() }
    }

    pub(crate) fn key(&self) -> (Style, i32, i32) {
        (self.style, (self.saturation * 100.0).round() as i32, (self.grain * 100.0).round() as i32)
    }
}

pub(crate) fn fnv(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &c| (h ^ c as u64).wrapping_mul(0x0100_0000_01b3))
}

pub(crate) fn hash(s: &str) -> u64 {
    mix(fnv(s.as_bytes()))
}

pub(crate) fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// `Palette::avatars` indices around the colour wheel: red, orange, yellow, green, cyan, blue,
/// purple, pink. Gray (8) is off the wheel.
const WHEEL: [usize; 8] = [4, 0, 1, 3, 2, 6, 7, 5];

/// The avatar colour `steps` around the wheel from `variant`.
fn around(variant: usize, steps: i32) -> usize {
    let at = WHEEL.iter().position(|&i| i == variant).unwrap_or(0) as i32;
    WHEEL[(at + steps).rem_euclid(8) as usize]
}

pub(crate) type Rgb = [f32; 3];

pub(crate) fn rgb(c: Color) -> Rgb {
    [c.r, c.g, c.b]
}

pub(crate) fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

pub(crate) fn saturate(c: Rgb, s: f32) -> Rgb {
    let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    c.map(|v| (l + (v - l) * s).clamp(0.0, 1.0))
}

pub(crate) fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn unit(h: u64, shift: u32) -> f32 {
    ((h >> shift) & 0xffff) as f32 / 65535.0
}

/// The GLSL-style lattice hash Desktop's mesh uses.
fn lattice(x: f32, y: f32) -> f32 {
    let fract = |v: f32| v - v.floor();
    let (mut px, mut py) = (fract(x * 123.34), fract(y * 456.21));
    let d = px * (px + 45.32) + py * (py + 45.32);
    px += d;
    py += d;
    fract(px * py)
}

fn noise(x: f32, y: f32) -> f32 {
    let (ix, iy) = (x.floor(), y.floor());
    let (fx, fy) = (x - ix, y - iy);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let (a, b, c, d) = (lattice(ix, iy), lattice(ix + 1.0, iy), lattice(ix, iy + 1.0), lattice(ix + 1.0, iy + 1.0));
    a + (b - a) * sx + (c - a + (d - c - b + a) * sx) * sy
}

/// Signed distance to a rounded box centred at the origin.
pub(crate) fn round_box(x: f32, y: f32, half: f32, r: f32) -> f32 {
    let (qx, qy) = (x.abs() - half + r, y.abs() - half + r);
    (qx.max(0.0).hypot(qy.max(0.0))) + qx.max(qy).min(0.0) - r
}

/// The tile's corner radius as a fraction of its side (4 pt at 16 pt).
pub(crate) const RADIUS: f32 = 0.25;

/// A mark's pixels as straight-alpha sRGB, `px` square, for a tile `size_pt` points wide.
pub(crate) fn draw(p: &Palette, seed: &str, variant: usize, look: &Look, size_pt: f32, px: u32) -> Vec<u8> {
    if crate::funky::funky(look.style) {
        return crate::funky::draw(look.style, seed, size_pt, px);
    }
    if crate::sprites::owns(look.style) {
        return crate::sprites::draw(p, seed, variant, look, px);
    }
    let h = hash(seed);
    let dir = if h >> 8 & 1 == 0 { 1 } else { -1 };
    let s = look.saturation;
    let a = saturate(rgb(p.avatars[variant].0), s);
    let b = saturate(rgb(p.avatars[around(variant, dir)].0), s);
    let c = saturate(rgb(p.avatars[around(variant, 2 * dir)].0), s);
    let edge = saturate(rgb(p.avatars[variant].1), s);
    let fg = rgb(p.fg);
    let style = look.style;
    let letter = seed.chars().find(|c| c.is_alphanumeric()).unwrap_or('?');
    let retro = crate::retro::Ctx { h, a, b, c, dark: p.dark, px, sat: s, letter };
    // A hairline ring half a point wide, in tile units.
    let ring = 0.5 / size_pt;
    let angle = (h >> 16 & 7) as f32 * std::f32::consts::FRAC_PI_4;
    let diag = |x: f32, y: f32| {
        let (dx, dy) = (angle.cos(), angle.sin());
        smooth(0.0, 1.0, (x * dx + y * dy) / std::f32::consts::SQRT_2 + 0.5)
    };
    let shade = |u: f32, v: f32| -> [f32; 4] {
        let (x, y) = (u - 0.5, v - 0.5);
        let d = round_box(x, y, 0.5, RADIUS);
        if d > 0.0 {
            return [0.0; 4];
        }
        let mut col: Rgb = match style {
            Style::Desktop => a,
            Style::Gradient | Style::Monogram => {
                let col = lerp(a, b, diag(x, y));
                // A soft light from one top corner.
                let (hx, hy) = (if h & 1 == 0 { 0.2 } else { 0.8 }, 0.18);
                let glow = (1.0 - (u - hx).hypot(v - hy) / 0.75).clamp(0.0, 1.0);
                let col = lerp(col, fg, 0.16 * glow * glow);
                if style == Style::Monogram { col.map(|v| v * 0.82) } else { col }
            }
            Style::Grain => {
                let col = lerp(a, b, diag(x, y));
                let across = (size_pt * 0.9).clamp(12.0, 28.0);
                let (gx, gy) = ((u * across).floor(), (v * across).floor());
                let n = lattice(gx + unit(h, 40) * 97.0, gy + unit(h, 24) * 61.0) - 0.5;
                col.map(|v| (v * (1.0 + n * 2.6 * look.grain)).clamp(0.0, 1.0))
            }
            Style::Mesh => {
                let (sx, sy) = (unit(h, 0) * 40.0, unit(h, 20) * 40.0);
                let (mx, my) = (u * 2.3 + sx, (1.0 - v) * 2.3 + sy);
                let mut col = a;
                for (i, ink) in [b, c, edge].into_iter().enumerate() {
                    let i = i as f32;
                    let (nx, ny) = (mx * (0.35 + 0.12 * i), my * 0.6 + i * 3.7);
                    let n = 0.65 * noise(nx, ny) + 0.35 * noise(nx * 2.1 + 7.0, ny * 2.1 + 7.0);
                    col = lerp(col, ink, smooth(0.35, 0.75, n) * 0.85);
                }
                let height = |x: f32, y: f32| (0.7 * noise(x * 0.45, y * 0.45) + 0.3 * noise(x * 1.03 + 3.0, y * 1.03 + 3.0)) * 0.4;
                let e = 0.02;
                let h0 = height(mx, my);
                let (nx, ny, nz) = (-(height(mx + e, my) - h0) / e, -(height(mx, my + e) - h0) / e, 1.4);
                let len = (nx * nx + ny * ny + nz * nz).sqrt();
                let lit = (nx * -0.42 + ny * 0.63 + nz * 0.74) / len;
                let shade = 0.78 + 0.38 * lit;
                let gloss = lit.max(0.0).powf(18.0) * 0.2;
                let (gx, gy) = ((u * 16.0).floor(), (v * 16.0).floor());
                let grain = (lattice(gx + 11.0, gy + 5.0) - 0.5) * look.grain;
                col.map(|v| (v * shade + gloss).clamp(0.0, 1.0) * (1.0 + grain))
            }
            Style::Conic => {
                let (cx, cy) = (0.5 + (unit(h, 0) - 0.5) * 0.4, 0.5 + (unit(h, 16) - 0.5) * 0.4);
                let turn = (v - cy).atan2(u - cx) / std::f32::consts::TAU + unit(h, 32);
                let w = |k: f32| {
                    let c = (0.5 + 0.5 * ((turn - k) * std::f32::consts::TAU).cos()).powf(1.6);
                    c.max(1e-4)
                };
                let (wa, wb, wc) = (w(0.0), w(1.0 / 3.0), w(2.0 / 3.0));
                let sum = wa + wb + wc;
                let col: Rgb = std::array::from_fn(|i| (a[i] * wa + b[i] * wb + c[i] * wc) / sum);
                let near = (1.0 - (u - cx).hypot(v - cy) / 0.5).clamp(0.0, 1.0);
                lerp(col, fg, 0.12 * near * near)
            }
            Style::Blob => {
                let mut col = a.map(|v| v * 0.85);
                for (i, ink) in [b, c, edge].into_iter().enumerate() {
                    let k = 13 * i as u32;
                    let (bx, by) = (0.1 + 0.8 * unit(h, k), 0.1 + 0.8 * unit(h, k + 7));
                    let r = 0.32 + 0.2 * unit(h, (k + 29) % 48);
                    let t = (-((u - bx).powi(2) + (v - by).powi(2)) / (r * r)).exp();
                    col = lerp(col, ink, 0.8 * t);
                }
                col
            }
            Style::Glyph | Style::Quilt => a,
            Style::Rings => {
                let (cx, cy) = (if h & 1 == 0 { 0.0 } else { 1.0 }, if h & 2 == 0 { 0.0 } else { 1.0 });
                let r = (u - cx).hypot(v - cy) / 0.2 + unit(h, 12) * 0.6;
                if r.floor() as i32 & 1 == 0 { lerp(a, b, 0.6) } else { a }
            }
            Style::Band => {
                let slope = [0.6f32, -0.6, 1.4, -1.4][(h >> 12 & 3) as usize];
                let off = (unit(h, 20) - 0.5) * 0.3;
                let t = (y - x * slope) / (1.0 + slope * slope).sqrt() - off;
                if t.abs() < 0.14 {
                    b
                } else if (t - 0.27).abs() < 0.05 {
                    c
                } else {
                    a
                }
            }
            Style::Pixel | Style::Sticker | Style::Pop | Style::Bauhaus | Style::Orb | Style::Squiggle => a,
            Style::Horizon => {
                let horizon = 0.62 + (unit(h, 4) - 0.5) * 0.12;
                let sx = 0.5 + (unit(h, 28) - 0.5) * 0.36;
                if v > horizon {
                    lerp(b, a, 0.35).map(|v| v * 0.8)
                } else if (u - sx).hypot(v - horizon) < 0.24 {
                    lerp(fg, b, 0.3)
                } else {
                    lerp(lerp(a, edge, 0.35), a, v / horizon)
                }
            }
            _ => crate::retro::shade(style, &retro, u, v),
        };
        if style == Style::Desktop {
            // Desktop's sheen: white at 16% at the top, fading out at the bottom.
            col = lerp(col, [1.0; 3], 0.16 * (1.0 - v));
        }
        let ink = match style {
            Style::Glyph => glyph(h, x * 16.0, y * 16.0),
            Style::Quilt => quilt(h, u * 16.0, v * 16.0),
            _ => 0.0,
        };
        if ink > 0.0 {
            col = lerp(col, fg, 0.92 * ink);
        }
        if d > -ring {
            let strength = match style {
                Style::Desktop | Style::Glyph | Style::Quilt => 1.0,
                Style::Neon | Style::Dither => 0.2,
                _ => 0.45,
            };
            col = lerp(col, edge, strength);
        }
        [col[0], col[1], col[2], 1.0]
    };
    let n = if px <= 48 { 4 } else { 2 };
    let mut buf = Buf::new(px, px);
    for py in 0..px {
        for x in 0..px {
            let mut acc = [0.0f32; 4];
            for sy in 0..n {
                for sx in 0..n {
                    let u = (x as f32 + (sx as f32 + 0.5) / n as f32) / px as f32;
                    let v = (py as f32 + (sy as f32 + 0.5) / n as f32) / px as f32;
                    let [r, g, b, alpha] = shade(u, v);
                    acc = [acc[0] + r * alpha, acc[1] + g * alpha, acc[2] + b * alpha, acc[3] + alpha];
                }
            }
            buf.px[(py * px + x) as usize] = acc.map(|v| v / (n * n) as f32);
        }
    }
    buf.to_rgba8()
}

/// One of eight small shapes, centred, in 16 pt tile points: 1 inside, 0 outside.
fn glyph(h: u64, x: f32, y: f32) -> f32 {
    let inside = |b: bool| if b { 1.0 } else { 0.0 };
    match (h >> 24) % 8 {
        0 => inside(x.hypot(y) <= 2.75),
        1 => inside((x.hypot(y) - 2.9).abs() <= 0.8),
        2 => inside(x.abs() + y.abs() <= 3.4),
        3 => inside(y <= 2.4 && y >= -3.0 && x.abs() <= (y + 3.0) * 0.62),
        4 => inside(round_box(x, y, 2.6, 0.9) <= 0.0),
        5 => inside(x.hypot(y + 1.2) <= 3.3 && y >= -1.2),
        6 => inside((x - 1.9).hypot(y) <= 1.6 || (x + 1.9).hypot(y) <= 1.6),
        _ => inside((x.abs() <= 0.85 && y.abs() <= 3.2) || (y.abs() <= 0.85 && x.abs() <= 3.2)),
    }
}

/// A mirrored 3×3 grid of cells inside a 2.5 pt margin; at least three cells are filled.
fn quilt(h: u64, x: f32, y: f32) -> f32 {
    let (m, cell) = (2.5, 11.0 / 3.0);
    let (gx, gy) = ((x - m) / cell, (y - m) / cell);
    if !(0.0..3.0).contains(&gx) || !(0.0..3.0).contains(&gy) {
        return 0.0;
    }
    let (cx, cy) = (gx as usize, gy as usize);
    // Six free cells (two columns, three rows); the third column mirrors the first.
    let mut bits = (h >> 32) as u32 & 0x3f;
    if bits.count_ones() < 3 {
        bits |= 0b010_101;
    }
    let col = if cx == 2 { 0 } else { cx };
    if bits >> (cy * 2 + col) & 1 == 0 {
        return 0.0;
    }
    let (fx, fy) = (gx.fract(), gy.fract());
    let gap = 0.08;
    if fx < gap || fx > 1.0 - gap || fy < gap || fy > 1.0 - gap { 0.0 } else { 1.0 }
}

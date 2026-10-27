//! More loud marks, from the project's avatar hue at full strength: an invader sprite, a dithered
//! glow, a bitmap letter, Truchet tiles, a face, lava and neon. Sprite, Dither and Bitmap work in
//! whole device pixels, so their edges stay sharp at 1× and 2×.

use crate::pixels::lin;
use crate::styles::{Rgb, Style, lerp, mix, round_box, saturate, smooth};

/// What a mark's shader knows besides the sample point.
pub(crate) struct Ctx {
    pub h: u64,
    pub a: Rgb,
    pub b: Rgb,
    pub c: Rgb,
    pub dark: bool,
    pub px: u32,
    pub sat: f32,
    pub letter: char,
}

const BLACK: Rgb = [0.0; 3];
const WHITE: Rgb = [1.0; 3];

fn srgb(r: u8, g: u8, b: u8) -> Rgb {
    [r, g, b].map(|v| lin(v as f32 / 255.0))
}

fn lum(c: Rgb) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// The hue at full strength: its brightest channel at 1.
fn vivid(c: Rgb, s: f32) -> Rgb {
    let m = c[0].max(c[1]).max(c[2]).max(1e-3);
    saturate(c.map(|v| v / m), 1.2 * s)
}

impl Ctx {
    fn bits(&self, k: u64) -> u64 {
        mix(self.h ^ k.wrapping_mul(0x9e37_79b9_7f4a_7c15))
    }

    fn unit(&self, k: u64) -> f32 {
        (self.bits(k) & 0xffff) as f32 / 65535.0
    }

    /// A pale tile in light mode and a deep one in dark, tinted by `c`.
    fn ground(&self, c: Rgb) -> Rgb {
        if self.dark { lerp(c, BLACK, 0.8) } else { lerp(c, WHITE, 0.82) }
    }

    /// The cell of an `n`-cell grid whose cells are `px / div` device pixels, centred.
    fn cell(&self, div: u32, n: i32, ix: i32, iy: i32) -> Option<(i32, i32, i32)> {
        let cell = (self.px / div).max(1) as i32;
        let off = (self.px as i32 - cell * n) / 2;
        let (gx, gy) = ((ix - off).div_euclid(cell), (iy - off).div_euclid(cell));
        ((0..n).contains(&gx) && (0..n).contains(&gy)).then_some((gx, gy, cell))
    }
}

/// The colour at `(u, v)` (0–1 across the tile), for the funky styles.
pub(crate) fn shade(style: Style, k: &Ctx, u: f32, v: f32) -> Rgb {
    let (ix, iy) = ((u * k.px as f32) as i32, (v * k.px as f32) as i32);
    let (x, y) = (u - 0.5, v - 0.5);
    let (av, bv, cv) = (vivid(k.a, k.sat), vivid(k.b, k.sat), vivid(k.c, k.sat));
    match style {
        Style::Sprite => {
            let bg = k.ground(cv);
            let Some((gx, gy, _)) = k.cell(9, 7, ix, iy) else { return bg };
            // Columns from the outside in (3 is the middle); the right half mirrors the left.
            let col = gx.min(6 - gx);
            let eye = gy == 2 && col == 2;
            let body = (1..=4).contains(&gy) && col >= 1;
            let on = !eye && (body || k.bits(5) >> (gy * 4 + col) & 1 == 1);
            if !on {
                bg
            } else if gy >= 5 {
                bv
            } else {
                av
            }
        }
        Style::Dither => {
            const BAYER: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
            let dot = (k.px / 16).max(1) as i32;
            let (dx, dy) = (ix / dot, iy / dot);
            let t = if k.bits(6) & 1 == 0 {
                let (lx, ly) = (0.2 + 0.6 * k.unit(7), 0.2 + 0.4 * k.unit(8));
                1.0 - (u - lx).hypot(v - ly) / 0.85
            } else {
                let angle = k.unit(9) * std::f32::consts::TAU;
                0.5 + (x * angle.cos() + y * angle.sin()) * 1.1
            };
            let threshold = (BAYER[((dy & 3) * 4 + (dx & 3)) as usize] as f32 + 0.5) / 16.0;
            if t.clamp(0.0, 1.0) > threshold { lerp(av, WHITE, 0.3) } else { lerp(av, BLACK, 0.8) }
        }
        Style::Bitmap => {
            let base = if k.dark { av.map(|c| c * 0.42) } else { av };
            let cell = (k.px / 10).max(1) as i32;
            let (ox, oy) = ((k.px as i32 - 6 * cell) / 2, (k.px as i32 - 8 * cell) / 2);
            let (gx, gy) = ((ix - ox).div_euclid(cell), (iy - oy).div_euclid(cell));
            let rows = font(k.letter);
            let lit = |gx: i32, gy: i32| (0..5).contains(&gx) && (0..7).contains(&gy) && rows[gy as usize] >> (4 - gx) & 1 == 1;
            if lit(gx, gy) {
                lerp(WHITE, av, 0.08)
            } else if lit(gx - 1, gy - 1) {
                base.map(|c| c * 0.3)
            } else {
                base
            }
        }
        Style::Truchet => {
            let n = 3.0;
            let (cx, cy) = ((u * n).floor().min(n - 1.0), (v * n).floor().min(n - 1.0));
            let (mut fx, fy) = (u * n - cx, v * n - cy);
            if k.bits(10) >> (cy * n + cx) as u64 & 1 == 1 {
                fx = 1.0 - fx;
            }
            let d = (fx.hypot(fy) - 0.5).abs().min(((1.0 - fx).hypot(1.0 - fy) - 0.5).abs());
            let bg = if k.dark { av.map(|c| c * 0.4) } else { av };
            if d < 0.17 { lerp(bv, WHITE, if k.dark { 0.25 } else { 0.6 }) } else { bg }
        }
        Style::Face => {
            let skin = lerp(k.a, av, 0.7);
            let skin = if k.dark { skin.map(|c| c * 0.75) } else { skin };
            let ink = if lum(skin) > 0.3 { srgb(28, 24, 32) } else { srgb(250, 248, 240) };
            let ring = |cx: f32, cy: f32, r: f32, w: f32| ((u - cx).hypot(v - cy) - r).abs() <= w;
            let disc = |cx: f32, cy: f32, r: f32| (u - cx).hypot(v - cy) <= r;
            let w = 0.05;
            let eyes = match k.bits(14) % 5 {
                0 => disc(0.33, 0.42, 0.075) || disc(0.67, 0.42, 0.075),
                1 => ((u - 0.34) / 0.06).hypot((v - 0.42) / 0.11) <= 1.0 || ((u - 0.66) / 0.06).hypot((v - 0.42) / 0.11) <= 1.0,
                2 => v <= 0.45 && (ring(0.33, 0.46, 0.09, w) || ring(0.67, 0.46, 0.09, w)),
                3 => {
                    if disc(0.5, 0.42, 0.1) {
                        return ink;
                    }
                    if disc(0.5, 0.42, 0.19) {
                        return if lum(ink) > 0.5 { srgb(28, 24, 32) } else { srgb(250, 248, 240) };
                    }
                    false
                }
                _ => (v - 0.42).abs() <= w * 0.8 && ((0.24..0.42).contains(&u) || (0.58..0.76).contains(&u)),
            };
            let mouth = match k.bits(15) % 5 {
                0 => v >= 0.62 && ring(0.5, 0.6, 0.15, w),
                1 => (v - 0.72).abs() <= w * 0.8 && (0.36..0.64).contains(&u),
                2 => ring(0.5, 0.72, 0.06, w * 0.8),
                3 => v >= 0.64 && disc(0.5, 0.64, 0.14),
                _ => v >= 0.66 && (ring(0.43, 0.66, 0.07, w * 0.8) || ring(0.57, 0.66, 0.07, w * 0.8)),
            };
            if eyes || mouth {
                ink
            } else if k.bits(16) & 1 == 1 && (disc(0.2, 0.62, 0.08) || disc(0.8, 0.62, 0.08)) {
                lerp(skin, srgb(255, 90, 120), 0.5)
            } else {
                skin
            }
        }
        Style::Lava => {
            let bg = if k.dark { lerp(cv.map(|c| c * 0.12), cv.map(|c| c * 0.3), v) } else { lerp(lerp(cv, WHITE, 0.75), lerp(cv, WHITE, 0.5), v) };
            let mut f = 0.0;
            for i in 0..4u64 {
                let (bx, by) = (0.22 + 0.56 * k.unit(20 + i), 0.18 + 0.64 * k.unit(30 + i));
                let r = if i == 0 { 0.2 + 0.06 * k.unit(40) } else { 0.09 + 0.07 * k.unit(40 + i) };
                f += r * r / ((u - bx).powi(2) + (v - by).powi(2)).max(1e-4);
            }
            if f < 1.0 {
                return bg;
            }
            let col = lerp(bv, av, v);
            let shine = smooth(1.0, 3.5, f);
            lerp(col.map(|c| c * (0.72 + 0.28 * shine)), WHITE, 0.25 * smooth(2.5, 8.0, f))
        }
        Style::Neon => {
            let tube = lerp(bv, WHITE, 0.15);
            let seg = |ax: f32, ay: f32, bx: f32, by: f32| {
                let (px, py, dx, dy) = (x - ax, y - ay, bx - ax, by - ay);
                let t = ((px * dx + py * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
                (px - dx * t).hypot(py - dy * t)
            };
            let d = match k.bits(19) % 5 {
                0 => (x.hypot(y) - 0.24).abs(),
                1 => seg(0.0, -0.25, 0.25, 0.19).min(seg(0.25, 0.19, -0.25, 0.19)).min(seg(-0.25, 0.19, 0.0, -0.25)),
                2 => round_box(x, y, 0.24, 0.07).abs(),
                3 => seg(-0.27, 0.1, -0.09, -0.12).min(seg(-0.09, -0.12, 0.09, 0.12)).min(seg(0.09, 0.12, 0.27, -0.1)),
                _ => seg(-0.06, -0.26, 0.1, -0.02).min(seg(0.1, -0.02, -0.1, 0.03)).min(seg(-0.1, 0.03, 0.06, 0.27)),
            };
            let glow = (-(d / 0.1).powi(2)).exp() * 0.75;
            let core = 1.0 - smooth(0.018, 0.04, d);
            let tile = lerp(srgb(10, 9, 16), k.a, 0.08);
            let col = lerp(lerp(tile, tube, glow), lerp(tube, WHITE, 0.55), core);
            // Scan lines a device pixel apart, two at 2×.
            let line = (k.px / 32).max(1) as i32;
            if (iy / line) & 1 == 1 { col.map(|c| c * 0.8) } else { col }
        }
        _ => k.a,
    }
}

/// A 5×7 bitmap glyph: seven rows, the high bit of five on the left.
fn font(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        _ => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b00000, 0b00100],
    }
}

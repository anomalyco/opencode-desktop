//! Loud marks that don't look like soft tiles: a pixel sprite, a sticker, a pop letter, Bauhaus
//! blocks, a glossy orb and a hand-drawn squiggle. They use their own bright colours rather than
//! the theme's avatars, so each reads on light and dark alike.

use rune::Color;

use crate::pixels::Buf;
use crate::styles::{RADIUS, Style, hash, round_box};

type Rgb = [f32; 3];

/// Hot pink, tangerine, lemon, lime, mint, sky, cobalt, violet, tomato, teal.
const HUES: [&str; 10] = ["ff4fa0", "ff7a1a", "ffd23f", "9be15d", "2fd8a8", "3fb8ff", "3a5bff", "8b5cff", "ff4b3e", "12a4a0"];
/// The hues dark enough for cream ink on top.
const DEEP: [usize; 6] = [0, 1, 6, 7, 8, 9];
const CREAM: &str = "fff6e5";
const INK: &str = "1b1a22";

fn c(hex: &str) -> Rgb {
    let c = rune::color::hex(&format!("#{hex}")).unwrap_or(Color::TRANSPARENT);
    [c.r, c.g, c.b]
}

fn color(hex: &str) -> Color {
    rune::color::hex(&format!("#{hex}")).unwrap_or(Color::TRANSPARENT)
}

fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

fn unit(h: u64, shift: u32) -> f32 {
    ((h >> shift) & 0xffff) as f32 / 65535.0
}

fn deep(h: u64, shift: u32) -> usize {
    DEEP[((h >> shift) % DEEP.len() as u64) as usize]
}

/// Two different hues from the seed.
fn pair(h: u64) -> (usize, usize) {
    let a = (h % 10) as usize;
    let b = (a + 2 + (h >> 4) as usize % 7) % 10;
    (a, b)
}

pub(crate) fn funky(style: Style) -> bool {
    matches!(style, Style::Pixel | Style::Sticker | Style::Pop | Style::Bauhaus | Style::Orb | Style::Squiggle)
}

/// Pop's letter and its offset shadow.
pub(crate) fn pop_ink() -> (Color, Color) {
    (color(CREAM), color(INK))
}

pub(crate) fn draw(style: Style, seed: &str, size_pt: f32, px: u32) -> Vec<u8> {
    let h = hash(seed);
    let pt = 1.0 / size_pt;
    let tile = |x: f32, y: f32| round_box(x - 0.5, y - 0.5, 0.5, RADIUS);
    let pxf = px as f32;
    let shade = |u: f32, v: f32| -> [f32; 4] {
        match style {
            Style::Pixel => {
                let bg = c(HUES[deep(h, 0)]);
                // Whole device pixels per cell, so the sprite stays crisp at 16 pt.
                let cell = (pxf * 0.74 / 5.0).floor().max(1.0);
                let off = ((pxf - cell * 5.0) / 2.0).round();
                let (gx, gy) = (((u * pxf - off) / cell).floor(), ((v * pxf - off) / cell).floor());
                if !(0.0..5.0).contains(&gx) || !(0.0..5.0).contains(&gy) {
                    return [bg[0], bg[1], bg[2], 1.0];
                }
                let (cx, cy) = (gx as u32, gy as u32);
                let col = if cx > 2 { 4 - cx } else { cx };
                let mut bits = (h >> 20) as u32 & 0x7fff;
                if !(6..=11).contains(&bits.count_ones()) {
                    bits = (bits ^ 0b101_010_111_010_101) | 0b000_010_000_000_010;
                }
                let on = bits >> (cy * 3 + col) & 1 == 1;
                // A few cells in a second colour: the sprite's eyes.
                let eye = on && cy == 1 + (h >> 40 & 1) as u32 && col == (h >> 41 & 1) as u32;
                let ink = if eye { c(HUES[(deep(h, 0) + 2 + (h >> 44) as usize % 3) % 10]) } else { c(CREAM) };
                let col = if on { ink } else { bg };
                [col[0], col[1], col[2], 1.0]
            }
            Style::Sticker => {
                let (a, _) = pair(h);
                let k = 3.0 + (h >> 12 & 3) as f32;
                let phase = unit(h, 16) * std::f32::consts::TAU;
                let wob = 0.07 + 0.04 * unit(h, 32);
                let r_at = |x: f32, y: f32| {
                    let t = y.atan2(x);
                    0.41 * (1.0 + wob * (k * t + phase).sin() + 0.03 * (2.0 * k * t - phase).cos())
                };
                let edge = (0.085f32).max(1.0 * pt);
                let blob = |x: f32, y: f32| x.hypot(y) - r_at(x, y);
                let (x, y) = (u - 0.48, v - 0.46);
                let d = blob(x, y);
                // A hard drop shadow, down and right.
                let s = blob(x - 0.03, y - 0.045);
                if d <= -edge {
                    let mut col = c(HUES[a]);
                    let shine = (x + 0.12).hypot(y + 0.14);
                    if shine < 0.07 {
                        col = lerp(col, [1.0; 3], 0.75);
                    }
                    [col[0], col[1], col[2], 1.0]
                } else if d <= 0.0 {
                    let w = c("ffffff");
                    [w[0], w[1], w[2], 1.0]
                } else if s <= 0.0 {
                    [0.0, 0.0, 0.0, 0.35]
                } else {
                    [0.0; 4]
                }
            }
            Style::Pop => {
                if tile(u, v) > 0.0 {
                    return [0.0; 4];
                }
                let bg = c(HUES[deep(h, 0)]);
                // Halftone dots in one corner, only where they're big enough to see.
                let dots = size_pt >= 24.0 && {
                    let n = 7.0;
                    let (cx, cy) = if h >> 8 & 1 == 0 { (u, v) } else { (1.0 - u, v) };
                    let (fx, fy) = ((cx * n).fract() - 0.5, (cy * n).fract() - 0.5);
                    let r = 0.32 * (1.0 - (cx + cy) / 1.1).max(0.0);
                    fx.hypot(fy) < r
                };
                let col = if dots { lerp(bg, [1.0; 3], 0.3) } else { bg };
                [col[0], col[1], col[2], 1.0]
            }
            Style::Bauhaus => {
                if tile(u, v) > 0.0 {
                    return [0.0; 4];
                }
                let inks = [c("e63b2e"), c("f5c518"), c("1e4fd8"), c(INK), c("f2e8d5")];
                let (gx, gy) = ((u * 2.0).min(1.999) as u32, (v * 2.0).min(1.999) as u32);
                let i = gy * 2 + gx;
                let bits = (h >> (i * 8)) as u32;
                let bg = (bits % 5) as usize;
                let fg = (bg + 1 + (bits >> 3) as usize % 4) % 5;
                let (fx, fy) = ((u * 2.0).fract(), (v * 2.0).fract());
                let corner = (bits >> 6 & 3) as usize;
                let (ox, oy) = ([0.0, 1.0, 1.0, 0.0][corner], [0.0, 0.0, 1.0, 1.0][corner]);
                let inside = match bits >> 9 & 3 {
                    // A quarter circle from a corner.
                    0 => (fx - ox).hypot(fy - oy) < 1.0,
                    // A circle.
                    1 => (fx - 0.5).hypot(fy - 0.5) < 0.36,
                    // A half disc on one side.
                    2 => {
                        let (sx, sy) = [(0.5, 0.0), (1.0, 0.5), (0.5, 1.0), (0.0, 0.5)][corner];
                        (fx - sx).hypot(fy - sy) < 0.5
                    }
                    // A triangle: half the cell, cut on a diagonal.
                    _ => {
                        if corner & 1 == 0 {
                            fx > fy
                        } else {
                            fx + fy < 1.0
                        }
                    }
                };
                let col = inks[if inside { fg } else { bg }];
                [col[0], col[1], col[2], 1.0]
            }
            Style::Orb => {
                let (a, b) = pair(h);
                let (x, y) = ((u - 0.5) / 0.46, (v - 0.5) / 0.46);
                let r2 = x * x + y * y;
                // A soft contact shadow under the ball.
                if r2 > 1.0 {
                    let (sx, sy) = ((u - 0.5) / 0.4, (v - 0.92) / 0.06);
                    let s = (1.0 - (sx * sx + sy * sy)).clamp(0.0, 1.0);
                    return [0.0, 0.0, 0.0, 0.25 * s];
                }
                let z = (1.0 - r2).sqrt();
                let (lx, ly, lz) = (-0.45f32, -0.55f32, 0.7f32);
                let lit = (x * lx + y * ly + z * lz).max(0.0);
                let angle = (h >> 24 & 7) as f32 * std::f32::consts::FRAC_PI_4;
                let t = ((x * angle.cos() + y * angle.sin()) * 0.5 + 0.5).clamp(0.0, 1.0);
                let base = lerp(c(HUES[a]), c(HUES[b]), t);
                let mut col = base.map(|v| v * (0.45 + 0.65 * lit));
                // A rim of reflected light from below.
                col = lerp(col, c(HUES[b]), 0.35 * (1.0 - z).powi(2) * (y.max(0.0)));
                let spec = ((x + 0.38).hypot(y + 0.42) / 0.22).min(1.0);
                col = lerp(col, [1.0; 3], 0.85 * (1.0 - spec * spec));
                [col[0], col[1], col[2], 1.0]
            }
            Style::Squiggle => {
                if tile(u, v) > 0.0 {
                    return [0.0; 4];
                }
                let bg = c(HUES[deep(h, 0)]);
                let d = squiggle(h, u, v);
                let w = 0.06f32.max(0.8 * pt);
                let dot = {
                    let (dx, dy) = (0.2 + 0.6 * unit(h, 48), if h >> 30 & 1 == 0 { 0.22 } else { 0.8 });
                    (u - dx).hypot(v - dy) < 0.075f32.max(1.0 * pt)
                };
                let col = if d < w {
                    c(CREAM)
                } else if dot {
                    c(HUES[(deep(h, 0) + 2 + (h >> 52) as usize % 4) % 10])
                } else {
                    bg
                };
                [col[0], col[1], col[2], 1.0]
            }
            _ => [0.0; 4],
        }
    };
    // Pixel stays crisp: one sample at each pixel's centre.
    let n = if style == Style::Pixel { 1 } else if px <= 48 { 4 } else { 2 };
    let mut buf = Buf::new(px, px);
    for py in 0..px {
        for x in 0..px {
            let mut acc = [0.0f32; 4];
            for sy in 0..n {
                for sx in 0..n {
                    let u = (x as f32 + (sx as f32 + 0.5) / n as f32) / pxf;
                    let v = (py as f32 + (sy as f32 + 0.5) / n as f32) / pxf;
                    let [r, g, b, alpha] = shade(u, v);
                    acc = [acc[0] + r * alpha, acc[1] + g * alpha, acc[2] + b * alpha, acc[3] + alpha];
                }
            }
            buf.px[(py * px + x) as usize] = acc.map(|v| v / (n * n) as f32);
        }
    }
    // Pixel's corners still want antialiasing: blend the tile's edge in afterwards.
    if style == Style::Pixel {
        for py in 0..px {
            for x in 0..px {
                let mut cover = 0.0;
                for s in 0..16 {
                    let (sx, sy) = ((s % 4) as f32 + 0.5, (s / 4) as f32 + 0.5);
                    if tile((x as f32 + sx / 4.0) / pxf, (py as f32 + sy / 4.0) / pxf) <= 0.0 {
                        cover += 1.0 / 16.0;
                    }
                }
                let p = &mut buf.px[(py * px + x) as usize];
                let a = p[3].max(1e-6);
                *p = [p[0] / a * cover, p[1] / a * cover, p[2] / a * cover, cover];
            }
        }
    }
    buf.to_rgba8()
}

/// Distance to a seeded hand-drawn stroke: a wave, a zigzag or a loop.
fn squiggle(h: u64, u: f32, v: f32) -> f32 {
    let kind = h >> 36 & 3;
    let phase = unit(h, 4) * std::f32::consts::TAU;
    let amp = 0.12 + 0.06 * unit(h, 12);
    let at = |t: f32| -> (f32, f32) {
        // A slight wobble, as if drawn by hand.
        let wob = 0.012 * (t * 23.0 + phase).sin();
        match kind {
            0 | 1 => (0.18 + 0.64 * t, 0.5 + amp * (t * std::f32::consts::TAU * 1.5 + phase).sin() + wob),
            2 => {
                let saw = ((t * 3.5).fract() - 0.5).abs() * 2.0 - 0.5;
                (0.18 + 0.64 * t, 0.5 + amp * 1.6 * saw + wob)
            }
            _ => {
                // A cursive loop: a circle pulled along a line.
                let a = t * std::f32::consts::TAU * 2.0 + phase;
                (0.24 + 0.52 * t + 0.1 * a.cos(), 0.5 + 0.16 * a.sin() + wob)
            }
        }
    };
    let n = 48;
    let mut best = f32::MAX;
    let mut prev = at(0.0);
    for i in 1..=n {
        let p = at(i as f32 / n as f32);
        best = best.min(segment(u, v, prev, p));
        prev = p;
    }
    best
}

fn segment(x: f32, y: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((x - a.0) * dx + (y - a.1) * dy) / (dx * dx + dy * dy).max(1e-9)).clamp(0.0, 1.0);
    (x - a.0 - dx * t).hypot(y - a.1 - dy * t)
}

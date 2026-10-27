//! The rest of the Pixel family: critters, monsters, loot and invaders, two more pixel fonts, and
//! dithered landscapes, blends and grain. Each draws one colour per device pixel on a grid of whole
//! device pixels, so edges stay sharp at 1× and 2×; only the tile's rounded corners are smoothed.

use crate::Palette;
use crate::pixels::Buf;
use crate::retro::{font, lum, srgb, vivid};
use crate::styles::{Look, RADIUS, Rgb, Style, hash, lerp, mix, rgb, round_box, saturate};

const BLACK: Rgb = [0.0; 3];
const WHITE: Rgb = [1.0; 3];

/// PICO-8's sixteen colours.
const PICO: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (29, 43, 83),
    (126, 37, 83),
    (0, 135, 81),
    (171, 82, 54),
    (95, 87, 79),
    (194, 195, 199),
    (255, 241, 232),
    (255, 0, 77),
    (255, 163, 0),
    (255, 236, 39),
    (0, 228, 54),
    (41, 173, 255),
    (131, 118, 156),
    (255, 119, 168),
    (255, 204, 170),
];
/// NES colours bright enough for an item: red, pink, blue, green, purple, teal, gold.
const NES: [(u8, u8, u8); 7] = [(248, 56, 0), (248, 88, 152), (0, 120, 248), (0, 168, 0), (104, 68, 252), (0, 136, 136), (248, 184, 0)];
/// The Game Boy's four greens, darkest first.
const DMG: [(u8, u8, u8); 4] = [(15, 56, 15), (48, 98, 48), (139, 172, 15), (155, 188, 15)];

fn pico(i: usize) -> Rgb {
    let (r, g, b) = PICO[i];
    srgb(r, g, b)
}

fn tone((r, g, b): (u8, u8, u8)) -> Rgb {
    srgb(r, g, b)
}

pub(crate) fn owns(style: Style) -> bool {
    matches!(
        style,
        Style::Critter
            | Style::Monster
            | Style::Loot
            | Style::GameBoy
            | Style::Cga
            | Style::Duo
            | Style::Chunky
            | Style::Dusk
            | Style::Blend
            | Style::Static
    )
}

struct Seed {
    h: u64,
    dark: bool,
    hue: Rgb,
}

impl Seed {
    fn bits(&self, k: u64) -> u64 {
        mix(self.h ^ k.wrapping_mul(0x9e37_79b9_7f4a_7c15))
    }

    fn unit(&self, k: u64) -> f32 {
        (self.bits(k) & 0xffff) as f32 / 65535.0
    }

    fn pick(&self, k: u64, n: usize) -> usize {
        (self.bits(k) % n as u64) as usize
    }

    fn coin(&self, k: u64) -> bool {
        self.bits(k) & 1 == 1
    }

    /// A pale tile in light mode and a deep one in dark, tinted by `c`.
    fn ground(&self, c: Rgb) -> Rgb {
        if self.dark { lerp(c, BLACK, 0.8) } else { lerp(c, WHITE, 0.82) }
    }
}

/// `nx` by `ny` cells, each a whole number of device pixels, filling at most `fill` of the tile,
/// centred.
#[derive(Clone, Copy)]
struct Grid {
    cell: i32,
    ox: i32,
    oy: i32,
}

impl Grid {
    fn new(px: i32, nx: i32, ny: i32, fill: f32) -> Grid {
        let cell = ((px as f32 * fill) as i32 / nx.max(ny)).max(1);
        Grid { cell, ox: (px - cell * nx).div_euclid(2), oy: (px - cell * ny).div_euclid(2) }
    }

    fn at(self, ix: i32, iy: i32) -> (i32, i32) {
        ((ix - self.ox).div_euclid(self.cell), (iy - self.oy).div_euclid(self.cell))
    }
}

const INK: u8 = 1;
const BODY: u8 = 2;
const ACCENT: u8 = 3;
const SHINE: u8 = 4;
const EXTRA: u8 = 7;

/// A sprite: an index into its colours per cell, 0 for the ground.
struct Map {
    w: i32,
    h: i32,
    cells: Vec<u8>,
}

impl Map {
    fn new(w: i32, h: i32) -> Map {
        Map { w, h, cells: vec![0; (w * h) as usize] }
    }

    /// Rows of `.` (ground), `k` ink, `b` body, `a` accent, `w` shine, `l` light, `d` shade and `s`.
    fn parse(rows: &[&str]) -> Map {
        let mut m = Map::new(rows[0].len() as i32, rows.len() as i32);
        for (y, row) in rows.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                let i = ".kbawlds".find(c).unwrap_or(0) as u8;
                m.set(x as i32, y as i32, i);
            }
        }
        m
    }

    fn get(&self, x: i32, y: i32) -> u8 {
        if (0..self.w).contains(&x) && (0..self.h).contains(&y) { self.cells[(y * self.w + x) as usize] } else { 0 }
    }

    fn set(&mut self, x: i32, y: i32, c: u8) {
        if (0..self.w).contains(&x) && (0..self.h).contains(&y) {
            self.cells[(y * self.w + x) as usize] = c;
        }
    }

    /// The cell `d` from the middle on both sides of row `y` (an even width has two middle cells).
    fn pair(&mut self, y: i32, d: i32, c: u8) {
        let left = (self.w - 1) / 2 - d;
        self.set(left, y, c);
        self.set(self.w - 1 - left, y, c);
    }

    fn flip(mut self) -> Map {
        for y in 0..self.h {
            self.cells[(y * self.w) as usize..((y + 1) * self.w) as usize].reverse();
        }
        self
    }

    /// Inks the ground beside the sprite (not diagonally), so it reads on any ground.
    fn outline(self) -> Map {
        let solid = |x: i32, y: i32| !matches!(self.get(x, y), 0 | INK);
        let mut out = Map { w: self.w, h: self.h, cells: self.cells.clone() };
        for y in 0..self.h {
            for x in 0..self.w {
                if self.get(x, y) == 0 && [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dy)| solid(x + dx, y + dy)) {
                    out.set(x, y, INK);
                }
            }
        }
        out
    }
}

type Paint = Box<dyn Fn(i32, i32) -> Rgb>;

fn sprite(map: Map, g: Grid, colors: [Rgb; 8], ground: impl Fn(i32, i32) -> Rgb + 'static) -> Paint {
    Box::new(move |ix, iy| {
        let (x, y) = g.at(ix, iy);
        match map.get(x, y) {
            0 => ground(ix, iy),
            c => colors[c as usize],
        }
    })
}

/// The ordered-dither threshold at `(x, y)` in a `2^bits`-square Bayer matrix, 0–1.
fn bayer(x: i32, y: i32, bits: u32) -> f32 {
    let (a, b) = (x ^ y, y);
    let mut v = 0;
    for i in 0..bits {
        v = (v << 2) | ((a >> i & 1) << 1) | (b >> i & 1);
    }
    (v as f32 + 0.5) / (1 << (2 * bits)) as f32
}

pub(crate) fn draw(p: &Palette, seed: &str, variant: usize, look: &Look, px: u32) -> Vec<u8> {
    let s = look.saturation;
    let k = Seed { h: hash(seed), dark: p.dark, hue: vivid(saturate(rgb(p.avatars[variant].0), s), s) };
    let n = px as i32;
    let ink = srgb(34, 26, 46);
    let paint: Paint = match look.style {
        Style::Critter => {
            let pink = srgb(255, 110, 160);
            let gold = srgb(255, 200, 60);
            let dist = |a: Rgb, b: Rgb| (0..3).map(|i| (a[i] - b[i]).abs()).sum::<f32>();
            let accent = if dist(k.hue, pink) > dist(k.hue, gold) { pink } else { gold };
            let colors = [BLACK, ink, k.hue, accent, WHITE, WHITE, ink, srgb(60, 200, 90)];
            let ground = k.ground(k.hue);
            sprite(critter(&k), Grid::new(n, 12, 12, 0.92), colors, move |_, _| ground)
        }
        Style::GameBoy => {
            let [d0, d1, d2, d3] = DMG.map(tone);
            let g = Grid::new(n, 12, 12, 0.92);
            let colors = [BLACK, d0, d1, d2, d3, d3, d0, d2];
            // The ground fades from the lightest green to the next near the bottom, in the sprite's pixels.
            sprite(critter(&k), g, colors, move |ix, iy| {
                let (x, y) = g.at(ix, iy);
                let t = ((iy as f32 / n as f32 - 0.6) / 0.4).clamp(0.0, 1.0);
                if t > bayer(x, y, 2) { d2 } else { d3 }
            })
        }
        Style::Monster => {
            let (map, colors) = monster(&k);
            let ground = if k.dark { pico(1) } else { pico(7) };
            sprite(map, Grid::new(n, 12, 12, 0.92), colors, move |_, _| ground)
        }
        Style::Loot => {
            let (map, colors) = loot(&k);
            let ground = k.ground(colors[BODY as usize]);
            sprite(map, Grid::new(n, 12, 12, 0.92), colors, move |_, _| ground)
        }
        Style::Cga => {
            let (cyan, magenta) = (srgb(85, 255, 255), srgb(255, 85, 255));
            let (body, eye) = if k.coin(30) { (cyan, magenta) } else { (magenta, cyan) };
            let colors = [BLACK, BLACK, body, eye, WHITE, WHITE, BLACK, BLACK];
            sprite(invader(&k), Grid::new(n, 11, 8, 0.8), colors, |_, _| BLACK)
        }
        Style::Duo => {
            let [a, b] = initials(seed).map(font3);
            let tile = if k.dark { k.hue.map(|c| c * 0.5) } else { k.hue };
            let (letter, shadow) = if lum(tile) > 0.4 { (ink, lerp(tile, WHITE, 0.55)) } else { (WHITE, lerp(tile, BLACK, 0.6)) };
            let g = Grid::new(n, 8, 6, 0.86);
            let lit = move |x: i32, y: i32| {
                let (rows, cx) = if (0..3).contains(&x) { (a, x) } else if (4..7).contains(&x) { (b, x - 4) } else { return false };
                (0..5).contains(&y) && rows[y as usize] >> (2 - cx) & 1 == 1
            };
            Box::new(move |ix, iy| {
                let (x, y) = g.at(ix, iy);
                if lit(x, y) {
                    letter
                } else if lit(x - 1, y - 1) {
                    shadow
                } else {
                    tile
                }
            })
        }
        Style::Chunky => {
            let rows = font(seed.chars().find(|c| c.is_alphanumeric()).unwrap_or('?'));
            let mut m = Map::new(8, 9);
            for (y, row) in rows.iter().enumerate() {
                // Each lit cell also lights the one to its right: a bold 6×7 letter.
                let bold = row << 1 | row;
                for x in 0..6 {
                    if bold >> (5 - x) & 1 == 1 {
                        m.set(x + 1, y as i32 + 1, BODY);
                    }
                }
            }
            let line = if k.dark { BLACK } else { ink };
            let ground = k.ground(k.hue);
            sprite(m.outline(), Grid::new(n, 8, 9, 0.86), [BLACK, line, k.hue, BLACK, WHITE, WHITE, BLACK, BLACK], move |_, _| ground)
        }
        Style::Dusk => {
            let (lo, hi) = if k.dark { (lerp(k.hue, BLACK, 0.86), lerp(k.hue, WHITE, 0.1)) } else { (lerp(k.hue, BLACK, 0.55), lerp(k.hue, WHITE, 0.72)) };
            let dot = (n / 24).max(1);
            let (sx, sy, r) = (0.25 + 0.5 * k.unit(31), 0.34 + 0.12 * k.unit(32), 0.13 + 0.05 * k.unit(33));
            let (p1, p2, p3) = (k.unit(34) * 6.3, k.unit(35) * 6.3, k.unit(36) * 6.3);
            let far = move |u: f32| 0.6 + 0.07 * (u * 6.0 + p1).sin() + 0.03 * (u * 13.0 + p2).sin();
            let near = move |u: f32| 0.78 + 0.06 * (u * 4.5 + p3).sin();
            Box::new(move |ix, iy| {
                let (dx, dy) = (ix / dot, iy / dot);
                let (u, v) = (((dx as f32) + 0.5) * dot as f32 / n as f32, ((dy as f32) + 0.5) * dot as f32 / n as f32);
                let sun = (u - sx).hypot(v - sy);
                let t = if v > near(u) {
                    0.0
                } else if v > far(u) {
                    0.5
                } else if sun < r {
                    1.0
                } else {
                    (0.15 + 0.75 * (v / far(u)).powf(2.0) + 0.35 * (1.0 - sun / 0.5).max(0.0)).min(0.97)
                };
                if t > bayer(dx, dy, 3) { hi } else { lo }
            })
        }
        Style::Blend => {
            let a = pico([8, 9, 10, 11, 12, 14][k.pick(40, 6)]);
            let b = pico([1, 2, 3, 4, 5, 13][k.pick(41, 6)]);
            let dot = (n / 16).max(1);
            let kind = k.pick(42, 3);
            let (cx, cy) = (0.2 + 0.6 * k.unit(43), 0.2 + 0.6 * k.unit(44));
            let angle = k.unit(45) * std::f32::consts::TAU;
            Box::new(move |ix, iy| {
                let (dx, dy) = (ix / dot, iy / dot);
                let (u, v) = (((dx as f32) + 0.5) * dot as f32 / n as f32, ((dy as f32) + 0.5) * dot as f32 / n as f32);
                let d = (u - cx).hypot(v - cy);
                let t = match kind {
                    0 => 1.0 - d / 0.8,
                    1 => 0.5 + ((u - 0.5) * angle.cos() + (v - 0.5) * angle.sin()) * 1.2,
                    _ => 0.5 + 0.5 * (d * std::f32::consts::TAU * 2.2).cos(),
                };
                if t > bayer(dx, dy, 3) { a } else { b }
            })
        }
        Style::Static => {
            let (off, on) = if k.dark { (lerp(k.hue, BLACK, 0.88), k.hue) } else { (lerp(k.hue, WHITE, 0.86), lerp(k.hue, BLACK, 0.35)) };
            let dot = (n / 24).max(1);
            let m = (n + dot - 1) / dot;
            let glows = [(0.2 + 0.6 * k.unit(50), 0.2 + 0.6 * k.unit(51), 0.42 + 0.14 * k.unit(52)), (0.15 + 0.7 * k.unit(53), 0.15 + 0.7 * k.unit(54), 0.2 + 0.08 * k.unit(55))];
            let mut level: Vec<f32> = (0..m * m)
                .map(|i| {
                    let (u, v) = (((i % m) as f32 + 0.5) / m as f32, ((i / m) as f32 + 0.5) / m as f32);
                    let glow = glows.iter().map(|&(x, y, r)| (1.0 - (u - x).hypot(v - y) / r).max(0.0)).fold(0.0, f32::max);
                    0.08 + 0.95 * glow.powf(0.7)
                })
                .collect();
            let mut lit = vec![false; (m * m) as usize];
            for y in 0..m {
                for x in 0..m {
                    let i = (y * m + x) as usize;
                    let set = level[i] > 0.5;
                    lit[i] = set;
                    let e = level[i] - if set { 1.0 } else { 0.0 };
                    let mut spread = |dx: i32, dy: i32, w: f32| {
                        if (0..m).contains(&(x + dx)) && y + dy < m {
                            level[((y + dy) * m + x + dx) as usize] += e * w;
                        }
                    };
                    spread(1, 0, 7.0 / 16.0);
                    spread(-1, 1, 3.0 / 16.0);
                    spread(0, 1, 5.0 / 16.0);
                    spread(1, 1, 1.0 / 16.0);
                }
            }
            Box::new(move |ix, iy| if lit[((iy / dot) * m + ix / dot) as usize] { on } else { off })
        }
        _ => Box::new(move |_, _| k.hue),
    };
    let mut buf = Buf::new(px, px);
    let pxf = px as f32;
    for iy in 0..n {
        for ix in 0..n {
            let mut col = paint(ix, iy);
            let mut cover = 0;
            for s in 0..16 {
                let (u, v) = ((ix as f32 + (s % 4) as f32 / 4.0 + 0.125) / pxf, (iy as f32 + (s / 4) as f32 / 4.0 + 0.125) / pxf);
                cover += (round_box(u - 0.5, v - 0.5, 0.5, RADIUS) <= 0.0) as u32;
            }
            // A one-pixel edge, so a pale tile holds its shape on a pale sidebar.
            let (u, v) = ((ix as f32 + 0.5) / pxf, (iy as f32 + 0.5) / pxf);
            if round_box(u - 0.5, v - 0.5, 0.5, RADIUS) > -1.0 / pxf {
                col = if k.dark { lerp(col, WHITE, 0.12) } else { col.map(|c| c * 0.78) };
            }
            let a = cover as f32 / 16.0;
            buf.px[(iy * n + ix) as usize] = [col[0] * a, col[1] * a, col[2] * a, a];
        }
    }
    buf.to_rgba8()
}

/// A mirrored critter on a 12×12 grid: a body, ears, antennae or a sprout, eyes, a mouth, cheeks,
/// arms and feet, all from the seed, outlined in ink.
fn critter(k: &Seed) -> Map {
    // Half-widths from the body's top row down; every body ends on row 9.
    const BODIES: [&[i32]; 5] = [&[2, 3, 4, 4, 4, 3], &[3, 4, 4, 4, 4, 4], &[2, 3, 3, 3, 3, 3, 2], &[2, 2, 3, 4, 4, 3], &[2, 3, 4, 4, 4, 4, 3]];
    let body = BODIES[k.pick(1, BODIES.len())];
    let top = 10 - body.len() as i32;
    let width = |y: i32| body.get((y - top) as usize).copied().unwrap_or(0);
    let mut m = Map::new(12, 12);
    for y in top..10 {
        for d in 0..width(y) {
            m.pair(y, d, BODY);
        }
    }
    let w0 = body[0];
    match k.pick(2, 5) {
        0 => {
            m.pair(top - 1, w0 - 1, BODY);
            if k.coin(3) {
                m.pair(top - 2, w0 - 1, BODY);
            }
        }
        1 => {
            m.pair(top - 1, w0 - 1, BODY);
            m.pair(top - 1, (w0 - 2).max(0), BODY);
            m.pair(top - 2, w0 - 1, BODY);
        }
        2 => {
            m.pair(top - 1, 1, INK);
            m.pair(top - 2, 2, ACCENT);
        }
        3 => {
            m.set(6, top - 1, EXTRA);
            m.set(7, top - 2, EXTRA);
            m.set(5, top - 2, EXTRA);
        }
        _ => {}
    }
    let ey = top + 1;
    let room = width(ey).min(width(ey + 1));
    let ed = if room >= 4 { 1 + k.pick(4, 2) as i32 } else { 1 };
    let eyes = k.pick(5, 4);
    let wide = (ed + 1).min(room - 1);
    match eyes {
        0 => m.pair(ey, ed, INK),
        1 => {
            m.pair(ey, ed, INK);
            m.pair(ey + 1, ed, INK);
        }
        2 => {
            let ed = wide - 1;
            m.pair(ey, ed, SHINE);
            m.pair(ey, ed + 1, INK);
            m.pair(ey + 1, ed, INK);
            m.pair(ey + 1, ed + 1, INK);
        }
        _ => {
            m.pair(ey + 1, wide - 1, INK);
            m.pair(ey + 1, wide, INK);
        }
    }
    let my = ey + if matches!(eyes, 1 | 2) { 3 } else { 2 };
    if my <= 9 {
        match k.pick(6, 3) {
            0 => m.pair(my, 0, INK),
            1 => m.pair(my, 0, ACCENT),
            _ => {
                m.pair(my, 1, INK);
                if my < 9 {
                    m.pair(my + 1, 0, INK);
                }
            }
        }
    }
    if k.coin(7) {
        let cy = ey + 2;
        let cd = wide + 1;
        if m.get(5 - cd, cy) == BODY {
            m.pair(cy, cd, ACCENT);
        }
    }
    if k.coin(8) {
        let ay = top + 3;
        let w = width(ay);
        if w < 5 {
            m.pair(ay, w, BODY);
        }
    }
    let last = body[body.len() - 1];
    let feet: &[i32] = [&[2][..], &[1, 3], &[3]][k.pick(9, 3)];
    for &d in feet {
        if d < last {
            m.pair(10, d, BODY);
        }
    }
    m.outline()
}

/// A mushroom, ghost, slime or robot, in PICO-8 colours.
fn monster(k: &Seed) -> (Map, [Rgb; 8]) {
    const MUSHROOM: [&str; 12] = [
        "............",
        "....bbbb....",
        "..bbwbbwbb..",
        ".bwwbbbbwwb.",
        ".bbbbwwbbbb.",
        ".bbbbwwbbbb.",
        "...ssssss...",
        "...skssks...",
        "...ssssss...",
        "...ssssss...",
        "....ssss....",
        "............",
    ];
    const GHOST: [&str; 12] = [
        "............",
        "....bbbb....",
        "...bbbbbb...",
        "..bbbbbbbb..",
        "..bwwbbwwb..",
        "..bkwbbkwb..",
        "..bbbbbbbb..",
        "..bbbbbbbb..",
        "..bbbbbbbb..",
        "..bbbbbbbb..",
        "..bb.bb.bb..",
        "............",
    ];
    const SLIME: [&str; 12] = [
        "............",
        "............",
        "............",
        ".....bb.....",
        "....bbbb....",
        "...bwbbbb...",
        "..bwbbbbbb..",
        "..bbkbbkbb..",
        ".bbbbbbbbbb.",
        ".bbbbbbbbdb.",
        "..bddbbddd..",
        "............",
    ];
    const ROBOT: [&str; 12] = [
        "............",
        ".....a......",
        ".....k......",
        "..bbbbbbbb..",
        "..bwwbbwwb..",
        "..bwkbbkwb..",
        "..bbbbbbbb..",
        "..bbkkkkbb..",
        "..bbbbbbbb..",
        "...dd..dd...",
        "..ddd..ddd..",
        "............",
    ];
    let kind = k.pick(20, 4);
    let mut map = Map::parse(match kind {
        0 => &MUSHROOM,
        1 => &GHOST,
        2 => &SLIME,
        _ => &ROBOT,
    });
    let body = pico(match kind {
        0 => [8, 9, 14, 12, 2][k.pick(21, 5)],
        1 => [8, 12, 14, 9, 11][k.pick(21, 5)],
        2 => [11, 12, 14, 10][k.pick(21, 4)],
        _ => [6, 12, 9][k.pick(21, 3)],
    });
    if kind == 0 {
        // Each pair of spots shows or not; the middle one always does.
        for y in 2..4 {
            for x in 0..6 {
                if map.get(x, y) == SHINE && !k.coin(22 + (y * 6 + x) as u64) {
                    map.set(x, y, BODY);
                    map.set(11 - x, y, BODY);
                }
            }
        }
    }
    if k.coin(23) {
        map = map.flip();
    }
    let ink = if k.dark { pico(0) } else { pico(1) };
    let colors = [BLACK, ink, body, pico(8), pico(7), pico(7), lerp(body, BLACK, 0.35), pico(15)];
    (map.outline(), colors)
}

/// A potion, gem, heart or sword, in NES colours.
fn loot(k: &Seed) -> (Map, [Rgb; 8]) {
    const POTION: [&str; 12] = [
        "............",
        "....ssss....",
        ".....ll.....",
        ".....ll.....",
        "....llll....",
        "...bbbbbb...",
        "..bwbbbbbb..",
        "..bwbbbbbb..",
        "..bbbbbbdb..",
        "..bbbbbbdb..",
        "...bbbbbb...",
        "............",
    ];
    const GEM: [&str; 12] = [
        "............",
        "............",
        "............",
        "...lllbbb...",
        "..lwllbbbd..",
        ".llllbbbbdd.",
        "..llbbbbdd..",
        "...lbbbbd...",
        "....lbbd....",
        ".....bd.....",
        "............",
        "............",
    ];
    const HEART: [&str; 12] = [
        "............",
        "............",
        "..bbb..bbb..",
        ".bwwbbbbbbb.",
        ".bwbbbbbbbb.",
        ".bbbbbbbbdb.",
        "..bbbbbbdb..",
        "...bbbbbb...",
        "....bbbb....",
        ".....bb.....",
        "............",
        "............",
    ];
    const SWORD: [&str; 12] = [
        "............",
        ".........ww.",
        "........wwl.",
        ".......wwl..",
        "......wwl...",
        "..a..wwl....",
        "...awwl.....",
        "....al......",
        "...s.a......",
        "..s.........",
        ".a..........",
        "............",
    ];
    let kind = k.pick(24, 4);
    let mut map = Map::parse(match kind {
        0 => &POTION,
        1 => &GEM,
        2 => &HEART,
        _ => &SWORD,
    });
    if k.coin(25) {
        map = map.flip();
    }
    let body = tone(NES[k.pick(26, NES.len())]);
    let gold = tone((248, 184, 0));
    let (shine, light) = if kind == 3 { (srgb(252, 252, 252), srgb(188, 188, 188)) } else { (srgb(252, 252, 252), lerp(body, WHITE, 0.5)) };
    let wood = if kind == 3 { body } else { srgb(172, 124, 0) };
    let colors = [BLACK, BLACK, body, gold, shine, light, lerp(body, BLACK, 0.45), wood];
    (map.outline(), colors)
}

/// A mirrored space invader, 11×8: a core, a head, maybe antennae, shoulders and arms, two eyes
/// and seeded legs.
fn invader(k: &Seed) -> Map {
    let mut m = Map::new(11, 8);
    for y in 2..=5 {
        for d in 0..=3 {
            m.pair(y, d, BODY);
        }
    }
    for d in 0..=1 + k.pick(10, 2) as i32 {
        m.pair(1, d, BODY);
    }
    if k.coin(11) {
        m.pair(0, 2 + k.pick(12, 2) as i32, BODY);
    }
    if k.coin(13) {
        m.pair(3, 4, BODY);
        m.pair(4, 4, BODY);
    }
    if k.coin(14) {
        let y = 2 + 2 * k.pick(15, 2) as i32;
        m.pair(y, 5, BODY);
        m.pair(y + 1, 5, BODY);
    }
    if k.coin(16) {
        for d in 1..=3 {
            if k.coin(17 + d as u64) {
                m.pair(5, d, 0);
            }
        }
    }
    m.pair(3, 1 + k.pick(18, 2) as i32, ACCENT);
    let legs: [(&[i32], &[i32]); 5] = [(&[1, 3], &[4]), (&[2, 4], &[1, 5]), (&[3], &[2, 4]), (&[1, 2, 3, 4], &[4]), (&[2], &[3])];
    let (a, b) = legs[k.pick(19, legs.len())];
    for &d in a {
        m.pair(6, d, BODY);
    }
    for &d in b {
        m.pair(7, d, BODY);
    }
    m
}

/// The first letters of the first two words, or the first two letters of one word.
fn initials(seed: &str) -> [char; 2] {
    let words: Vec<&str> = seed.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let mut chars: Vec<char> = if words.len() >= 2 { words.iter().filter_map(|w| w.chars().next()).collect() } else { seed.chars().filter(|c| c.is_alphanumeric()).collect() };
    chars.resize(2, ' ');
    [chars[0], chars[1]]
}

/// A 3×5 bitmap glyph: five rows, the high bit of three on the left.
fn font3(c: char) -> [u8; 5] {
    match c.to_ascii_uppercase() {
        'A' => [0b010, 0b101, 0b111, 0b101, 0b101],
        'B' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'C' => [0b011, 0b100, 0b100, 0b100, 0b011],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'E' => [0b111, 0b100, 0b110, 0b100, 0b111],
        'F' => [0b111, 0b100, 0b110, 0b100, 0b100],
        'G' => [0b011, 0b100, 0b101, 0b101, 0b011],
        'H' => [0b101, 0b101, 0b111, 0b101, 0b101],
        'I' => [0b111, 0b010, 0b010, 0b010, 0b111],
        'J' => [0b001, 0b001, 0b001, 0b101, 0b010],
        'K' => [0b101, 0b101, 0b110, 0b101, 0b101],
        'L' => [0b100, 0b100, 0b100, 0b100, 0b111],
        'M' => [0b101, 0b111, 0b111, 0b101, 0b101],
        'N' => [0b110, 0b101, 0b101, 0b101, 0b101],
        'O' => [0b010, 0b101, 0b101, 0b101, 0b010],
        'P' => [0b110, 0b101, 0b110, 0b100, 0b100],
        'Q' => [0b010, 0b101, 0b101, 0b110, 0b011],
        'R' => [0b110, 0b101, 0b110, 0b101, 0b101],
        'S' => [0b011, 0b100, 0b010, 0b001, 0b110],
        'T' => [0b111, 0b010, 0b010, 0b010, 0b010],
        'U' => [0b101, 0b101, 0b101, 0b101, 0b111],
        'V' => [0b101, 0b101, 0b101, 0b101, 0b010],
        'W' => [0b101, 0b101, 0b111, 0b111, 0b101],
        'X' => [0b101, 0b101, 0b010, 0b101, 0b101],
        'Y' => [0b101, 0b101, 0b010, 0b010, 0b010],
        'Z' => [0b111, 0b001, 0b010, 0b100, 0b111],
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b110, 0b001, 0b010, 0b100, 0b111],
        '3' => [0b110, 0b001, 0b010, 0b001, 0b110],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b110, 0b001, 0b110],
        '6' => [0b011, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b110],
        ' ' => [0; 5],
        _ => [0b110, 0b001, 0b010, 0b000, 0b010],
    }
}

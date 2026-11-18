//! Vector box-drawing, block-element, and underline `Shape` primitives for terminal rows.

use rune::{Color, LineCap, Shape};
use terminal::UnderlineKind;

pub fn draw_special_underline(
    mut shape: Shape,
    kind: UnderlineKind,
    x: f32,
    w: f32,
    h: f32,
    color: Color,
) -> Shape {
    let y = (h - 2.0).max(1.0);
    match kind {
        UnderlineKind::Double => shape
            .line([x, y - 2.0], [x + w, y - 2.0], 1.0, LineCap::Butt, color)
            .line([x, y], [x + w, y], 1.0, LineCap::Butt, color),
        UnderlineKind::Curly => {
            let steps = ((w / 3.0).ceil() as usize).max(2);
            let mut pts = Vec::with_capacity(steps + 1);
            for i in 0..=steps {
                let t = i as f32 / steps as f32;
                let py = y - 1.0 + (t * std::f32::consts::TAU).sin() * 1.3;
                pts.push([x + w * t, py]);
            }
            shape.stroke_with(pts, false, 1.1, LineCap::Round, color)
        }
        UnderlineKind::Dotted => {
            let mut cx = x;
            while cx + 1.5 <= x + w {
                shape = shape.line([cx, y], [cx + 1.5, y], 1.2, LineCap::Butt, color);
                cx += 3.5;
            }
            shape
        }
        UnderlineKind::Dashed => {
            shape.line([x + 0.5, y], [(x + w - 1.0).max(x + 1.0), y], 1.2, LineCap::Butt, color)
        }
        UnderlineKind::None | UnderlineKind::Single => shape,
    }
}

/// Arm weights of U+2500..=U+257F, up, right, down, left: `0` none, `L` light, `H` heavy, `D`
/// double; `.` for the dashes, arcs and diagonals drawn on their own.
const BOX: [&str; 128] = [
    "0L0L", "0H0H", "L0L0", "H0H0", ".", ".", ".", ".", ".", ".", ".", ".", "0LL0", "0HL0", "0LH0", "0HH0",
    "00LL", "00LH", "00HL", "00HH", "LL00", "LH00", "HL00", "HH00", "L00L", "L00H", "H00L", "H00H", "LLL0", "LHL0", "HLL0", "LLH0",
    "HLH0", "HHL0", "LHH0", "HHH0", "L0LL", "L0LH", "H0LL", "L0HL", "H0HL", "H0LH", "L0HH", "H0HH", "0LLL", "0LLH", "0HLL", "0HLH",
    "0LHL", "0LHH", "0HHL", "0HHH", "LL0L", "LL0H", "LH0L", "LH0H", "HL0L", "HL0H", "HH0L", "HH0H", "LLLL", "LLLH", "LHLL", "LHLH",
    "HLLL", "LLHL", "HLHL", "HLLH", "HHLL", "LLHH", "LHHL", "HHLH", "LHHH", "HLHH", "HHHL", "HHHH", ".", ".", ".", ".",
    "0D0D", "D0D0", "0DL0", "0LD0", "0DD0", "00LD", "00DL", "00DD", "LD00", "DL00", "DD00", "L00D", "D00L", "D00D", "LDL0", "DLD0",
    "DDD0", "L0LD", "D0DL", "D0DD", "0DLD", "0LDL", "0DDD", "LD0D", "DL0L", "DD0D", "LDLD", "DLDL", "DDDD", ".", ".", ".",
    ".", ".", ".", ".", "000L", "L000", "0L00", "00L0", "000H", "H000", "0H00", "00H0", "0H0L", "L0H0", "0L0H", "H0L0",
];

/// Draws `ch` into the cell at `x` (`w` by `h` points, `s` device pixels a point) as geometry,
/// for box drawing (U+2500..=U+257F), block elements (U+2580..=U+259F), Braille (U+2800..=U+28FF)
/// and the Powerline separators: every edge on a whole device pixel and every line a whole
/// number of pixels thick, centred the same way in every cell, so lines meet cell to cell at any
/// scale and in any font, as Ghostty's and Windows Terminal's built-in glyphs do. `false` for
/// any other character.
pub fn draw_cell_art(shape: &mut Shape, ch: char, x: f32, w: f32, h: f32, s: f32, color: Color) -> bool {
    let c = ch as u32;
    let (wp, hp) = ((w * s).round().max(1.0), (h * s).round().max(1.0));
    let light = (hp / 18.0).round().max(1.0);
    let heavy = (light * 2.0).max(light + 1.0);
    let mut p = Px { shape, x, s, color };
    match c {
        0x2500..=0x257F => match BOX[(c - 0x2500) as usize].as_bytes() {
            [u, r, d, l] => p.arms([*u, *r, *d, *l].map(|b| match b { b'L' => 1, b'H' => 2, b'D' => 3, _ => 0 }), wp, hp, light, heavy),
            _ => p.special(c, wp, hp, light, heavy),
        },
        0x2580..=0x259F => p.block(c, wp, hp),
        0x2800..=0x28FF => p.braille(c - 0x2800, wp, hp),
        0xE0B0..=0xE0B3 | 0xE0B4 | 0xE0B6 => p.powerline(c, wp, hp, light),
        0x276F => p.stroke(&[[wp * 0.22, hp * 0.29], [wp * 0.72, hp * 0.5], [wp * 0.22, hp * 0.71]], 1.6 * s, LineCap::Round),
        _ => return false,
    }
    true
}

/// A cell's drawing in its own device pixels, written into the row's shape in points.
struct Px<'a> {
    shape: &'a mut Shape,
    x: f32,
    s: f32,
    color: Color,
}

/// Where a band `th` thick starts when centred in `len` (whole pixels, the same in every cell).
fn band(len: f32, th: f32) -> f32 {
    ((len - th) / 2.0).floor()
}

impl Px<'_> {
    fn at(&self, [px, py]: [f32; 2]) -> [f32; 2] {
        [self.x + px / self.s, py / self.s]
    }

    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        self.rect_in(x0, y0, x1, y1, self.color);
    }

    fn rect_in(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: Color) {
        if x1 > x0 && y1 > y0 {
            let pts = vec![self.at([x0, y0]), self.at([x1, y0]), self.at([x1, y1]), self.at([x0, y1])];
            *self.shape = std::mem::take(self.shape).fill(pts, color);
        }
    }

    fn poly(&mut self, pts: &[[f32; 2]]) {
        let pts: Vec<[f32; 2]> = pts.iter().map(|&q| self.at(q)).collect();
        *self.shape = std::mem::take(self.shape).fill(pts, self.color);
    }

    fn stroke(&mut self, pts: &[[f32; 2]], width_px: f32, cap: LineCap) {
        let pts: Vec<[f32; 2]> = pts.iter().map(|&q| self.at(q)).collect();
        *self.shape = std::mem::take(self.shape).stroke_with(pts, false, width_px / self.s, cap, self.color);
    }

    /// Lines from the cell's middle to the edges each arm (`[up, right, down, left]`: 0 none,
    /// 1 light, 2 heavy, 3 double) reaches. Doubles nest as Ghostty draws them: at a corner the
    /// outer lines meet each other and the inner ones each other; where a double crosses or
    /// tees into another, the lines stop at the nearer one instead of crossing it.
    fn arms(&mut self, [u, r, d, l]: [u8; 4], wp: f32, hp: f32, light: f32, heavy: f32) {
        let t = |a: u8| match a {
            1 => light,
            2 => heavy,
            _ => 0.0,
        };
        let (vt, ht) = (t(u).max(t(d)), t(l).max(t(r)));
        let (vd, hd) = (u == 3 || d == 3, l == 3 || r == 3);
        let (hya, vxa) = (band(hp, 3.0 * light), band(wp, 3.0 * light));
        let (hyb, vxb) = (hya + 2.0 * light, vxa + 2.0 * light);
        // Where single and heavy arms reach into the middle: across the other axis's stroke.
        let k = vt.max(ht);
        let (h_end, h_start) = if vd { (vxb + light, vxa) } else { (band(wp, k) + k, band(wp, k)) };
        let (v_end, v_start) = if hd { (hyb + light, hya) } else { (band(hp, k) + k, band(hp, k)) };
        for (arm, side) in [(l, 0), (r, 1)] {
            match arm {
                0 => {}
                3 => {
                    // Each line's end: [line A (top), line B (bottom)].
                    let (ua, da) = (u == 3, d == 3);
                    let mid = if vt > 0.0 { [band(wp, vt), band(wp, vt) + vt] } else { [(wp / 2.0).floor() - 1.0, (wp / 2.0).ceil() + 1.0] };
                    let ends = if side == 0 {
                        match (ua, da) {
                            (true, true) => [vxa + light, vxa + light],
                            (false, true) => [vxb + light, vxa + light],
                            (true, false) => [vxa + light, vxb + light],
                            (false, false) => [mid[1], mid[1]],
                        }
                    } else {
                        match (ua, da) {
                            (true, true) => [vxb, vxb],
                            (false, true) => [vxa, vxb],
                            (true, false) => [vxb, vxa],
                            (false, false) => [mid[0], mid[0]],
                        }
                    };
                    for (y, e) in [(hya, ends[0]), (hyb, ends[1])] {
                        if side == 0 { self.rect(0.0, y, e, y + light) } else { self.rect(e, y, wp, y + light) }
                    }
                }
                a => {
                    let (y, th) = (band(hp, t(a)), t(a));
                    if side == 0 { self.rect(0.0, y, h_end, y + th) } else { self.rect(h_start, y, wp, y + th) }
                }
            }
        }
        for (arm, side) in [(u, 0), (d, 1)] {
            match arm {
                0 => {}
                3 => {
                    let (la, ra) = (l == 3, r == 3);
                    let mid = if ht > 0.0 { [band(hp, ht), band(hp, ht) + ht] } else { [(hp / 2.0).floor() - 1.0, (hp / 2.0).ceil() + 1.0] };
                    let ends = if side == 0 {
                        match (la, ra) {
                            (true, true) => [hya + light, hya + light],
                            (true, false) => [hya + light, hyb + light],
                            (false, true) => [hyb + light, hya + light],
                            (false, false) => [mid[1], mid[1]],
                        }
                    } else {
                        match (la, ra) {
                            (true, true) => [hyb, hyb],
                            (true, false) => [hyb, hya],
                            (false, true) => [hya, hyb],
                            (false, false) => [mid[0], mid[0]],
                        }
                    };
                    for (x, e) in [(vxa, ends[0]), (vxb, ends[1])] {
                        if side == 0 { self.rect(x, 0.0, x + light, e) } else { self.rect(x, e, x + light, hp) }
                    }
                }
                a => {
                    let (x, th) = (band(wp, t(a)), t(a));
                    if side == 0 { self.rect(x, 0.0, x + th, v_end) } else { self.rect(x, v_start, x + th, hp) }
                }
            }
        }
    }

    /// The dashed lines, the rounded corners and the diagonals.
    fn special(&mut self, c: u32, wp: f32, hp: f32, light: f32, heavy: f32) {
        // (horizontal, heavy, dashes)
        let dashes = match c {
            0x2504 => Some((true, false, 3.0)),
            0x2505 => Some((true, true, 3.0)),
            0x2506 => Some((false, false, 3.0)),
            0x2507 => Some((false, true, 3.0)),
            0x2508 => Some((true, false, 4.0)),
            0x2509 => Some((true, true, 4.0)),
            0x250A => Some((false, false, 4.0)),
            0x250B => Some((false, true, 4.0)),
            0x254C => Some((true, false, 2.0)),
            0x254D => Some((true, true, 2.0)),
            0x254E => Some((false, false, 2.0)),
            0x254F => Some((false, true, 2.0)),
            _ => None,
        };
        if let Some((horizontal, is_heavy, n)) = dashes {
            let th = if is_heavy { heavy } else { light };
            let len = if horizontal { wp } else { hp };
            let seg = len / n;
            let gap = (seg * 0.3).round().max(1.0);
            for i in 0..n as u32 {
                let a = (i as f32 * seg + gap / 2.0).round();
                let b = ((i as f32 + 1.0) * seg - gap / 2.0).round();
                if horizontal {
                    let y = band(hp, th);
                    self.rect(a, y, b, y + th);
                } else {
                    let x = band(wp, th);
                    self.rect(x, a, x + th, b);
                }
            }
            return;
        }
        let (cx, cy) = (band(wp, light) + light / 2.0, band(hp, light) + light / 2.0);
        // A quarter of the ellipse about corner `o` through the two edge midpoints, `a0`..`a1` in turns.
        let mut arc = |o: [f32; 2], rx: f32, ry: f32, a0: f32, a1: f32| {
            let pts: Vec<[f32; 2]> = (0..=16)
                .map(|i| {
                    let a = (a0 + (a1 - a0) * i as f32 / 16.0) * std::f32::consts::TAU;
                    [o[0] + rx * a.cos(), o[1] + ry * a.sin()]
                })
                .collect();
            self.stroke(&pts, light, LineCap::Butt);
        };
        match c {
            0x256D => arc([wp, hp], wp - cx, hp - cy, 0.5, 0.75),
            0x256E => arc([0.0, hp], cx, hp - cy, 0.75, 1.0),
            0x256F => arc([0.0, 0.0], cx, cy, 0.0, 0.25),
            0x2570 => arc([wp, 0.0], wp - cx, cy, 0.25, 0.5),
            0x2571 => self.stroke(&[[wp, 0.0], [0.0, hp]], light, LineCap::Butt),
            0x2572 => self.stroke(&[[0.0, 0.0], [wp, hp]], light, LineCap::Butt),
            _ => {
                self.stroke(&[[wp, 0.0], [0.0, hp]], light, LineCap::Butt);
                self.stroke(&[[0.0, 0.0], [wp, hp]], light, LineCap::Butt);
            }
        }
    }

    /// U+2580..=U+259F: halves, eighths, shades and quadrants.
    fn block(&mut self, c: u32, wp: f32, hp: f32) {
        let ey = |k: f32| (hp * k / 8.0).round();
        let ex = |k: f32| (wp * k / 8.0).round();
        let (mx, my) = ((wp / 2.0).round(), (hp / 2.0).round());
        match c {
            0x2580 => self.rect(0.0, 0.0, wp, my),
            0x2581..=0x2588 => self.rect(0.0, hp - ey((c - 0x2580) as f32), wp, hp),
            0x2589..=0x258F => self.rect(0.0, 0.0, ex((0x2590 - c) as f32), hp),
            0x2590 => self.rect(mx, 0.0, wp, hp),
            0x2591..=0x2593 => {
                let shade = self.color.alpha(self.color.a * 0.25 * (c - 0x2590) as f32);
                self.rect_in(0.0, 0.0, wp, hp, shade);
            }
            0x2594 => self.rect(0.0, 0.0, wp, ey(1.0)),
            0x2595 => self.rect(wp - ex(1.0), 0.0, wp, hp),
            _ => {
                // Quadrants: upper left, upper right, lower left, lower right.
                let q = match c {
                    0x2596 => [false, false, true, false],
                    0x2597 => [false, false, false, true],
                    0x2598 => [true, false, false, false],
                    0x2599 => [true, false, true, true],
                    0x259A => [true, false, false, true],
                    0x259B => [true, true, true, false],
                    0x259C => [true, true, false, true],
                    0x259D => [false, true, false, false],
                    0x259E => [false, true, true, false],
                    _ => [false, true, true, true],
                };
                for (on, [x0, y0, x1, y1]) in q.into_iter().zip([[0.0, 0.0, mx, my], [mx, 0.0, wp, my], [0.0, my, mx, hp], [mx, my, wp, hp]]) {
                    if on {
                        self.rect(x0, y0, x1, y1);
                    }
                }
            }
        }
    }

    /// A Braille pattern: dots 1-8 (bits 0-7) in two columns of four.
    fn braille(&mut self, bits: u32, wp: f32, hp: f32) {
        let d = (wp / 4.0).round().max(1.0);
        const DOTS: [(f32, f32); 8] = [(0.0, 0.0), (0.0, 1.0), (0.0, 2.0), (1.0, 0.0), (1.0, 1.0), (1.0, 2.0), (0.0, 3.0), (1.0, 3.0)];
        for (bit, (col, row)) in DOTS.iter().enumerate() {
            if bits & (1 << bit) != 0 {
                let x = (wp * (0.25 + col * 0.5) - d / 2.0).round();
                let y = (hp * (0.125 + row * 0.25) - d / 2.0).round();
                self.rect(x, y, x + d, y + d);
            }
        }
    }

    /// Powerline's separators: solid and thin triangles, and half circles, the cell's full height.
    fn powerline(&mut self, c: u32, wp: f32, hp: f32, light: f32) {
        let half = |from: f32, left: bool| -> Vec<[f32; 2]> {
            (0..=16)
                .map(|i| {
                    let a = (from + 0.5 * i as f32 / 16.0) * std::f32::consts::TAU;
                    [if left { wp } else { 0.0 } + wp * a.cos(), hp / 2.0 + hp / 2.0 * a.sin()]
                })
                .collect()
        };
        match c {
            0xE0B0 => self.poly(&[[0.0, 0.0], [wp, hp / 2.0], [0.0, hp]]),
            0xE0B1 => self.stroke(&[[0.0, 0.0], [wp, hp / 2.0], [0.0, hp]], light, LineCap::Butt),
            0xE0B2 => self.poly(&[[wp, 0.0], [0.0, hp / 2.0], [wp, hp]]),
            0xE0B3 => self.stroke(&[[wp, 0.0], [0.0, hp / 2.0], [wp, hp]], light, LineCap::Butt),
            0xE0B4 => self.poly(&half(-0.25, false)),
            _ => self.poly(&half(0.25, true)),
        }
    }
}

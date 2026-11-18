//! The pin board as filled parallelograms (`pin-board.tsx` `draw`, `drawBoard`, `drawCube`,
//! `readColors`; `pin-quads.ts`): one list for the pins behind the composer, whose first
//! `shadows` quads blend by MAX so overlapping shadows merge instead of darkening, and one for
//! the pins popped toward the viewer.

use super::machine::{PinPhase, close_age};
use super::{Entrance, Pins, chain_of, close_of};

const SHADES: usize = 16;
/// The counter pins' shade out of the board, so they read as cubes.
const LOOSE: usize = 4;
/// The light's direction, (0.55, -0.83) normalised.
const LIGHT: [f64; 2] = [0.552380364401192, -0.8335921862781622];

/// A quad: corner (x, y), edges u and v, premultiplied colour (`pin-quads.ts` `FLOATS`).
pub type Quad = [f32; 10];

#[derive(Default)]
pub struct Quads {
    pub list: Vec<Quad>,
    pub shadows: usize,
}

impl Quads {
    fn begin(&mut self) {
        self.list.clear();
        self.shadows = 0;
    }

    fn push(&mut self, x: f64, y: f64, u: [f64; 2], v: [f64; 2], c: Rgba) {
        let a = c[3];
        self.list.push([x as f32, y as f32, u[0] as f32, u[1] as f32, v[0] as f32, v[1] as f32, (c[0] / 255.0 * a) as f32, (c[1] / 255.0 * a) as f32, (c[2] / 255.0 * a) as f32, a as f32]);
    }

    fn shadow(&mut self, x: f64, y: f64, w: f64, h: f64, c: Rgba) {
        self.push(x, y, [w, 0.0], [0.0, h], c);
        self.shadows = self.list.len();
    }

    fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, c: Rgba) {
        self.push(x, y, [w, 0.0], [0.0, h], c);
    }
}

/// Straight sRGB 0-255 and alpha 0-1, as `pin-quads.ts` takes them.
pub type Rgba = [f64; 4];

/// Theme colours (`readColors`).
#[derive(Clone, Debug)]
pub struct Colors {
    side: [[Rgba; SHADES + 1]; 5],
    face: [[Rgba; SHADES + 1]; 5],
    socket_rim: Rgba,
    socket_floor: Rgba,
    pub shadow: Rgba,
    highlight: Rgba,
    lowlight: Rgba,
}

impl Colors {
    /// From the panel's `bg`, and `icon-icon-faint`, `-muted`, `-base` and `orange-500`.
    pub fn new(bg: [f64; 3], faint: [f64; 3], muted: [f64; 3], base: [f64; 3], brand: [f64; 3]) -> Colors {
        let levels = [bg, mix(bg, faint, 0.5), muted, base, faint];
        let dark = bg[0] * 0.2126 + bg[1] * 0.7152 + bg[2] * 0.0722 < 128.0;
        let black = [0.0; 3];
        let shades = |f: &dyn Fn(f64) -> [f64; 3]| std::array::from_fn(|i| color(f(i as f64 / SHADES as f64), 1.0));
        // Sides are the pin's own colour in shadow, so they stay darker than its face in either theme.
        Colors {
            side: std::array::from_fn(|lv| shades(&|t| mix(black, levels[lv], 0.3 + 0.5 * t))),
            face: std::array::from_fn(|lv| shades(&|t| mix(levels[lv], brand, t * if lv == 1 { 0.4 } else { 0.85 }))),
            socket_rim: color(mix(bg, black, if dark { 0.6 } else { 0.16 }), 1.0),
            socket_floor: color(mix(bg, black, if dark { 0.35 } else { 0.08 }), 1.0),
            shadow: color(black, if dark { 0.3 } else { 0.12 }),
            highlight: color([255.0; 3], 0.2),
            lowlight: color(black, 0.2),
        }
    }
}

fn mix(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn color(c: [f64; 3], alpha: f64) -> Rgba {
    [js_round(c[0]), js_round(c[1]), js_round(c[2]), alpha]
}

/// `Math.round`: halves go up, also below zero.
pub fn js_round(v: f64) -> f64 {
    (v + 0.5).floor()
}

fn shade_of(v: f64) -> usize {
    js_round(v.clamp(0.0, 1.0) * SHADES as f64) as usize
}

/// Scratch kept between frames so drawing allocates nothing once warm.
#[derive(Default)]
pub struct Scratch {
    band: Vec<usize>,
    /// The painter's order for each tilt quadrant, sorted once.
    orders: [Vec<usize>; 4],
    loose: Vec<(usize, f64)>,
    flat: Vec<usize>,
}

impl Pins {
    /// Both layers' quads for `now` (`draw`). The back list is in the region's points; true when
    /// the front layer has anything.
    pub fn draw(&mut self, now: f64, colors: &Colors, back: &mut Quads, front: &mut Quads, s: &mut Scratch) -> bool {
        back.begin();
        self.draw_board(now, colors, back, s);
        let v = self.view;
        s.loose.clear();
        s.loose.extend(self.pins.iter().enumerate().filter(|(_, p)| !Pins::resting(p)).map(|(i, p)| (i, Pins::pop_of(p, now))));
        for &(i, pop) in &s.loose {
            let p = &self.pins[i];
            let offset = v.depth * (0.35 + pop * 5.0);
            let size = v.size * (1.0 + pop * 1.6) * (1.0 + pop * 0.4);
            back.rect(
                p.x + self.tilt[0] * v.depth - LIGHT[0] * offset - size / 2.0,
                p.y + self.tilt[1] * v.depth - LIGHT[1] * offset - size / 2.0,
                size,
                size,
                [0.0, 0.0, 0.0, colors.shadow[3] * (1.0 - pop * 0.65)],
            );
        }
        // Cubes further along the extrusion direction sit in front, so stacked pins overlap correctly.
        let (tx, ty) = (self.tilt[0], self.tilt[1]);
        let mut flat = std::mem::take(&mut s.flat);
        flat.clear();
        flat.extend(s.loose.iter().filter(|(_, pop)| *pop <= 0.06).map(|(i, _)| *i));
        flat.sort_by(|&a, &b| {
            let (a, b) = (&self.pins[a], &self.pins[b]);
            (a.x * tx + a.y * ty - (b.x * tx + b.y * ty)).total_cmp(&0.0)
        });
        for &i in &flat {
            self.draw_cube(back, i, 1.0, colors);
        }
        s.flat = flat;
        front.begin();
        s.loose.retain(|(_, pop)| *pop > 0.06);
        s.loose.sort_by(|a, b| a.1.total_cmp(&b.1));
        for k in 0..s.loose.len() {
            let (i, pop) = s.loose[k];
            self.draw_cube(front, i, 1.0 + pop * 1.6, colors);
        }
        !s.loose.is_empty()
    }

    /// Pins are prisms from their front face back to the board. Drawing them in order of their
    /// grid position along the extrusion direction is a correct painter's order for any heights,
    /// since pins on the same diagonal never overlap.
    fn draw_board(&mut self, now: f64, colors: &Colors, quads: &mut Quads, s: &mut Scratch) {
        let v = self.view;
        let (sx, sy) = (if self.tilt[0] >= 0.0 { 1.0 } else { -1.0 }, if self.tilt[1] >= 0.0 { 1.0 } else { -1.0 });
        let (ox, oy) = (self.tilt[0] * v.depth, self.tilt[1] * v.depth);
        let half = v.size / 2.0;
        let size = v.size;
        let age = self.age(now);
        let close = close_age(self.board, now);
        let sweep = self.entrance == Entrance::Sweep;
        let reach = |p: &super::machine::Pin| if close >= 0.0 { close_of(p, close) } else if sweep { if age >= (f64::from(p.c) / super::COLS) * 0.45 { 1.0 } else { 0.0 } } else { chain_of(p, age) };
        for p in &self.pins {
            if p.phase != PinPhase::Home {
                continue;
            }
            let g = reach(p);
            if g <= 0.05 {
                continue;
            }
            let k = g.min(1.0);
            let sh = colors.shadow;
            quads.shadow(p.hx + ox * k - LIGHT[0] * 2.5 * k - half - 1.0, p.hy + oy * k - LIGHT[1] * 2.5 * k - half - 1.0, size + 2.0, size + 2.0, [sh[0], sh[1], sh[2], sh[3] * k]);
        }
        // An empty socket: a dark rim with its floor visible through the opening, shifted by the tilt.
        for p in &self.pins {
            if p.phase != PinPhase::Home {
                quads.rect(p.hx + ox - half, p.hy + oy - half, size, size, colors.socket_rim);
            }
        }
        for p in &self.pins {
            if p.phase == PinPhase::Home {
                continue;
            }
            let (w, h) = (size - (ox * 0.45).abs(), size - (oy * 0.45).abs());
            if w > 0.0 && h > 0.0 {
                quads.rect(p.hx + ox - half + (ox * 0.45).max(0.0), p.hy + oy - half + (oy * 0.45).max(0.0), w, h, colors.socket_floor);
            }
        }
        let key = usize::from(sx > 0.0) + if sy > 0.0 { 2 } else { 0 };
        if s.orders[key].is_empty() {
            let mut order: Vec<usize> = (0..self.pins.len()).collect();
            let pins = &self.pins;
            order.sort_by(|&a, &b| {
                let (a, b) = (&pins[a], &pins[b]);
                (f64::from(a.c) * sx + f64::from(a.r) * sy - (f64::from(b.c) * sx + f64::from(b.r) * sy)).total_cmp(&0.0)
            });
            s.orders[key] = order;
        }
        let side_x = if sx > 0.0 { js_round(LIGHT[0].max(0.0) * SHADES as f64) as usize } else { 0 };
        let side_y = if sy < 0.0 { js_round((-LIGHT[1]).max(0.0) * SHADES as f64) as usize } else { 0 };
        let mut current = f64::NAN;
        s.band.clear();
        let order = std::mem::take(&mut s.orders[key]);
        for &i in &order {
            let p = &mut self.pins[i];
            if p.phase != PinPhase::Home {
                continue;
            }
            let height = (if p.lv == 1 { 0.35 } else { 1.0 }) * (1.0 - p.z.max(0.0)) + (-p.z).max(0.0) * 0.6;
            if sweep && close < 0.0 {
                let grow = ((age - (f64::from(p.c) / super::COLS) * 0.45) / 0.3).clamp(0.0, 1.0);
                if grow == 0.0 {
                    continue;
                }
                p.bd = 1.0;
                p.fd = 1.0 - height * (1.0 - (1.0 - grow).powi(3));
            }
            if !sweep || close >= 0.0 {
                let w = if close >= 0.0 { close_of(p, close) } else { chain_of(p, age) };
                if w <= 0.01 {
                    continue;
                }
                let g = w.min(1.0);
                p.bd = g;
                p.fd = (1.0 - height) * g;
                p.e = p.e.max((g * std::f64::consts::PI).sin() * 0.85);
            }
            let k = f64::from(p.c) * sx + f64::from(p.r) * sy;
            if k != current && !s.band.is_empty() {
                self.flush(quads, &s.band, colors, [sx, sy], [ox, oy], [side_x, side_y]);
                s.band.clear();
            }
            current = k;
            s.band.push(i);
        }
        if !s.band.is_empty() {
            self.flush(quads, &s.band, colors, [sx, sy], [ox, oy], [side_x, side_y]);
        }
        s.orders[key] = order;
    }

    /// Each diagonal goes out grouped by colour, as Desktop's 2D fallback fills each run as one path.
    fn flush(&self, quads: &mut Quads, band: &[usize], colors: &Colors, sgn: [f64; 2], o: [f64; 2], side: [usize; 2]) {
        let v = self.view;
        let (half, size) = (v.size / 2.0, v.size);
        let [sx, sy] = sgn;
        let [ox, oy] = o;
        for lv in 1..=3u8 {
            for p in band.iter().map(|&i| &self.pins[i]).filter(|p| p.lv == lv) {
                let fx = p.hx + ox * p.fd + sx * half;
                quads.push(fx, p.hy + oy * p.fd - half, [ox * (p.bd - p.fd), oy * (p.bd - p.fd)], [0.0, size], colors.side[usize::from(lv)][side[0]]);
            }
            for p in band.iter().map(|&i| &self.pins[i]).filter(|p| p.lv == lv) {
                let fy = p.hy + oy * p.fd + sy * half;
                quads.push(p.hx + ox * p.fd - half, fy, [size, 0.0], [ox * (p.bd - p.fd), oy * (p.bd - p.fd)], colors.side[usize::from(lv)][side[1]]);
            }
        }
        for p in band.iter().map(|&i| &self.pins[i]) {
            quads.rect(p.hx + ox * p.fd - half, p.hy + oy * p.fd - half, size, size, colors.face[usize::from(p.lv)][shade_of(p.e.max(p.z))]);
        }
        if size < 4.0 {
            return;
        }
        // The lit and shaded edges are split at the corners so no pixel is tinted twice by one of them.
        for p in band.iter().map(|&i| &self.pins[i]).filter(|p| p.lv != 1) {
            let (x, y) = (p.hx + ox * p.fd - half, p.hy + oy * p.fd - half);
            quads.rect(x, y, size, 1.0, colors.highlight);
            quads.rect(x + size - 1.0, y + 1.0, 1.0, size - 1.0, colors.highlight);
        }
        for p in band.iter().map(|&i| &self.pins[i]).filter(|p| p.lv != 1) {
            let (x, y) = (p.hx + ox * p.fd - half, p.hy + oy * p.fd - half);
            quads.rect(x, y + size - 1.0, size, 1.0, colors.lowlight);
            quads.rect(x, y, 1.0, size - 1.0, colors.lowlight);
        }
    }

    /// Loose pins are extruded squares. Each visible side is lit by its own normal, so tumbling
    /// pins flicker. Counter pins are recessed and nearly black on the board; out of it they use
    /// the loose shade so they read as cubes.
    fn draw_cube(&self, layer: &mut Quads, i: usize, scale: f64, colors: &Colors) {
        let p = &self.pins[i];
        let v = self.view;
        let lv = if p.lv == 1 { LOOSE } else { usize::from(p.lv) };
        let radius = (v.size * scale) / std::f64::consts::SQRT_2;
        let mut corners = [[0.0f64; 2]; 4];
        for (j, c) in corners.iter_mut().enumerate() {
            let a = p.rot + std::f64::consts::FRAC_PI_4 + (j as f64 * std::f64::consts::PI) / 2.0;
            *c = [p.x + a.cos() * radius, p.y + a.sin() * radius];
        }
        let (ox, oy) = (self.tilt[0] * v.depth * scale, self.tilt[1] * v.depth * scale);
        for j in 0..4 {
            let [ax, ay] = corners[j];
            let [bx, by] = corners[(j + 1) % 4];
            let (nx, ny) = ((ax + bx) / 2.0 - p.x, (ay + by) / 2.0 - p.y);
            if nx * self.tilt[0] + ny * self.tilt[1] <= 0.0 {
                continue;
            }
            let n = match nx.hypot(ny) { 0.0 => 1.0, n => n };
            let shade = js_round(((nx * LIGHT[0] + ny * LIGHT[1]) / n).max(0.0) * SHADES as f64) as usize;
            layer.push(ax, ay, [bx - ax, by - ay], [ox, oy], colors.side[lv][shade]);
        }
        let [x, y] = corners[0];
        layer.push(x, y, [corners[1][0] - x, corners[1][1] - y], [corners[3][0] - x, corners[3][1] - y], colors.face[lv][shade_of(p.e)]);
    }
}

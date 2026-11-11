//! Pixel buffers in linear light with premultiplied alpha.

use image::{ImageBuffer, Rgba, RgbaImage, imageops::FilterType};

pub(crate) fn lin(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub(crate) fn enc(c: f32) -> f32 {
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

pub(crate) struct Buf {
    pub w: u32,
    pub h: u32,
    pub px: Vec<[f32; 4]>,
}

impl Buf {
    pub(crate) fn new(w: u32, h: u32) -> Buf {
        Buf { w, h, px: vec![[0.0; 4]; (w * h) as usize] }
    }

    pub(crate) fn from_rgba8(img: &RgbaImage) -> Buf {
        let table: Vec<f32> = (0..256).map(|i| lin(i as f32 / 255.0)).collect();
        let px = img.pixels().map(|p| {
            let a = p[3] as f32 / 255.0;
            [table[p[0] as usize] * a, table[p[1] as usize] * a, table[p[2] as usize] * a, a]
        });
        Buf { w: img.width(), h: img.height(), px: px.collect() }
    }

    pub(crate) fn to_rgba8(&self) -> Vec<u8> {
        let q = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
        self.px
            .iter()
            .flat_map(|&[r, g, b, a]| {
                if a <= 1e-4 {
                    return [0, 0, 0, 0];
                }
                [q(enc(r / a)), q(enc(g / a)), q(enc(b / a)), q(a)]
            })
            .collect()
    }

    pub(crate) fn coverage(&self) -> f32 {
        self.px.iter().map(|p| p[3]).sum::<f32>() / self.px.len().max(1) as f32
    }

    /// Crops to the visible pixels, as a centred square.
    pub(crate) fn trim(self) -> Buf {
        let (mut x0, mut y0, mut x1, mut y1) = (self.w, self.h, 0, 0);
        for y in 0..self.h {
            for x in 0..self.w {
                if self.px[(y * self.w + x) as usize][3] > 0.03 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        if x1 <= x0 || y1 <= y0 {
            return self;
        }
        let side = (x1 - x0).max(y1 - y0);
        if side == self.w && side == self.h {
            return self;
        }
        let (cx, cy) = ((x0 + x1) as i64, (y0 + y1) as i64);
        let (ox, oy) = ((cx - side as i64) / 2, (cy - side as i64) / 2);
        let mut out = Buf::new(side, side);
        out.over(&self, -ox, -oy);
        out
    }

    /// The centred square a square box shows of it under `object-fit: cover`.
    pub(crate) fn cover(self) -> Buf {
        let side = self.w.min(self.h);
        if self.w == self.h {
            return self;
        }
        let mut out = Buf::new(side, side);
        out.over(&self, -(((self.w - side) / 2) as i64), -(((self.h - side) / 2) as i64));
        out
    }

    /// Draws `src` over this at (`ox`, `oy`).
    pub(crate) fn over(&mut self, src: &Buf, ox: i64, oy: i64) {
        for y in 0..src.h as i64 {
            let ty = y + oy;
            if ty < 0 || ty >= self.h as i64 {
                continue;
            }
            for x in 0..src.w as i64 {
                let tx = x + ox;
                if tx < 0 || tx >= self.w as i64 {
                    continue;
                }
                let s = src.px[(y * src.w as i64 + x) as usize];
                let d = &mut self.px[(ty * self.w as i64 + tx) as usize];
                for c in 0..4 {
                    d[c] = s[c] + d[c] * (1.0 - s[3]);
                }
            }
        }
    }

    /// Halves with a box filter while that stays at least twice the target, then Lanczos.
    pub(crate) fn resize(self, w: u32, h: u32) -> Buf {
        let mut b = self;
        while b.w >= w * 4 && b.h >= h * 4 {
            b = b.halve();
        }
        if b.w == w && b.h == h {
            return b;
        }
        let flat: Vec<f32> = b.px.iter().flatten().copied().collect();
        let img: ImageBuffer<Rgba<f32>, Vec<f32>> = ImageBuffer::from_raw(b.w, b.h, flat).expect("buffer size");
        let out = image::imageops::resize(&img, w, h, FilterType::Lanczos3);
        let raw = out.into_raw();
        let px = raw.chunks_exact(4).map(|c| {
            let a = c[3].clamp(0.0, 1.0);
            [c[0].clamp(0.0, a), c[1].clamp(0.0, a), c[2].clamp(0.0, a), a]
        });
        Buf { w, h, px: px.collect() }
    }

    pub(crate) fn halve(&self) -> Buf {
        let (w, h) = (self.w / 2, self.h / 2);
        let mut out = Buf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let at = |dx: u32, dy: u32| self.px[((y * 2 + dy) * self.w + x * 2 + dx) as usize];
                let (a, b, c, d) = (at(0, 0), at(1, 0), at(0, 1), at(1, 1));
                out.px[(y * w + x) as usize] = std::array::from_fn(|i| (a[i] + b[i] + c[i] + d[i]) * 0.25);
            }
        }
        out
    }
}

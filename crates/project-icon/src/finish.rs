//! How a tile catches the light: flat, as Desktop draws it, or lit like a sweet. Candy puts a
//! glossy band across the top third, deepens the body toward the bottom and sets the tile on a
//! drop shadow; Glass is OpenCode iOS's drawer tile (`DrawerListProjectTile`), a soft highlight
//! and a fine fading edge. The light is one bitmap per size, drawn over the image or the letter,
//! so every tile of a size shares it.

use rune::{Color, El, Shadow};

use crate::Palette;
use crate::styles::{RADIUS, lerp, round_box, smooth};

/// How project icons catch the light.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Finish {
    /// Desktop's tile: its sheen and half-point ring, no shadow.
    #[default]
    Flat,
    /// A glossy band across the top third, a body deeper at the bottom, a drop shadow.
    Candy,
    /// OpenCode iOS's lit glass: a soft highlight from the top, a white edge fading down.
    Glass,
}

impl Finish {
    pub const ALL: [Finish; 3] = [Finish::Candy, Finish::Glass, Finish::Flat];
    pub const NAMES: [&str; 3] = ["Candy", "Glass", "Flat"];

    pub fn name(self) -> &'static str {
        Self::NAMES[Self::ALL.iter().position(|f| *f == self).unwrap_or(2)]
    }

    pub(crate) fn lit(self) -> bool {
        self != Finish::Flat
    }

    /// Keeps Desktop's half-point ring in the avatar's border colour; lit tiles draw their edge.
    pub(crate) fn ringed(self) -> bool {
        self == Finish::Flat
    }
}

/// The tile's shadows at iOS's 20 pt scale: a contact shadow under an ambient one (iOS's own
/// for Glass, deeper for Candy), half as deep again in dark mode, where shadows read less.
pub(crate) fn shadows(finish: Finish, tile: El, size_pt: f32, dark: bool) -> El {
    let k = size_pt / 20.0;
    let depth = if dark { 1.5 } else { 1.0 };
    let black = |a: f32| Color::BLACK.alpha((a * depth).min(1.0));
    // `blur` is CSS's (2σ), twice `CALayer.shadowRadius`.
    let (ambient, contact) = match finish {
        Finish::Candy => ((1.5 * k, 3.5 * k, 0.3), (0.75 * k, 1.25 * k, 0.4)),
        _ => ((1.5 * k, 3.5 * k, 0.14), (0.5 * k, 1.0 * k, 0.3)),
    };
    tile.box_shadow(Shadow { y: ambient.0, blur: ambient.1, color: black(ambient.2), ..Shadow::default() })
        .box_shadow(Shadow { y: contact.0, blur: contact.1, color: black(contact.2), ..Shadow::default() })
}

/// A letter tile's body under the light, straight-alpha sRGB, without a ring: Glass's flat avatar
/// colour, or Candy's, lighter at the top and richer at the bottom (darkened, never greyed).
pub(crate) fn body(p: &Palette, variant: usize, finish: Finish, px: u32) -> Vec<u8> {
    let srgb = |c: Color| [c.r, c.g, c.b].map(crate::pixels::enc);
    let bg = srgb(p.avatars[variant].0);
    let (top, bottom) = match finish {
        Finish::Candy => (lerp(bg, [1.0; 3], 0.14), bg.map(|c| c * 0.76)),
        _ => (bg, bg),
    };
    tile(px, |_, v| {
        let col = lerp(top, bottom, smooth(0.0, 1.0, v));
        [col[0], col[1], col[2], 1.0]
    })
}

/// The light over a tile `size_pt` wide, straight-alpha sRGB. Images carry their own detail, so
/// they take a softer highlight (iOS: 0.6 of it; Candy keeps more, so its band still reads).
pub(crate) fn light(finish: Finish, image: bool, size_pt: f32, px: u32) -> Vec<u8> {
    let ring = 0.5 / size_pt;
    tile(px, |u, v| {
        let edge = round_box(u - 0.5, v - 0.5, 0.5, RADIUS) > -ring;
        match finish {
            Finish::Candy => {
                let soft = if image { 0.8 } else { 1.0 };
                let shade = 0.24 * smooth(0.55, 1.0, v);
                let top = if edge { 0.5 * (1.0 - smooth(0.0, 0.5, v)) } else { band(u, v) * soft };
                over([1.0; 3], top, [0.0; 3], shade)
            }
            Finish::Glass => {
                let soft = if image { 0.6 } else { 1.0 };
                let hl = specular(u, v) * 0.3 * soft;
                let e = if edge { 0.6 * (1.0 - 0.85 * v) } else { 0.0 };
                [1.0, 1.0, 1.0, e + hl * (1.0 - e)]
            }
            Finish::Flat => [1.0, 1.0, 1.0, 0.0],
        }
    })
}

/// `a` at alpha `aa` over `b` at alpha `ba`, straight alpha.
fn over(a: [f32; 3], aa: f32, b: [f32; 3], ba: f32) -> [f32; 4] {
    let out = aa + ba * (1.0 - aa);
    if out <= 0.0 {
        return [0.0; 4];
    }
    let mix = |i: usize| (a[i] * aa + b[i] * ba * (1.0 - aa)) / out;
    [mix(0), mix(1), mix(2), out]
}

/// Candy's gloss: an ellipse from above the tile whose lower edge crosses a third of the way down
/// at the sides and a little lower in the middle, inset from the edge, brightest at the top.
fn band(u: f32, v: f32) -> f32 {
    let inset = 0.06;
    let inside = round_box(u - 0.5, v - 0.5, 0.5 - inset, RADIUS - inset);
    let t = ((u - 0.5) / 0.98).hypot((v + 0.36) / 0.8);
    let cover = (1.0 - smooth(0.975, 1.0, t)) * (1.0 - smooth(-0.015, 0.0, inside));
    cover * (0.55 - 0.38 * (v / 0.44).clamp(0.0, 1.0))
}

/// iOS's specular spot: centred just above the top edge, 1.3 × 0.75 sides, its white falling to
/// 0.42 by 42% of the way out and to nothing by 72%.
fn specular(u: f32, v: f32) -> f32 {
    let t = ((u - 0.5) / 1.3).hypot((v + 0.12) / 0.75);
    if t < 0.42 {
        1.0 - 0.58 * t / 0.42
    } else {
        (0.42 * (1.0 - (t - 0.42) / 0.3)).max(0.0)
    }
}

/// A rounded tile `px` square, shaded per point in tile units with 4 × 4 samples a pixel, as
/// straight-alpha sRGB bytes; samples outside the corners are clear.
fn tile(px: u32, shade: impl Fn(f32, f32) -> [f32; 4]) -> Vec<u8> {
    let n = 4;
    let q = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut out = Vec::with_capacity((px * px * 4) as usize);
    for py in 0..px {
        for x in 0..px {
            let mut acc = [0.0f32; 4];
            for sy in 0..n {
                for sx in 0..n {
                    let u = (x as f32 + (sx as f32 + 0.5) / n as f32) / px as f32;
                    let v = (py as f32 + (sy as f32 + 0.5) / n as f32) / px as f32;
                    if round_box(u - 0.5, v - 0.5, 0.5, RADIUS) > 0.0 {
                        continue;
                    }
                    let [r, g, b, a] = shade(u, v);
                    acc = [acc[0] + r * a, acc[1] + g * a, acc[2] + b * a, acc[3] + a];
                }
            }
            if acc[3] <= 0.0 {
                out.extend([0; 4]);
                continue;
            }
            out.extend([q(acc[0] / acc[3]), q(acc[1] / acc[3]), q(acc[2] / acc[3]), q(acc[3] / (n * n) as f32)]);
        }
    }
    out
}

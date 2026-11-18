//! The 16 pt project face with room for its unread dot. Cut the coverage of the tile itself,
//! rather than masking a subtree: every row then samples a small ordinary image. The gap is
//! transparent, including over a selected row. The dot and the letter remain native elements.

use super::*;
use rune::Pixels;

#[derive(Hash, PartialEq, Eq)]
enum Art { Letter, Tile([u8; 32]), Mark([u8; 32]), Vector(u64) }

#[derive(Hash, PartialEq, Eq)]
struct Key {
    art: Art,
    px: u32,
    finish: Finish,
    bg: [u32; 4],
    border: [u32; 4],
}

#[derive(Default)]
pub(super) struct Tiles(HashMap<Key, Pixels>);

fn color(c: Color) -> [u32; 4] { [c.r, c.g, c.b, c.a].map(f32::to_bits) }

impl Icons {
    /// Desktop's session badge (`project-avatar.css`, `[data-unread]`). Letter tiles of the
    /// same colour share their notched face, independent of the letter. Logos keep their art.
    pub fn unread_face<'a>(&self, pal: &Palette, project: impl Into<Project<'a>>) -> El {
        const PT: f32 = 16.0;
        let p = project.into();
        let size = (PT * self.scale()).round().clamp(1.0, 256.0) as u32;
        let variant = chosen_color(p.color).unwrap_or(8);
        let image = self.found(&p);
        let (art, src) = match &image {
            None => (Art::Letter, None),
            Some(Shown::Tile(i, src)) => (Art::Tile(i.id()), Some(src)),
            Some(Shown::Mark(i, src)) => (Art::Mark(i.id()), Some(src)),
            Some(Shown::Vector(_, hash, src)) => (Art::Vector(*hash), Some(src)),
        };
        let mark = matches!(art, Art::Mark(_));
        let (bg, border) = if mark { (pal.tile, pal.ring) } else { pal.avatars[variant] };
        let finish = self.finish();
        let key = Key { art, px: size, finish, bg: color(bg), border: color(border) };
        let mut cache = self.unread.borrow_mut();
        if cache.0.len() >= 128 && !cache.0.contains_key(&key) { cache.0.clear(); }
        let face = cache.0.entry(key).or_insert_with(|| {
            let pixels = Pixels::new();
            let mut rgba = if src.is_none() && finish.lit() {
                finish::body(pal, variant, finish, size)
            } else { styles::draw(pal, "", variant, &Look::style(Style::Desktop), PT, size) };
            if let Some(src) = src {
                let side = if mark { (size as f32 * 0.75).round().max(1.0) as u32 } else { size };
                let logo = Buf::from_rgba8(src).resize(side, side).to_rgba8();
                let offset = (size - side) / 2;
                let rgb = [bg.r, bg.g, bg.b].map(|c| pixels::enc(c) * 255.0);
                let edge = [border.r, border.g, border.b].map(|c| pixels::enc(c) * 255.0);
                for (i, dest) in rgba.chunks_exact_mut(4).enumerate() {
                    let (x, y) = (i as u32 % size, i as u32 / size);
                    let (u, v) = ((x as f32 + 0.5) / size as f32, (y as f32 + 0.5) / size as f32);
                    let inset = -styles::round_box(u - 0.5, v - 0.5, 0.5, styles::RADIUS) * PT;
                    let ring = ((0.5 - inset) * size as f32 / PT + 0.5).clamp(0.0, 1.0) * border.a;
                    let sample = (x >= offset && y >= offset && x < offset + side && y < offset + side)
                        .then(|| &logo[(((y-offset)*side+x-offset)*4) as usize..]);
                    for k in 0..3 {
                        let base = rgb[k] + (edge[k] - rgb[k]) * ring;
                        dest[k] = sample.map_or(base, |s| base + (f32::from(s[k])-base)*f32::from(s[3])/255.0).round() as u8;
                    }
                }
            }
            let light = finish.lit().then(|| finish::light(finish, src.is_some(), PT, size));
            for (i, p) in rgba.chunks_exact_mut(4).enumerate() {
                if let Some(light) = &light {
                    let l = &light[i * 4..];
                    for k in 0..3 { p[k] = ((u32::from(p[k])*(255-u32::from(l[3]))+u32::from(l[k])*u32::from(l[3])+127)/255) as u8; }
                }
                let dx = PT - (i as u32 % size) as f32 * PT / size as f32 - 0.5 * PT / size as f32;
                let dy = (i as u32 / size) as f32 * PT / size as f32 + 0.5 * PT / size as f32;
                p[3] = (f32::from(p[3]) * (dx.hypot(dy)-4.0).clamp(0.0, 1.0)).round() as u8;
            }
            pixels.set(size, size, rgba);
            pixels
        });
        // Keep the face's usual outer-edge coverage (`Icons::mark` and `Icons::picture`).
        // The unread mask changes the top-right corner, not the other three corners.
        let mut tile = el().size(PT, PT).radius(PT * styles::RADIUS).pixels_fit(face, SurfaceFit::Fill);
        if src.is_none() {
            let name = if p.name.is_empty() { opencode_kit::path::basename(p.directory) } else { p.name };
            let first = name.chars().next().or_else(|| p.id.chars().next());
            let letter = first.map_or_else(|| "?".into(), |c| c.to_uppercase().to_string());
            tile = tile.center().child(text(letter).font(TextStyle::new(11.0, 530.0).lh(PT)).color(pal.fg));
        }
        self.lift(pal, tile, PT)
    }
}

/// A small, centre-cropped copy of a vector logo for the notched tile. Decode on the existing
/// icon worker, never on the key that first puts an unread project on screen.
pub(super) fn svg_miniature(bytes: &[u8]) -> Option<image::RgbaImage> {
    let rgba = rune::image::svg_rgba_cover(bytes, PX).ok()?;
    image::RgbaImage::from_raw(PX, PX, rgba)
}

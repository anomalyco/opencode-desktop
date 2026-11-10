//! A spreadsheet cell's text in its own family, weight and slant, as Desktop's canvas draws it
//! (`canvas2d.ts` `fontString`: `[italic] [bold] <size>px "<family>", sans-serif`): the family's
//! bundled face or its metric-compatible substitute (Carlito for Calibri), else Carlito. A face
//! that lacks the weight or slant asked for is emboldened or sheared, as `xlsx-raster` does.
//! After BetterOffice's native viewer (`xlsx_scene.rs`, Apache-2.0).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use rustybuzz::{Face, GlyphBuffer, UnicodeBuffer};
use vello::Scene;
use vello::Glyph;
use vello::kurbo::{Affine, Rect};
use vello::peniko::{Blob, Color, Fill, FontData};
use xlsx_render::Align;

use super::scene::{finite, positive_f32};
use crate::shared::strict_color;

const BOLD_OFFSET_PX: f32 = 0.35;
const ITALIC_SHEAR: f64 = 0.21;
pub const PX_PER_PT: f32 = 96.0 / 72.0;

/// A face a cell draws with: its shaping tables, its bytes as Vello draws them, and whether it is
/// bold and italic itself.
pub struct CellFace {
    face: Face<'static>,
    pub font: FontData,
    bold: bool,
    italic: bool,
}

static PACKS: LazyLock<crate::fonts::FontPacks> = LazyLock::new(|| crate::fonts::FontPacks::new(None));
/// Each family, weight and slant asked for, and the face it resolved to.
type Faces = HashMap<(String, bool, bool), Option<&'static CellFace>>;

static FACES: LazyLock<Mutex<Faces>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The face for `family` in `bold` and `italic`; Carlito when the family has none. Each face loads
/// once and lives for the process, as a workbook names only a few.
pub fn cell_face(family: Option<&str>, bold: bool, italic: bool) -> Option<&'static CellFace> {
    let family = family.map(str::trim).filter(|f| !f.is_empty()).unwrap_or("Calibri");
    let key = (family.to_ascii_lowercase(), bold, italic);
    let mut faces = FACES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(face) = faces.get(&key) {
        return *face;
    }
    let face = load(family, bold, italic).or_else(|| load("Calibri", bold, italic));
    faces.insert(key, face);
    face
}

fn load(family: &str, bold: bool, italic: bool) -> Option<&'static CellFace> {
    let (handle, bytes) = PACKS.face(family, bold, italic, betteroffice_fonts::LastResortOffice::Word)?;
    let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let face = Face::from_slice(bytes, 0)?;
    Some(Box::leak(Box::new(CellFace { face, font: FontData::new(Blob::from(bytes.to_vec()), 0), bold: handle.bold, italic: handle.italic })))
}
pub struct PreparedText {
    face: &'static CellFace,
    baseline: f32,
    pen: f32,
    font_size_px: f32,
    scale: f32,
    run_width: f32,
    glyphs: Vec<Glyph>,
    color: Color,
    highlight: Option<Color>,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    dashed_underline: bool,
}

impl PreparedText {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        face: &'static CellFace,
        x: f32,
        y: f32,
        text: &str,
        font_size_pt: f32,
        color: &str,
        align: Align,
        bold: bool,
        italic: bool,
        underline: bool,
        strike: bool,
        highlight: Option<&str>,
        dashed_underline: bool,
    ) -> Result<Self, String> {
        let x = finite(x, "text x")?;
        let baseline = finite(y, "text y")?;
        let font_size_pt = positive_f32(font_size_pt, "text font size")?;
        let font_size_px = font_size_pt * PX_PER_PT;
        let scale = font_size_px / face.face.units_per_em() as f32;
        let shaped = shape(&face.face, text);
        let total: i32 = shaped
            .glyph_positions()
            .iter()
            .map(|position| position.x_advance)
            .sum();
        let run_width = total as f32 * scale;
        let pen = match align {
            Align::Left => x,
            Align::Center => x - run_width / 2.0,
            Align::Right => x - run_width,
        };
        let mut glyph_pen = 0.0;
        let glyphs = shaped
            .glyph_infos()
            .iter()
            .zip(shaped.glyph_positions())
            .map(|(info, position)| {
                let glyph = Glyph {
                    id: info.glyph_id,
                    x: glyph_pen + position.x_offset as f32 * scale,
                    y: -(position.y_offset as f32) * scale,
                };
                glyph_pen += position.x_advance as f32 * scale;
                glyph
            })
            .collect();
        Ok(Self {
            face,
            baseline,
            pen,
            font_size_px,
            scale,
            run_width,
            glyphs,
            color: strict_color(color)?,
            highlight: highlight.map(strict_color).transpose()?,
            // Only a face without the weight or slant asked for gets the synthetic one.
            bold: bold && !face.bold,
            italic: italic && !face.italic,
            underline,
            strike,
            dashed_underline,
        })
    }

    pub fn paint(&self, scene: &mut Scene) {
        self.paint_highlight(scene);
        self.paint_glyphs(scene, self.pen);
        if self.bold {
            self.paint_glyphs(scene, self.pen + BOLD_OFFSET_PX);
        }
        self.paint_decorations(scene);
    }

    fn paint_highlight(&self, scene: &mut Scene) {
        let Some(color) = self.highlight else {
            return;
        };
        let face = &self.face.face;
        let ascent = face.ascender() as f32 * self.scale;
        let descent = -face.descender() as f32 * self.scale;
        let rect = Rect::new(
            f64::from(self.pen - 2.0),
            f64::from(self.baseline - ascent - 1.0),
            f64::from(self.pen + self.run_width + 2.0),
            f64::from(self.baseline + descent + 1.0),
        );
        scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, &rect);
    }

    fn paint_glyphs(&self, scene: &mut Scene, pen: f32) {
        let glyph_transform = self.italic.then_some(Affine::skew(ITALIC_SHEAR, 0.0));
        scene
            .draw_glyphs(&self.face.font)
            .font_size(self.font_size_px)
            .brush(self.color)
            .glyph_transform(glyph_transform)
            .transform(Affine::translate((
                f64::from(pen),
                f64::from(self.baseline),
            )))
            .draw(Fill::NonZero, self.glyphs.iter().copied());
    }

    fn paint_decorations(&self, scene: &mut Scene) {
        if self.run_width <= 0.0 {
            return;
        }
        let face = &self.face.face;
        let em = face.units_per_em() as f32;
        if self.underline {
            let metrics = face.underline_metrics();
            let position = metrics
                .map(|metrics| metrics.position as f32)
                .unwrap_or(-0.1 * em);
            let thickness = metrics
                .map(|metrics| metrics.thickness as f32)
                .unwrap_or(0.05 * em);
            self.paint_bar(scene, position, thickness);
        }
        if self.strike {
            let metrics = face.strikeout_metrics();
            let position = metrics
                .map(|metrics| metrics.position as f32)
                .unwrap_or(0.26 * em);
            let thickness = metrics
                .map(|metrics| metrics.thickness as f32)
                .unwrap_or(0.05 * em);
            self.paint_bar(scene, position, thickness);
        }
        if self.dashed_underline {
            let metrics = face.underline_metrics();
            let position = metrics
                .map(|metrics| metrics.position as f32)
                .unwrap_or(-0.1 * em);
            let thickness = metrics
                .map(|metrics| metrics.thickness as f32)
                .unwrap_or(0.05 * em);
            let height = (thickness * self.scale).max(0.5);
            let y = self.baseline - position * self.scale - height / 2.0;
            let dash = 3.0_f32.max(height * 2.0);
            let gap = 2.0_f32.max(height);
            let mut x = self.pen;
            let end = x + self.run_width;
            while x < end {
                let right = x + dash.min(end - x);
                let rect = Rect::new(
                    f64::from(x),
                    f64::from(y),
                    f64::from(right),
                    f64::from(y + height),
                );
                scene.fill(Fill::NonZero, Affine::IDENTITY, self.color, None, &rect);
                x += dash + gap;
            }
        }
    }

    fn paint_bar(&self, scene: &mut Scene, center_up: f32, thickness: f32) {
        let height = (thickness * self.scale).max(0.5);
        let center = self.baseline - center_up * self.scale;
        let rect = Rect::new(
            f64::from(self.pen),
            f64::from(center - height / 2.0),
            f64::from(self.pen + self.run_width),
            f64::from(center + height / 2.0),
        );
        scene.fill(Fill::NonZero, Affine::IDENTITY, self.color, None, &rect);
    }
}

pub fn measure_text(face: &CellFace, text: &str, font_size_pt: f32) -> f32 {
    let face = &face.face;
    let advance: i32 = shape(face, text)
        .glyph_positions()
        .iter()
        .map(|position| position.x_advance)
        .sum();
    advance as f32 * font_size_pt * PX_PER_PT / face.units_per_em() as f32
}

fn shape(face: &Face<'static>, text: &str) -> GlyphBuffer {
    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(text);
    rustybuzz::shape(face, &[], buffer)
}


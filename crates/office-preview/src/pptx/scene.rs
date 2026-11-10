//! A slide's display list as a Vello scene: its background, then each primitive in order, charts
//! and tables clipped to their frames. An item that can't be drawn is left out and counted. After
//! BetterOffice's native viewer (`pptx_scene.rs`, Apache-2.0); placeholders draw as
//! `pptx-raster` draws them.

use std::collections::HashMap;

use betteroffice_pptx::{ImageCrop, Paint, PositionedTextLine, Primitive, Stroke as DisplayStroke, SurfaceDisplayList, Transform};
use ooxml_drawingml::GeometryPathCommand;
use vello::kurbo::{Affine, Rect, Stroke};
use vello::peniko::{Color, Fill, FontData, ImageBrush, ImageData};
use vello::{Glyph, Scene};

use super::paint::{Frame, build_path, finite, image_mapping, image_outline, nonnegative, positioned_glyph, positive, prepare_paint, prepare_stroke, primitive_kind};
use crate::shared::{PageScene, Skipped, color, with_dashes};

const PLACEHOLDER_STROKE: Color = Color::from_rgb8(0x8a, 0x94, 0xa6);
const PLACEHOLDER_LABEL: Color = Color::from_rgb8(0x5d, 0x66, 0x75);
const PLACEHOLDER_LABEL_PX: f32 = 12.0;

/// A deck's picture by id, decoded with its effects.
pub type Images<'a> = &'a dyn Fn(&str, &[pptx_render::ImageEffect]) -> Result<ImageData, String>;

/// What a slide's display list refers to by id: the faces registered with the layout, the deck's
/// decoded pictures, and the face placeholder labels draw with.
pub struct Resources<'a> {
    pub fonts: &'a HashMap<u32, FontData>,
    pub images: Images<'a>,
    pub label: Option<&'a (FontData, Vec<u8>)>,
}

pub fn translate_slide(list: &SurfaceDisplayList, resources: &Resources<'_>) -> Result<PageScene, String> {
    let width = f64::from(positive(list.width, "slide width")?);
    let height = f64::from(positive(list.height, "slide height")?);
    let mut t = Translator { scene: Scene::new(), resources, skipped: Skipped::default() };
    let bounds = Rect::new(0.0, 0.0, width, height);
    t.scene.fill(Fill::NonZero, Affine::IDENTITY, Color::WHITE, None, &bounds);
    if let Some(paint) = &list.background {
        match prepare_paint(paint, bounds) {
            Ok(paint) => paint.fill(&mut t.scene, Affine::IDENTITY, &bounds, Fill::NonZero),
            Err(reason) => t.skipped.record("background", reason),
        }
    }
    for primitive in &list.primitives {
        t.primitive(primitive, Affine::IDENTITY);
    }
    Ok(PageScene { scene: t.scene, width, height, skipped: t.skipped })
}

struct Translator<'a> {
    scene: Scene,
    resources: &'a Resources<'a>,
    skipped: Skipped,
}

struct PictureSource<'a> {
    asset_id: Option<&'a str>,
    effects: &'a [pptx_render::ImageEffect],
    crop: ImageCrop,
    destination: Option<pptx_render::ImageDestination>,
    path: Option<&'a [GeometryPathCommand]>,
}

impl Translator<'_> {
    fn primitive(&mut self, primitive: &Primitive, parent: Affine) {
        let result = match primitive {
            Primitive::Chart { x, y, w, h, primitives, transform, .. } | Primitive::Table { x, y, w, h, primitives, transform, .. } => {
                Frame::new(*x, *y, *w, *h).and_then(|frame| self.container(frame, primitives, *transform, parent))
            }
            Primitive::Shape { x, y, w, h, path, fill, stroke, transform, clip, even_odd, .. } => {
                Frame::new(*x, *y, *w, *h).and_then(|frame| self.shape(frame, path, fill.as_ref(), stroke.as_ref(), (*transform, parent), clip.as_deref(), *even_odd))
            }
            Primitive::Image { x, y, w, h, asset_id, effects, crop, destination, path, stroke, transform, .. } => Frame::new(*x, *y, *w, *h).and_then(|frame| {
                let source = PictureSource { asset_id: asset_id.as_deref(), effects, crop: *crop, destination: *destination, path: path.as_deref() };
                self.image(frame, source, stroke.as_ref(), *transform, parent)
            }),
            Primitive::TextBox { x, y, w, h, paragraphs, lines, overflow, transform, .. } => Frame::new(*x, *y, *w, *h).and_then(|frame| {
                let has_text = paragraphs.iter().flat_map(|p| &p.runs).any(|run| !run.text.is_empty());
                self.text_box(frame, has_text, lines, *overflow, *transform, parent)
            }),
            Primitive::Placeholder { x, y, w, h, label, transform, .. } => Frame::new(*x, *y, *w, *h).and_then(|frame| self.placeholder(frame, label.as_deref(), *transform, parent)),
        };
        if let Err(reason) = result {
            self.skipped.record(primitive_kind(primitive), reason);
        }
    }

    /// A chart's or table's children, clipped to its frame.
    fn container(&mut self, frame: Frame, primitives: &[Primitive], transform: Transform, parent: Affine) -> Result<(), String> {
        let affine = frame.transform(transform, parent)?;
        self.scene.push_clip_layer(Fill::NonZero, affine, &frame.rect);
        for primitive in primitives {
            self.primitive(primitive, affine);
        }
        self.scene.pop_layer();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn shape(&mut self, frame: Frame, commands: &[GeometryPathCommand], fill: Option<&Paint>, stroke: Option<&DisplayStroke>, (transform, parent): (Transform, Affine), clip: Option<&[GeometryPathCommand]>, even_odd: bool) -> Result<(), String> {
        let path = build_path(commands, frame)?;
        if path.is_empty() && (fill.is_some() || stroke.is_some()) {
            return Err("shape path is empty".to_owned());
        }
        let fill = fill.map(|paint| prepare_paint(paint, frame.rect)).transpose()?;
        let stroke = stroke.map(|stroke| prepare_stroke(stroke, frame.rect)).transpose()?;
        let affine = frame.transform(transform, parent)?;
        if let Some(clip) = clip {
            self.scene.push_clip_layer(Fill::NonZero, affine, &build_path(clip, frame)?);
        }
        if let Some(fill) = fill {
            fill.fill(&mut self.scene, affine, &path, if even_odd { Fill::EvenOdd } else { Fill::NonZero });
        }
        if let Some((style, paint)) = stroke
            && style.width > 0.0
        {
            paint.stroke(&mut self.scene, &style, affine, &path);
        }
        if clip.is_some() {
            self.scene.pop_layer();
        }
        Ok(())
    }

    fn image(&mut self, frame: Frame, source: PictureSource<'_>, stroke: Option<&DisplayStroke>, transform: Transform, parent: Affine) -> Result<(), String> {
        let asset_id = source.asset_id.ok_or_else(|| "image has no asset id".to_owned())?;
        let image = (self.resources.images)(asset_id, source.effects)?;
        if image.width == 0 || image.height == 0 {
            return Err(format!("image asset {asset_id} has no pixels"));
        }
        let stroke = stroke.map(|stroke| prepare_stroke(stroke, frame.rect)).transpose()?;
        let affine = frame.transform(transform, parent)?;
        let outline = image_outline(frame, source.path)?;
        let bitmap = source.destination.map_or(Ok(frame), |rect| Frame::new(rect.x, rect.y, rect.w, rect.h))?;
        let mapping = image_mapping(bitmap, image.width, image.height, source.crop)?;
        self.scene.push_clip_layer(Fill::NonZero, affine, &outline);
        self.scene.draw_image(&ImageBrush::new(image), affine * mapping);
        self.scene.pop_layer();
        if let Some((style, paint)) = stroke
            && style.width > 0.0
        {
            paint.stroke(&mut self.scene, &style, affine, &outline);
        }
        Ok(())
    }

    fn text_box(&mut self, frame: Frame, has_text: bool, lines: &[PositionedTextLine], overflow: bool, transform: Transform, parent: Affine) -> Result<(), String> {
        if has_text && !lines.iter().any(|line| !line.runs.is_empty()) {
            return Err("text box has text but no positioned runs".to_owned());
        }
        let affine = frame.transform(transform, parent)?;
        let mut runs = Vec::new();
        for line in lines {
            finite(line.baseline, "text baseline")?;
            for run in &line.runs {
                let font = self.resources.fonts.get(&run.font_id).ok_or_else(|| format!("font id {} was not registered", run.font_id))?;
                let glyphs = run.glyphs.iter().map(positioned_glyph).collect::<Result<Vec<Glyph>, String>>()?;
                let size = positive(run.font_size_px, "text font size")?;
                let baseline = line.baseline - finite(run.baseline_offset_px, "text baseline offset")?;
                runs.push((font, glyphs, size, color(&run.color, 1.0)?, finite(run.x, "text run x")?, baseline, nonnegative(run.width, "text run width")?, run.underline));
            }
        }
        if !overflow {
            self.scene.push_clip_layer(Fill::NonZero, affine, &frame.rect);
        }
        for (font, glyphs, size, ink, x, baseline, width, underline) in runs {
            self.scene.draw_glyphs(font).font_size(size).brush(ink).transform(affine).draw(Fill::NonZero, glyphs.into_iter());
            if underline && width > 0.0 {
                let top = baseline + size * 0.08;
                let thickness = (size * 0.05).max(1.0);
                self.scene.fill(Fill::NonZero, affine, ink, None, &Rect::new(f64::from(x), f64::from(top), f64::from(x + width), f64::from(top + thickness)));
            }
        }
        if !overflow {
            self.scene.pop_layer();
        }
        Ok(())
    }

    /// A dashed box with its label centred in it.
    fn placeholder(&mut self, frame: Frame, label: Option<&str>, transform: Transform, parent: Affine) -> Result<(), String> {
        let affine = frame.transform(transform, parent)?;
        self.scene.stroke(&with_dashes(Stroke::new(1.0), [5.0, 4.0]), affine, PLACEHOLDER_STROKE, None, &frame.rect);
        let (Some(label), Some((font, bytes))) = (label, self.resources.label) else { return Ok(()) };
        let face = rustybuzz::Face::from_slice(bytes, 0).ok_or("placeholder label face is invalid")?;
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(label);
        let shaped = rustybuzz::shape(&face, &[], buffer);
        let scale = PLACEHOLDER_LABEL_PX / f32::from(face.units_per_em() as u16);
        let mut pen = 0.0f32;
        let glyphs: Vec<Glyph> = shaped
            .glyph_infos()
            .iter()
            .zip(shaped.glyph_positions())
            .map(|(info, pos)| {
                let glyph = Glyph { id: info.glyph_id, x: pen + pos.x_offset as f32 * scale, y: -(pos.y_offset as f32) * scale };
                pen += pos.x_advance as f32 * scale;
                glyph
            })
            .collect();
        let ascent = f32::from(face.ascender()) * scale;
        let descent = f32::from(face.descender()) * scale;
        let x = frame.rect.center().x - f64::from(pen) / 2.0;
        let y = frame.rect.center().y + f64::from(ascent + descent) / 2.0;
        self.scene.draw_glyphs(font).font_size(PLACEHOLDER_LABEL_PX).brush(PLACEHOLDER_LABEL).transform(affine * Affine::translate((x, y))).draw(Fill::NonZero, glyphs.into_iter());
        Ok(())
    }
}

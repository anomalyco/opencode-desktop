//! A Word page's display list as a Vello scene: the page's paper, its back borders, watermark,
//! headers and footers, body, notes and front borders, in the painter's order. An item that can't
//! be drawn is left out and counted. After BetterOffice's native viewer (`scene.rs`, Apache-2.0).

use anyhow::Result;
use docx_layout::display_list::{DisplayBorderStyle, DisplayPage, GlyphRunPrimitive, PageBorderZOrder, Primitive, TextRunPrimitive};
use docx_raster::{ImageScope, NoteKind};
use vello::kurbo::{Affine, Line, Rect, Stroke};
use vello::peniko::Fill;
use vello::{Glyph, Scene};

use super::fonts::FontRegistry;
use super::images::ImageRegistry;
use super::paint::{
    apply_border_dash, clip_rect, draw_image, draw_line, draw_shape, finite_f32, glyph_bounds, number, optional_number, primitive_attrs, primitive_kind, primitive_opacity,
    rect_from_numbers, text_clip_rect, translate_border, validate_visual_fields, visual_transform,
};
use crate::shared::{PageScene, Skipped, color, with_clip_layers, with_dashes};

/// Page `page`: its scene, from its paper up, in CSS px.
pub fn translate_page(page: &DisplayPage, fonts: &FontRegistry, images: &ImageRegistry) -> Result<PageScene> {
    let width = number(&page.width).map_err(anyhow::Error::msg)?;
    let height = number(&page.height).map_err(anyhow::Error::msg)?;
    let mut scene = Scene::new();
    let paper = color(page.background.as_deref().unwrap_or("#ffffff"), 1.0).map_err(anyhow::Error::msg)?;
    scene.fill(Fill::NonZero, Affine::IDENTITY, paper, None, &Rect::new(0.0, 0.0, width, height));
    let mut page_cx = PageCx { scene, fonts, images, width, height, skipped: Skipped::default() };
    for border in page.page_borders.iter().filter(|b| b.z_order == Some(PageBorderZOrder::Back)) {
        if let Err(reason) = translate_border(&mut page_cx.scene, border) {
            page_cx.skipped.record("pageBorder", reason);
        }
    }
    let (watermark, body) = page.primitives.split_at(page.watermark_primitive_count.unwrap_or(0).min(page.primitives.len()));
    watermark.iter().for_each(|p| page_cx.draw(p, ImageScope::Body));
    for region in [&page.header, &page.footer].into_iter().flatten() {
        region.primitives.iter().for_each(|p| page_cx.draw(p, ImageScope::HeaderFooter(&region.r_id)));
    }
    body.iter().for_each(|p| page_cx.draw(p, ImageScope::Body));
    for area in &page.note_areas {
        let scope = ImageScope::Notes(if area.kind.as_deref() == Some("endnote") { NoteKind::Endnote } else { NoteKind::Footnote });
        area.separator_primitives.iter().chain(&area.primitives).for_each(|p| page_cx.draw(p, scope));
    }
    for border in page.page_borders.iter().filter(|b| b.z_order != Some(PageBorderZOrder::Back)) {
        if let Err(reason) = translate_border(&mut page_cx.scene, border) {
            page_cx.skipped.record("pageBorder", reason);
        }
    }
    Ok(PageScene { scene: page_cx.scene, width, height, skipped: page_cx.skipped })
}

struct PageCx<'a> {
    scene: Scene,
    fonts: &'a FontRegistry,
    images: &'a ImageRegistry,
    width: f64,
    height: f64,
    skipped: Skipped,
}

impl PageCx<'_> {
    fn draw(&mut self, primitive: &Primitive, scope: ImageScope<'_>) {
        if let Err(reason) = translate_primitive(&mut self.scene, primitive, self.fonts, self.images, scope, self.width, self.height) {
            self.skipped.record(primitive_kind(primitive), reason);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn translate_primitive(
    scene: &mut Scene,
    primitive: &Primitive,
    fonts: &FontRegistry,
    images: &ImageRegistry,
    scope: ImageScope<'_>,
    page_width: f64,
    page_height: f64,
) -> Result<(), String> {
    validate_visual_fields(primitive)?;
    let attrs = primitive_attrs(primitive);
    let mut layers = Vec::with_capacity(2);
    if let Some(clip) = attrs
        .clip_group
        .as_ref()
        .and_then(|group| group.clip.as_ref())
    {
        let rect = clip_rect(clip, page_width, page_height)?;
        layers.push((Affine::IDENTITY, rect));
    }
    match primitive {
        Primitive::Text(run) => {
            if let Some(clip) = &run.paint_clip {
                let rect = text_clip_rect(clip, page_width, page_height)?;
                layers.push((Affine::IDENTITY, rect));
            }
        }
        Primitive::GlyphRun(run) => {
            if let Some(clip) = &run.paint_clip {
                let rect = text_clip_rect(clip, page_width, page_height)?;
                layers.push((Affine::IDENTITY, rect));
            }
        }
        _ => {}
    }
    with_clip_layers(scene, &layers, |scene| match primitive {
        Primitive::Text(run) => draw_text(scene, run, fonts, primitive_opacity(primitive)?),
        Primitive::GlyphRun(run) => {
            draw_glyph_run(scene, run, fonts, primitive_opacity(primitive)?)
        }
        Primitive::Rect(rect) => {
            let bounds = rect_from_numbers(&rect.x, &rect.y, &rect.w, &rect.h)?;
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                color(&rect.fill, primitive_opacity(primitive)?)?,
                None,
                &bounds,
            );
            Ok(())
        }
        Primitive::Line(line) => draw_line(scene, line, primitive_opacity(primitive)?),
        Primitive::Image(image) => {
            draw_image(scene, image, images, scope, primitive_opacity(primitive)?)
        }
        Primitive::Shape(shape) => draw_shape(scene, shape, primitive_opacity(primitive)?),
        Primitive::Decoration(decoration) => {
            let rect =
                rect_from_numbers(&decoration.x, &decoration.y, &decoration.w, &decoration.h)?;
            let opacity = primitive_opacity(primitive)?;
            let styled = decoration
                .attrs
                .style
                .filter(|style| *style != DisplayBorderStyle::Solid);
            if styled.is_some() || decoration.dashed || decoration.dotted {
                let thickness = rect.height().max(1.0);
                let mut stroke = Stroke::new(thickness);
                if decoration.dotted {
                    stroke = with_dashes(stroke, [thickness, thickness * 2.0]);
                } else if decoration.dashed {
                    stroke = with_dashes(stroke, [thickness * 2.0, thickness * 2.0]);
                } else if let Some(style) = styled {
                    stroke = apply_border_dash(stroke, style, thickness)?;
                }
                let y = rect.y0 + rect.height() / 2.0;
                scene.stroke(
                    &stroke,
                    Affine::IDENTITY,
                    color(&decoration.color, opacity)?,
                    None,
                    &Line::new((rect.x0, y), (rect.x1, y)),
                );
            } else {
                scene.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    color(&decoration.color, opacity)?,
                    None,
                    &rect,
                );
            }
            Ok(())
        }
    })
}

fn draw_text(
    scene: &mut Scene,
    run: &TextRunPrimitive,
    fonts: &FontRegistry,
    opacity: f32,
) -> Result<(), String> {
    let text = if run.all_caps {
        run.text.to_uppercase()
    } else {
        run.text.clone()
    };
    if text.is_empty() {
        return Ok(());
    }
    let letter_spacing = optional_number(run.letter_spacing.as_ref())?.unwrap_or(0.0) as f32;
    let word_spacing = optional_number(run.word_spacing.as_ref())?.unwrap_or(0.0) as f32;
    let (size, runs) = fonts
        .shape_fallback(
            &text,
            &run.font,
            run.rtl == Some(true),
            run.small_caps,
            letter_spacing,
            word_spacing,
        )
        .map_err(|error| error.to_string())?;
    let x = number(&run.x)?;
    let baseline = number(&run.baseline_y)?;
    let width = number(&run.width)?.max(0.0);
    let bounds = Rect::new(
        x,
        baseline - f64::from(size) * 0.8,
        x + width,
        baseline + f64::from(size) * 0.4,
    );
    let visual = visual_transform(
        bounds,
        run.rotation_deg.as_ref(),
        run.horizontal_scale.as_ref(),
    )?;
    for (font_id, glyphs) in runs {
        let face = fonts
            .face(font_id)
            .ok_or_else(|| format!("text fallback font id {font_id} is unavailable"))?;
        scene
            .draw_glyphs(face)
            .font_size(size)
            .brush(color(&run.color, 1.0)?)
            .brush_alpha(opacity)
            .transform(visual * Affine::translate((x, baseline)))
            .draw(Fill::NonZero, glyphs.into_iter());
    }
    Ok(())
}

fn draw_glyph_run(
    scene: &mut Scene,
    run: &GlyphRunPrimitive,
    fonts: &FontRegistry,
    opacity: f32,
) -> Result<(), String> {
    if run.glyphs.is_empty() {
        return Ok(());
    }
    if !run.size.is_finite() || run.size <= 0.0 || run.size > f32::MAX as f64 {
        return Err("glyphRun size is invalid".to_owned());
    }
    let face = fonts
        .face(run.font_id)
        .ok_or_else(|| format!("glyphRun font id {} is unavailable", run.font_id))?;
    let glyphs = run
        .glyphs
        .iter()
        .map(|glyph| {
            Ok(Glyph {
                id: glyph.id,
                x: finite_f32(glyph.x, "glyph x")?,
                y: finite_f32(glyph.y, "glyph y")?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let bounds = glyph_bounds(run)?;
    let transform = visual_transform(
        bounds,
        run.rotation_deg.as_ref(),
        run.horizontal_scale.as_ref(),
    )?;
    scene
        .draw_glyphs(face)
        .font_size(run.size as f32)
        .brush(color(&run.color, 1.0)?)
        .brush_alpha(opacity)
        .transform(transform)
        .draw(Fill::NonZero, glyphs.into_iter());
    Ok(())
}


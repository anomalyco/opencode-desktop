//! A slide's frames, paths, paints and pictures as Vello takes them. After BetterOffice's native
//! viewer (`pptx_scene.rs`, Apache-2.0).

use betteroffice_pptx::{GradientType, ImageCrop, Paint, PositionedGlyph, Primitive, Stroke as DisplayStroke, Transform};
use ooxml_drawingml::GeometryPathCommand;
use vello::kurbo::{Affine, BezPath, Rect, Shape, Stroke};
use vello::peniko::{Color, Fill, Gradient};
use vello::{Glyph, Scene};

use crate::shared::{color, with_dashes};

pub enum PreparedPaint {
    Solid(Color),
    Gradient(Gradient),
}

impl PreparedPaint {
    pub fn stroke(&self, scene: &mut Scene, style: &Stroke, transform: Affine, shape: &impl Shape) {
        match self {
            Self::Solid(color) => scene.stroke(style, transform, *color, None, shape),
            Self::Gradient(gradient) => scene.stroke(style, transform, gradient, None, shape),
        }
    }

    pub fn fill(&self, scene: &mut Scene, transform: Affine, shape: &impl Shape, rule: Fill) {
        match self {
            Self::Solid(color) => scene.fill(rule, transform, *color, None, shape),
            Self::Gradient(gradient) => scene.fill(rule, transform, gradient, None, shape),
        }
    }
}

/// A primitive's box before its rotation and flips.
#[derive(Clone, Copy)]
pub struct Frame {
    pub rect: Rect,
}

impl Frame {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Result<Self, String> {
        let (x, y) = (finite(x, "primitive x")?, finite(y, "primitive y")?);
        let (w, h) = (nonnegative(w, "primitive width")?, nonnegative(h, "primitive height")?);
        Ok(Self { rect: Rect::new(f64::from(x), f64::from(y), f64::from(x + w), f64::from(y + h)) })
    }

    /// `parent`, then the primitive's rotation and flips about its centre.
    pub fn transform(self, transform: Transform, parent: Affine) -> Result<Affine, String> {
        let rotation = finite(transform.rotation_deg, "primitive rotation")?;
        let center = self.rect.center();
        let flip = Affine::translate(center.to_vec2())
            * Affine::scale_non_uniform(if transform.flip_h { -1.0 } else { 1.0 }, if transform.flip_v { -1.0 } else { 1.0 })
            * Affine::translate(-center.to_vec2());
        Ok(parent * Affine::rotate_about(f64::from(rotation).to_radians(), center) * flip)
    }
}

/// A paint over `bounds`. PowerPoint's rectangular and path gradients spread from the shape's
/// centre, which a radial gradient approximates.
pub fn prepare_paint(paint: &Paint, bounds: Rect) -> Result<PreparedPaint, String> {
    let (gradient_type, angle_deg, stops) = match paint {
        Paint::Solid { color: value } => return Ok(PreparedPaint::Solid(color(value, 1.0)?)),
        Paint::Gradient { gradient_type, angle_deg, stops } => (gradient_type, angle_deg, stops),
    };
    if stops.is_empty() {
        return Err("gradient has no stops".to_owned());
    }
    let stops = stops
        .iter()
        .map(|stop| if stop.position.is_finite() { Ok((stop.position.clamp(0.0, 1.0), color(&stop.color, 1.0)?)) } else { Err("gradient stop position is not finite".to_owned()) })
        .collect::<Result<Vec<_>, String>>()?;
    let center = bounds.center();
    let radius = bounds.width().hypot(bounds.height()) / 2.0;
    let gradient = match gradient_type {
        GradientType::Linear => {
            let radians = f64::from(finite(angle_deg.unwrap_or_default(), "gradient angle")?).to_radians();
            let (dx, dy) = (radians.cos() * radius, radians.sin() * radius);
            Gradient::new_linear((center.x - dx, center.y - dy), (center.x + dx, center.y + dy)).with_stops(stops.as_slice())
        }
        GradientType::Radial | GradientType::Rectangular | GradientType::Path => Gradient::new_radial(center, radius as f32).with_stops(stops.as_slice()),
    };
    Ok(PreparedPaint::Gradient(gradient))
}

pub fn prepare_stroke(stroke: &DisplayStroke, bounds: Rect) -> Result<(Stroke, PreparedPaint), String> {
    let width = nonnegative(stroke.width, "stroke width")?;
    let mut style = Stroke::new(f64::from(width));
    style.join = match stroke.join.as_deref() {
        Some("round") => vello::kurbo::Join::Round,
        Some("bevel") => vello::kurbo::Join::Bevel,
        _ => vello::kurbo::Join::Miter,
    };
    if stroke.dashed && width > 0.0 {
        style = with_dashes(style, [f64::from((width * 2.0).max(3.0)), f64::from(width.max(2.0))]);
    }
    let paint = match &stroke.paint {
        Some(paint) => prepare_paint(paint, bounds)?,
        None => PreparedPaint::Solid(color(&stroke.color, 1.0)?),
    };
    Ok((style, paint))
}

/// A normalized geometry path in `frame`.
pub fn build_path(commands: &[GeometryPathCommand], frame: Frame) -> Result<BezPath, String> {
    let (x, y, w, h) = (frame.rect.x0, frame.rect.y0, frame.rect.width(), frame.rect.height());
    let point = |px: f64, py: f64| -> Result<(f64, f64), String> {
        if !px.is_finite() || !py.is_finite() {
            return Err("shape path coordinate is not finite".to_owned());
        }
        Ok((x + px * w, y + py * h))
    };
    let mut path = BezPath::new();
    for command in commands {
        match *command {
            GeometryPathCommand::Move { x, y } => path.move_to(point(x, y)?),
            GeometryPathCommand::Line { x, y } => path.line_to(point(x, y)?),
            GeometryPathCommand::Quad { cpx, cpy, x, y } => path.quad_to(point(cpx, cpy)?, point(x, y)?),
            GeometryPathCommand::Cubic { cp1x, cp1y, cp2x, cp2y, x, y } => path.curve_to(point(cp1x, cp1y)?, point(cp2x, cp2y)?, point(x, y)?),
            GeometryPathCommand::Close => path.close_path(),
        }
    }
    Ok(path)
}

/// The picture's own outline when it has one, else its frame.
pub fn image_outline(frame: Frame, path: Option<&[GeometryPathCommand]>) -> Result<BezPath, String> {
    let Some(commands) = path else { return Ok(frame.rect.to_path(0.1)) };
    let outline = build_path(commands, frame)?;
    if outline.is_empty() {
        return Err("image mask path is empty".to_owned());
    }
    Ok(outline)
}

/// Maps the kept part of the source bitmap onto the frame.
pub fn image_mapping(frame: Frame, width: u32, height: u32, crop: ImageCrop) -> Result<Affine, String> {
    let (kept_x, kept_y) = crop.kept();
    if [crop.left, crop.top, crop.right, crop.bottom].iter().any(|edge| !edge.is_finite() || *edge < 0.0 || *edge > 1.0) || kept_x <= 0.0 || kept_y <= 0.0 {
        return Err("image crop is invalid".to_owned());
    }
    let (source_width, source_height) = (f64::from(width), f64::from(height));
    let (kept_width, kept_height) = (f64::from(kept_x) * source_width, f64::from(kept_y) * source_height);
    Ok(Affine::translate((frame.rect.x0, frame.rect.y0))
        * Affine::scale_non_uniform(frame.rect.width() / kept_width, frame.rect.height() / kept_height)
        * Affine::translate((-f64::from(crop.left) * source_width, -f64::from(crop.top) * source_height)))
}

pub fn primitive_kind(primitive: &Primitive) -> &'static str {
    match primitive {
        Primitive::Shape { .. } => "shape",
        Primitive::Image { .. } => "image",
        Primitive::TextBox { .. } => "textBox",
        Primitive::Placeholder { .. } => "placeholder",
        Primitive::Chart { .. } => "chart",
        Primitive::Table { .. } => "table",
    }
}

pub fn positioned_glyph(glyph: &PositionedGlyph) -> Result<Glyph, String> {
    let x = finite(glyph.x, "glyph x")?;
    let x_offset = finite(glyph.x_offset, "glyph x offset")?;
    finite(glyph.advance, "glyph advance")?;
    Ok(Glyph { id: glyph.glyph_id, x: finite(x + x_offset, "positioned glyph x")?, y: finite(glyph.y_offset, "glyph y offset")? })
}

pub fn finite(value: f32, label: &str) -> Result<f32, String> {
    if value.is_finite() { Ok(value) } else { Err(format!("{label} is not finite")) }
}

pub fn nonnegative(value: f32, label: &str) -> Result<f32, String> {
    let value = finite(value, label)?;
    if value >= 0.0 { Ok(value) } else { Err(format!("{label} is negative")) }
}

pub fn positive(value: f32, label: &str) -> Result<f32, String> {
    let value = finite(value, label)?;
    if value > 0.0 { Ok(value) } else { Err(format!("{label} is not positive")) }
}

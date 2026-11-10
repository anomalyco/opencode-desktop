//! A worksheet frame's display list as a Vello scene: fills, patterns, gridlines and borders,
//! paths and cell text, each clipped as the list says. A command that can't be drawn is left out
//! and counted. After BetterOffice's native viewer (`xlsx_scene.rs`, Apache-2.0).

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use vello::Scene;
use vello::kurbo::{Affine, BezPath, Cap, Line, Rect, Stroke};
use vello::peniko::{Blob, Color, Extend, Fill, ImageAlphaType, ImageBrush, ImageData, ImageFormat, ImageQuality};
use xlsx_render::{Align, DisplayList, DrawCmd, GeometryPathCommand, PatternType, Rect as DisplayRect, border_dash};

use super::text::{PreparedText, cell_face, measure_text};
use crate::shared::{PageScene, Skipped, strict_color, with_clip_layers, with_dashes};

/// `ratio`: device pixels per px, which bordered lines snap to as Desktop's canvas snaps them.
pub fn translate_sheet(display_list: &DisplayList, ratio: f32) -> Result<PageScene> {
    let width = frame_dimension(display_list.width, "width")?;
    let height = frame_dimension(display_list.height, "height")?;
    let mut scene = Scene::new();
    scene.fill(Fill::NonZero, Affine::IDENTITY, Color::WHITE, None, &Rect::new(0.0, 0.0, width, height));
    let mut skipped = Skipped::default();
    let mut tiles = PatternTiles::default();
    for command in &display_list.commands {
        if let Err(reason) = translate_command(&mut scene, command, &mut tiles, ratio) {
            skipped.record(command_kind(command), reason);
        }
    }
    Ok(PageScene { scene, width, height, skipped })
}

/// One 8x8 tile per pattern and color pair, shared by every cell that uses it.
#[derive(Default)]
struct PatternTiles(HashMap<(PatternType, Arc<str>, Arc<str>), ImageData>);

impl PatternTiles {
    fn tile(
        &mut self,
        pattern: PatternType,
        fg: &Arc<str>,
        bg: &Arc<str>,
    ) -> Result<ImageData, String> {
        let key = (pattern, fg.clone(), bg.clone());
        if let Some(tile) = self.0.get(&key) {
            return Ok(tile.clone());
        }
        let fg = strict_color(fg)?.to_rgba8();
        let bg = strict_color(bg)?.to_rgba8();
        let mut pixels = Vec::with_capacity(8 * 8 * 4);
        for row in pattern.mask() {
            for x in 0..8 {
                let color = if row & (1 << x) != 0 { fg } else { bg };
                pixels.extend([color.r, color.g, color.b, color.a]);
            }
        }
        let tile = ImageData {
            data: Blob::from(pixels),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: 8,
            height: 8,
        };
        self.0.insert(key, tile.clone());
        Ok(tile)
    }
}

fn translate_command(
    scene: &mut Scene,
    command: &DrawCmd,
    tiles: &mut PatternTiles,
    ratio: f32,
) -> Result<(), String> {
    match command {
        DrawCmd::PatternFill {
            x,
            y,
            w,
            h,
            pattern,
            fg,
            bg,
            origin_x,
            origin_y,
            clip,
        } => {
            let rect = rectangle(*x, *y, *w, *h, "pattern fill")?;
            let origin = Affine::translate((
                f64::from(finite(*origin_x, "pattern origin x")?),
                f64::from(finite(*origin_y, "pattern origin y")?),
            ));
            let brush = ImageBrush::new(tiles.tile(*pattern, fg, bg)?)
                .with_extend(Extend::Repeat)
                .with_quality(ImageQuality::Low);
            paint_clipped(scene, clip.as_ref(), |scene| {
                scene.fill(Fill::NonZero, Affine::IDENTITY, &brush, Some(origin), &rect);
            })
        }
        DrawCmd::FillRect {
            x,
            y,
            w,
            h,
            color,
            clip,
        } => {
            let rect = rectangle(*x, *y, *w, *h, "fill rectangle")?;
            let color = strict_color(color)?;
            paint_clipped(scene, clip.as_ref(), |scene| {
                scene.fill(Fill::NonZero, Affine::IDENTITY, color, None, &rect);
            })
        }
        DrawCmd::Line {
            x1,
            y1,
            x2,
            y2,
            width,
            color,
            style,
            rail,
            clip,
        } => {
            let line =
                PreparedLine::new(*x1, *y1, *x2, *y2, *width, color, style.as_deref(), *rail)?.snapped(ratio);
            paint_clipped(scene, clip.as_ref(), |scene| line.paint(scene))
        }
        DrawCmd::Path {
            commands,
            fill,
            stroke,
            clip,
        } => {
            let path = build_path(commands)?;
            let fill = strict_color(fill)?;
            let stroke = stroke
                .as_ref()
                .map(|stroke| -> Result<_, String> {
                    Ok((
                        Stroke::new(nonnegative(stroke.width, "path stroke width")?),
                        strict_color(&stroke.color)?,
                    ))
                })
                .transpose()?;
            paint_clipped(scene, clip.as_ref(), |scene| {
                scene.fill(Fill::NonZero, Affine::IDENTITY, fill, None, &path);
                if let Some((stroke, color)) = &stroke {
                    scene.stroke(stroke, Affine::IDENTITY, *color, None, &path);
                }
            })
        }
        DrawCmd::Text {
            x,
            y,
            text,
            font_size,
            color,
            clip,
            align,
            bold,
            italic,
            underline,
            strike,
            highlight,
            dashed_underline,
            font_family,
            number_format,
            ghost: _,
            chart: _,
            rotation_deg,
            rotation_box: _,
            rotation_valign: _,
            stacked,
            wrap,
        } => {
            if *stacked {
                return Err("stacked text is not translated".to_owned());
            }
            if let Some(degrees) = rotation_deg {
                return Err(format!("text rotated {degrees} degrees is not translated"));
            }
            let face = cell_face(font_family.as_deref(), *bold, *italic).ok_or("no face draws the cell's family")?;
            let prepare = |x: f32, y: f32, text: &str, align: Align| {
                PreparedText::new(
                    face,
                    x,
                    y,
                    text,
                    *font_size,
                    color,
                    align,
                    *bold,
                    *italic,
                    *underline,
                    *strike,
                    highlight.as_deref(),
                    *dashed_underline,
                )
            };
            let runs = if let Some(layout) = number_format {
                let mut runs = Vec::new();
                xlsx_render::paint_number_format(
                    text,
                    layout,
                    *align,
                    |literal| measure_text(face, literal, *font_size),
                    |literal, x| runs.push(prepare(x, *y, literal, Align::Left)),
                );
                runs.into_iter().collect::<Result<Vec<_>, _>>()?
            } else if let Some(wrap) = wrap {
                let lines = xlsx_render::wrap::wrap_lines(text, wrap.width, |line| {
                    measure_text(face, line, *font_size)
                });
                lines
                    .iter()
                    .enumerate()
                    .map(|(index, line)| {
                        let y = xlsx_render::wrap::wrapped_line_y(*y, index, lines.len(), wrap);
                        prepare(*x, y, line, *align)
                    })
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                vec![prepare(*x, *y, text, *align)?]
            };
            paint_clipped(scene, Some(clip), |scene| {
                for run in &runs {
                    run.paint(scene);
                }
            })
        }
    }
}

fn paint_clipped(
    scene: &mut Scene,
    clip: Option<&DisplayRect>,
    paint: impl FnOnce(&mut Scene),
) -> Result<(), String> {
    let Some(clip) = clip else {
        paint(scene);
        return Ok(());
    };
    let Some(rect) = clip_rectangle(*clip)? else {
        return Ok(());
    };
    with_clip_layers(scene, &[(Affine::IDENTITY, rect)], |scene| {
        paint(scene);
        Ok(())
    })
}

struct PreparedLine {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    width: f32,
    /// The width it strokes at: `width`, or a bordered line's snapped to whole device pixels.
    stroke: f32,
    color: Color,
    style: Option<String>,
    rail: Option<f32>,
}

impl PreparedLine {
    #[allow(clippy::too_many_arguments)]
    fn new(
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        width: f32,
        color: &str,
        style: Option<&str>,
        rail: Option<f32>,
    ) -> Result<Self, String> {
        if !matches!(style, None | Some("solid" | "double"))
            && border_dash(style.unwrap_or_default()).is_empty()
        {
            return Err(format!(
                "line style {} is not translated",
                style.unwrap_or_default()
            ));
        }
        Ok(Self {
            x1: finite(x1, "line x1")?,
            y1: finite(y1, "line y1")?,
            x2: finite(x2, "line x2")?,
            y2: finite(y2, "line y2")?,
            width: nonnegative_f32(width, "line width")?,
            stroke: nonnegative_f32(width, "line width")?,
            color: strict_color(color)?,
            style: style.map(str::to_owned),
            rail: rail
                .map(|rail| nonnegative_f32(rail, "line rail"))
                .transpose()?,
        })
    }

    /// The line on the device pixel grid at `ratio`, as `canvas2d.ts` `paintLine` puts a bordered
    /// one: an odd device width centred on a pixel, its ends on pixel edges, and a dashed or solid
    /// border a whole number of device pixels wide. Gridlines and old-style doubles stay put.
    fn snapped(mut self, ratio: f32) -> Self {
        let Some(style) = self.style.as_deref() else { return self };
        let device = (self.width * ratio).round().max(1.0);
        if style == "double" && self.rail.is_none() {
            return self;
        }
        let centre = |v: f32| ((v * ratio).floor() + (device % 2.0) / 2.0) / ratio;
        let edge = |v: f32| (v * ratio).round() / ratio;
        if self.y1 == self.y2 {
            (self.y1, self.y2, self.x1, self.x2) = (centre(self.y1), centre(self.y1), edge(self.x1), edge(self.x2));
        } else if self.x1 == self.x2 {
            (self.x1, self.x2, self.y1, self.y2) = (centre(self.x1), centre(self.x1), edge(self.y1), edge(self.y2));
        }
        if style != "double" {
            self.stroke = device / ratio;
        }
        self
    }

    fn paint(&self, scene: &mut Scene) {
        match self.style.as_deref() {
            Some("double") => {
                let (offset, width) = match self.rail {
                    Some(rail) => ((self.width - rail) / 2.0, rail),
                    None => ((self.width * 0.8).max(0.8), (self.width * 0.6).max(0.5)),
                };
                let horizontal = (self.y1 - self.y2).abs() <= (self.x1 - self.x2).abs();
                let (dx, dy) = if horizontal {
                    (0.0, offset)
                } else {
                    (offset, 0.0)
                };
                stroke_segment(
                    scene,
                    self.x1 - dx,
                    self.y1 - dy,
                    self.x2 - dx,
                    self.y2 - dy,
                    Stroke::new(f64::from(width)),
                    self.color,
                );
                stroke_segment(
                    scene,
                    self.x1 + dx,
                    self.y1 + dy,
                    self.x2 + dx,
                    self.y2 + dy,
                    Stroke::new(f64::from(width)),
                    self.color,
                );
            }
            Some(style) if !border_dash(style).is_empty() => {
                let scale = if style.starts_with("medium") {
                    self.width / 2.0
                } else {
                    self.width
                };
                stroke_segment(
                    scene,
                    self.x1,
                    self.y1,
                    self.x2,
                    self.y2,
                    with_dashes(
                        Stroke::new(f64::from(self.stroke)),
                        border_dash(style).iter().map(|run| f64::from(run * scale)),
                    ),
                    self.color,
                );
            }
            None | Some("solid") => stroke_segment(
                scene,
                self.x1,
                self.y1,
                self.x2,
                self.y2,
                Stroke::new(f64::from(self.stroke)),
                self.color,
            ),
            Some(_) => unreachable!(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn stroke_segment(
    scene: &mut Scene,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    stroke: Stroke,
    color: Color,
) {
    // Canvas lines end square at their ends (`lineCap = "butt"`), not round past them.
    scene.stroke(
        &stroke.with_caps(Cap::Butt),
        Affine::IDENTITY,
        color,
        None,
        &Line::new(
            (f64::from(x1), f64::from(y1)),
            (f64::from(x2), f64::from(y2)),
        ),
    );
}

fn build_path(commands: &[GeometryPathCommand]) -> Result<BezPath, String> {
    if commands.is_empty() {
        return Err("path has no drawable commands".to_owned());
    }
    let mut path = BezPath::new();
    for command in commands {
        match command {
            GeometryPathCommand::Move { x, y } => {
                path.move_to((path_coord(*x)?, path_coord(*y)?));
            }
            GeometryPathCommand::Line { x, y } => {
                path.line_to((path_coord(*x)?, path_coord(*y)?));
            }
            GeometryPathCommand::Quad { cpx, cpy, x, y } => {
                path.quad_to(
                    (path_coord(*cpx)?, path_coord(*cpy)?),
                    (path_coord(*x)?, path_coord(*y)?),
                );
            }
            GeometryPathCommand::Cubic {
                cp1x,
                cp1y,
                cp2x,
                cp2y,
                x,
                y,
            } => {
                path.curve_to(
                    (path_coord(*cp1x)?, path_coord(*cp1y)?),
                    (path_coord(*cp2x)?, path_coord(*cp2y)?),
                    (path_coord(*x)?, path_coord(*y)?),
                );
            }
            GeometryPathCommand::Close => path.close_path(),
        }
    }
    Ok(path)
}

fn path_coord(value: f64) -> Result<f64, String> {
    let value = value as f32;
    finite(value, "path coordinate").map(f64::from)
}

fn rectangle(x: f32, y: f32, w: f32, h: f32, name: &str) -> Result<Rect, String> {
    let x = finite(x, name)?;
    let y = finite(y, name)?;
    let w = positive_f32(w, name)?;
    let h = positive_f32(h, name)?;
    let right = finite(x + w, name)?;
    let bottom = finite(y + h, name)?;
    Ok(Rect::new(
        f64::from(x),
        f64::from(y),
        f64::from(right),
        f64::from(bottom),
    ))
}

fn clip_rectangle(rect: DisplayRect) -> Result<Option<Rect>, String> {
    let x = finite(rect.x, "clip x")?;
    let y = finite(rect.y, "clip y")?;
    let w = finite(rect.w, "clip width")?;
    let h = finite(rect.h, "clip height")?;
    if w <= 0.0 || h <= 0.0 {
        return Ok(None);
    }
    let right = finite(x + w, "clip right")?;
    let bottom = finite(y + h, "clip bottom")?;
    Ok(Some(Rect::new(
        f64::from(x),
        f64::from(y),
        f64::from(right),
        f64::from(bottom),
    )))
}

fn frame_dimension(value: f32, name: &str) -> Result<f64> {
    if !value.is_finite() || value <= 0.0 {
        anyhow::bail!("XLSX display-list {name} is invalid");
    }
    Ok(f64::from(value))
}

pub(super) fn finite(value: f32, name: &str) -> Result<f32, String> {
    value
        .is_finite()
        .then_some(value)
        .ok_or_else(|| format!("{name} is not finite"))
}

pub(super) fn positive_f32(value: f32, name: &str) -> Result<f32, String> {
    let value = finite(value, name)?;
    (value > 0.0)
        .then_some(value)
        .ok_or_else(|| format!("{name} is not positive"))
}

fn nonnegative_f32(value: f32, name: &str) -> Result<f32, String> {
    let value = finite(value, name)?;
    (value >= 0.0)
        .then_some(value)
        .ok_or_else(|| format!("{name} is negative"))
}

fn nonnegative(value: f32, name: &str) -> Result<f64, String> {
    nonnegative_f32(value, name).map(f64::from)
}

fn command_kind(command: &DrawCmd) -> &'static str {
    match command {
        DrawCmd::FillRect { .. } => "fillRect",
        DrawCmd::PatternFill { .. } => "patternFill",
        DrawCmd::Line { .. } => "line",
        DrawCmd::Path { .. } => "path",
        DrawCmd::Text { .. } => "text",
    }
}


//! What every format's translation shares: a page as a Vello scene, what it left out, and the
//! colour syntax display lists use. After BetterOffice's native viewer (`scene_shared.rs`,
//! Apache-2.0).

use std::collections::BTreeMap;

use vello::Scene;
use vello::kurbo::{Affine, Rect, Stroke};
use vello::peniko::{Color, Fill};

/// Display-list items a translation could not draw, by kind and by reason.
#[derive(Clone, Debug, Default)]
pub struct Skipped {
    pub counts: BTreeMap<String, usize>,
    pub reasons: BTreeMap<String, usize>,
}

impl Skipped {
    pub fn total(&self) -> usize {
        self.counts.values().sum()
    }

    pub fn record(&mut self, kind: &str, reason: String) {
        *self.counts.entry(kind.to_owned()).or_default() += 1;
        *self.reasons.entry(reason).or_default() += 1;
    }
}

/// One page, slide or sheet frame: its scene in CSS px from its top-left corner, paper included.
pub struct PageScene {
    pub scene: Scene,
    pub width: f64,
    pub height: f64,
    pub skipped: Skipped,
}

pub fn with_clip_layers<T>(scene: &mut Scene, layers: &[(Affine, Rect)], draw: impl FnOnce(&mut Scene) -> Result<T, String>) -> Result<T, String> {
    for (transform, rect) in layers {
        scene.push_clip_layer(Fill::NonZero, *transform, rect);
    }
    let result = draw(scene);
    for _ in layers {
        scene.pop_layer();
    }
    result
}

pub fn with_dashes<I>(stroke: Stroke, dashes: I) -> Stroke
where
    I: IntoIterator<Item = f64>,
{
    stroke.with_dashes(0.0, dashes)
}

/// A display list's colour (`#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()`, `transparent`)
/// at `opacity`.
pub fn color(value: &str, opacity: f32) -> Result<Color, String> {
    let (red, green, blue, alpha) = parse_color(value)?;
    let alpha = (f32::from(alpha) * opacity.clamp(0.0, 1.0)).round() as u8;
    Ok(Color::from_rgba8(red, green, blue, alpha))
}

/// A spreadsheet colour: `#rrggbb` or `#rrggbbaa` only.
pub fn strict_color(value: &str) -> Result<Color, String> {
    let hex = value.strip_prefix('#').ok_or_else(|| format!("invalid XLSX color {value}"))?;
    if hex.len() != 6 && hex.len() != 8 {
        return Err(format!("invalid XLSX color {value}"));
    }
    let (red, green, blue, alpha) = parse_hex(value, hex)?;
    Ok(Color::from_rgba8(red, green, blue, alpha))
}

fn parse_color(value: &str) -> Result<(u8, u8, u8, u8), String> {
    if value.eq_ignore_ascii_case("transparent") {
        return Ok((0, 0, 0, 0));
    }
    if let Some(hex) = value.strip_prefix('#') {
        let expanded: String;
        let hex = match hex.len() {
            3 | 4 => {
                expanded = hex.chars().flat_map(|c| [c, c]).collect();
                expanded.as_str()
            }
            6 | 8 => hex,
            _ => return Err(format!("invalid color {value}")),
        };
        return parse_hex(value, hex);
    }
    for (prefix, alpha) in [("rgba(", true), ("rgb(", false)] {
        let Some(body) = value.strip_prefix(prefix).and_then(|b| b.strip_suffix(')')) else { continue };
        let parts: Vec<&str> = body.split(',').map(str::trim).collect();
        if parts.len() != if alpha { 4 } else { 3 } {
            break;
        }
        let channel = |v: &str| -> Option<u8> {
            match v.strip_suffix('%') {
                Some(percent) => Some((percent.parse::<f32>().ok()?.clamp(0.0, 100.0) * 2.55).round() as u8),
                None => Some(v.parse::<f32>().ok()?.clamp(0.0, 255.0).round() as u8),
            }
        };
        let bad = || format!("invalid color {value}");
        let a = if !alpha {
            255
        } else if let Some(percent) = parts[3].strip_suffix('%') {
            (percent.parse::<f32>().map_err(|_| bad())?.clamp(0.0, 100.0) * 2.55).round() as u8
        } else {
            (parts[3].parse::<f32>().map_err(|_| bad())?.clamp(0.0, 1.0) * 255.0).round() as u8
        };
        return Ok((channel(parts[0]).ok_or_else(bad)?, channel(parts[1]).ok_or_else(bad)?, channel(parts[2]).ok_or_else(bad)?, a));
    }
    Err(format!("invalid color {value}"))
}

fn parse_hex(value: &str, hex: &str) -> Result<(u8, u8, u8, u8), String> {
    let byte = |i: usize| {
        hex.as_bytes().get(i..i + 2).and_then(|p| std::str::from_utf8(p).ok()).and_then(|p| u8::from_str_radix(p, 16).ok()).ok_or_else(|| format!("invalid color {value}"))
    };
    Ok((byte(0)?, byte(2)?, byte(4)?, if hex.len() == 8 { byte(6)? } else { 255 }))
}

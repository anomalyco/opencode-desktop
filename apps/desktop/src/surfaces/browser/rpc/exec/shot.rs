//! A screenshot's clip and size: the engine scales the capture (`Page.captureScreenshot` `clip.scale`),
//! so the app decodes and re-encodes only an image the engine could not size.

use std::io::Cursor;

use opencode_kit::browser_rpc::{Outcome, failure_code};
use serde_json::{Value, json};

use super::capture::{ScreenshotStage, ScreenshotState};
use super::send_rpc_cdp;
use crate::App;

pub(super) const MAX_FULLPAGE_EDGE: f64 = 16_384.0;
pub(super) const MAX_FULLPAGE_PIXELS: f64 = 16_000_000.0;

/// An element box: drawn (finite, positive; an ancestor's `scale(0)` gives none) and within the
/// full-page pixel bounds, scaled down to fit.
pub(super) fn capture_clip(app: &mut App, st: &mut ScreenshotState, (x, y, w, h): (f64, f64, f64, f64)) -> Option<Outcome> {
    if ![x, y, w, h].iter().all(|v| v.is_finite()) || w <= 0.0 || h <= 0.0 {
        return Some(Outcome::fail(failure_code::OPERATION_FAILED, "Element has zero visible size."));
    }
    let scale = (MAX_FULLPAGE_EDGE / w).min(MAX_FULLPAGE_EDGE / h).min((MAX_FULLPAGE_PIXELS / (w * h)).sqrt()).min(1.0);
    send_clip_capture(app, st, (x, y, w, h), scale)
}

/// The visible viewport from `Page.getLayoutMetrics`, scaled so the image is at most `max_w` pixels
/// wide (official resizes its frame to `maxWidth` too): the engine encodes the small image, and the
/// app need not decode, resize and re-encode the full one.
pub(super) fn capture_viewport(app: &mut App, st: &mut ScreenshotState, metrics: &Value, max_w: u32) -> Option<Outcome> {
    let css = &metrics["cssVisualViewport"];
    let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64);
    let (Some(x), Some(y), Some(w), Some(h)) = (num(css, "pageX"), num(css, "pageY"), num(css, "clientWidth"), num(css, "clientHeight")) else {
        return Some(Outcome::fail(failure_code::OPERATION_FAILED, "Could not read the page's viewport."));
    };
    if w <= 0.0 || h <= 0.0 {
        return Some(Outcome::fail(failure_code::OPERATION_FAILED, "The page has no visible area."));
    }
    // Device pixels per CSS pixel: the device-pixel viewport over the CSS one.
    let ratio = num(&metrics["visualViewport"], "clientWidth").filter(|d| *d > 0.0).map_or(1.0, |d| d / w);
    let scale = (f64::from(max_w) / (w * ratio)).min(1.0);
    send_clip_capture(app, st, (x, y, w, h), scale)
}

fn send_clip_capture(app: &mut App, st: &mut ScreenshotState, (x, y, w, h): (f64, f64, f64, f64), scale: f64) -> Option<Outcome> {
    let mut params = json!({ "format": st.format, "clip": { "x": x, "y": y, "width": w, "height": h, "scale": scale } });
    if let Some(q) = st.quality
        && st.format != "png"
    {
        params["quality"] = json!(q);
    }
    match send_rpc_cdp(app, st.host_id, "Page.captureScreenshot", params, None) {
        Ok(req) => {
            st.stage = ScreenshotStage::Capture { cdp_req: req };
            None
        }
        Err(e) => Some(Outcome::fail(failure_code::OPERATION_FAILED, e)),
    }
}

/// The image at most `max_w` wide: unchanged (and not decoded) when its header says it already fits.
pub(super) fn maybe_downscale(bytes: &[u8], format: &str, max_w: u32) -> Vec<u8> {
    if !matches!(format, "png" | "jpeg") {
        return bytes.to_vec();
    }
    let fits = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_dimensions().ok())
        .is_some_and(|(w, _)| w <= max_w);
    if fits {
        return bytes.to_vec();
    }
    let Ok(img) = image::load_from_memory(bytes) else {
        return bytes.to_vec();
    };
    if img.width() <= max_w {
        return bytes.to_vec();
    }
    let new_h = ((img.height() as u64 * max_w as u64) / img.width().max(1) as u64).max(1) as u32;
    let resized = img.resize(max_w, new_h, image::imageops::FilterType::Triangle);
    let mut out = Cursor::new(Vec::new());
    let fmt = if format == "jpeg" {
        image::ImageFormat::Jpeg
    } else {
        image::ImageFormat::Png
    };
    if resized.write_to(&mut out, fmt).is_ok() {
        out.into_inner()
    } else {
        bytes.to_vec()
    }
}

pub(super) fn box_model_rect(v: &Value) -> Option<(f64, f64, f64, f64)> {
    let border = v.get("model")?.get("border")?.as_array()?;
    if border.len() < 8 {
        return None;
    }
    let xs = [0, 2, 4, 6].map(|i| border[i].as_f64().unwrap_or(0.0));
    let ys = [1, 3, 5, 7].map(|i| border[i].as_f64().unwrap_or(0.0));
    let min_x = xs.into_iter().fold(f64::INFINITY, f64::min);
    let max_x = xs.into_iter().fold(f64::NEG_INFINITY, f64::max);
    let min_y = ys.into_iter().fold(f64::INFINITY, f64::min);
    let max_y = ys.into_iter().fold(f64::NEG_INFINITY, f64::max);
    let w = max_x - min_x;
    let h = max_y - min_y;
    (w > 0.0 && h > 0.0).then_some((min_x, min_y, w, h))
}

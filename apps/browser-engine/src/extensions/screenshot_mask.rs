//! Pure-Rust, zero-unsafe screenshot masking for `Page.captureScreenshot` on web tabs with
//! extension sub-frames (`review-rd274.md` Rounds 4 & 6).
//!
//! Decodes Chromium's lossless PNG output (8-bit RGB or RGBA, all 5 RFC 2083 scanline filters,
//! verified chunk CRCs, strict dimension/pixel caps), paints opaque `#18181B` over every
//! protected extension sub-frame rectangle (with conservative CSS padding across HiDPI scale,
//! browser zoom, viewport scroll, element `clip`, and `captureBeyondViewport` including
//! `position: fixed` overlays), and re-encodes in the caller's requested format (`png` or `jpeg`).

#![forbid(unsafe_code)]

use std::io::{Read, Write};

use base64::Engine as _;
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use serde_json::Value;

use super::frame_guard::{CssRect, ViewportMetrics};

const MAX_MASK_DIM: u32 = 16_384;
const MAX_MASK_PIXELS: usize = 16_777_216;
const MAX_COMPRESSED_IDAT: usize = 32 * 1024 * 1024;
const MASK_PAD_CSS: f64 = 4.0;
const MASK_RGBA: [u8; 4] = [24, 24, 27, 255];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenshotFormat {
    Png,
    Jpeg { quality: u8 },
}

#[derive(Clone, Copy, Debug)]
pub struct ClipSpec {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug)]
pub struct ParsedScreenshotParams {
    pub format: ScreenshotFormat,
    pub clip: Option<ClipSpec>,
    pub internal_png_params: Value,
}

/// Validates external `Page.captureScreenshot` params and builds the internal lossless PNG params.
pub fn parse_screenshot_params(params: &Value) -> Result<ParsedScreenshotParams, &'static str> {
    let format = match params.get("format").and_then(Value::as_str) {
        None | Some("png") => ScreenshotFormat::Png,
        Some("jpeg") => {
            let q = params
                .get("quality")
                .and_then(Value::as_u64)
                .unwrap_or(80)
                .clamp(1, 100) as u8;
            ScreenshotFormat::Jpeg { quality: q }
        }
        Some(_) => {
            return Err("Screenshots on tabs with extension frames only support 'png' or 'jpeg' format.");
        }
    };
    let capture_beyond_viewport = params
        .get("captureBeyondViewport")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if capture_beyond_viewport {
        return Err(
            "Full-page captureBeyondViewport screenshots are not permitted on tabs with extension frames.",
        );
    }
    let clip = if let Some(c) = params.get("clip") {
        if c.is_null() {
            None
        } else {
            let x = c.get("x").and_then(Value::as_f64).ok_or("Invalid screenshot clip.x.")?;
            let y = c.get("y").and_then(Value::as_f64).ok_or("Invalid screenshot clip.y.")?;
            let width = c.get("width").and_then(Value::as_f64).ok_or("Invalid screenshot clip.width.")?;
            let height = c.get("height").and_then(Value::as_f64).ok_or("Invalid screenshot clip.height.")?;
            let scale = c.get("scale").and_then(Value::as_f64).unwrap_or(1.0);
            if !x.is_finite()
                || !y.is_finite()
                || !width.is_finite()
                || !height.is_finite()
                || !scale.is_finite()
                || width <= 0.0
                || height <= 0.0
                || scale <= 0.0
                || width > f64::from(MAX_MASK_DIM)
                || height > f64::from(MAX_MASK_DIM)
                || scale > 8.0
            {
                return Err("Screenshot clip dimensions are out of bounds.");
            }
            Some(ClipSpec { x, y, width, height })
        }
    } else {
        None
    };

    // Always capture the live viewport as lossless PNG (never passing `captureBeyondViewport` or
    // `clip` to CEF OSR, avoiding CEF 156 OSR's wrapped-framebuffer quirk), paint out every
    // extension rect on the full viewport first, then apply any validated in-viewport clip in Rust.
    let mut internal = serde_json::Map::new();
    internal.insert("format".into(), Value::String("png".into()));
    if let Some(v) = params.get("fromSurface").and_then(Value::as_bool) {
        internal.insert("fromSurface".into(), Value::Bool(v));
    }
    Ok(ParsedScreenshotParams {
        format,
        clip,
        internal_png_params: Value::Object(internal),
    })
}

/// Validates that any requested `clip` lies within the currently visible viewport.
pub fn validate_clip_in_viewport(
    clip: Option<ClipSpec>,
    metrics: ViewportMetrics,
) -> Result<(), &'static str> {
    let Some(c) = clip else {
        return Ok(());
    };
    const EPS: f64 = 1.5;
    if c.x < metrics.page_x - EPS
        || c.y < metrics.page_y - EPS
        || c.x + c.width > metrics.page_x + metrics.client_width + EPS
        || c.y + c.height > metrics.page_y + metrics.client_height + EPS
    {
        return Err(
            "Screenshot clip extends outside the visible viewport on a tab with extension frames.",
        );
    }
    Ok(())
}

/// Decodes a base64 live-viewport PNG screenshot, paints opaque `#18181B` over all `rects`,
/// applies any validated in-viewport `clip`, and re-encodes in `png` or `jpeg`.
pub fn mask_screenshot_b64(
    png_b64: &str,
    parsed: &ParsedScreenshotParams,
    metrics: ViewportMetrics,
    rects: &[CssRect],
) -> Result<String, &'static str> {
    validate_clip_in_viewport(parsed.clip, metrics)?;
    let raw_png = base64::engine::general_purpose::STANDARD
        .decode(png_b64)
        .map_err(|_| "Could not decode screenshot base64 payload.")?;
    let (w, h, mut rgba) = decode_png_rgba(&raw_png)?;
    paint_masked_rects(w, h, &mut rgba, metrics, rects);
    let (out_w, out_h, out_rgba) = if let Some(c) = parsed.clip {
        crop_viewport_rgba(w, h, &rgba, metrics, c)?
    } else {
        (w, h, rgba)
    };
    let encoded = match parsed.format {
        ScreenshotFormat::Png => encode_png_rgba(out_w, out_h, &out_rgba)?,
        ScreenshotFormat::Jpeg { quality } => encode_jpeg_rgba(out_w, out_h, &out_rgba, quality),
    };
    Ok(base64::engine::general_purpose::STANDARD.encode(encoded))
}

fn paint_masked_rects(
    img_w: u32,
    img_h: u32,
    rgba: &mut [u8],
    metrics: ViewportMetrics,
    rects: &[CssRect],
) {
    // Use exact `visualViewport / cssVisualViewport` ratio so scrollbars never skew the scale.
    let sx = metrics.scale_x;
    let sy = metrics.scale_y;
    for &r in rects {
        fill_css_rect(
            img_w,
            img_h,
            rgba,
            (r.x0 - MASK_PAD_CSS) * sx,
            (r.y0 - MASK_PAD_CSS) * sy,
            (r.x1 + MASK_PAD_CSS) * sx,
            (r.y1 + MASK_PAD_CSS) * sy,
        );
    }
}

fn crop_viewport_rgba(
    img_w: u32,
    img_h: u32,
    rgba: &[u8],
    metrics: ViewportMetrics,
    clip: ClipSpec,
) -> Result<(u32, u32, Vec<u8>), &'static str> {
    let vx0 = clip.x - metrics.page_x;
    let vy0 = clip.y - metrics.page_y;
    let px0 = ((vx0 * metrics.scale_x).floor().max(0.0) as u32).min(img_w);
    let py0 = ((vy0 * metrics.scale_y).floor().max(0.0) as u32).min(img_h);
    let px1 = (((vx0 + clip.width) * metrics.scale_x).ceil().max(0.0) as u32).min(img_w);
    let py1 = (((vy0 + clip.height) * metrics.scale_y).ceil().max(0.0) as u32).min(img_h);
    let cw = px1.saturating_sub(px0);
    let ch = py1.saturating_sub(py0);
    if cw == 0 || ch == 0 {
        return Err("Screenshot clip produced an empty region.");
    }
    let src_stride = (img_w as usize) * 4;
    let dst_stride = (cw as usize) * 4;
    let mut out = vec![0u8; (ch as usize) * dst_stride];
    for y in 0..ch as usize {
        let src_off = (py0 as usize + y) * src_stride + (px0 as usize) * 4;
        let dst_off = y * dst_stride;
        out[dst_off..dst_off + dst_stride].copy_from_slice(&rgba[src_off..src_off + dst_stride]);
    }
    Ok((cw, ch, out))
}

fn fill_css_rect(img_w: u32, img_h: u32, rgba: &mut [u8], x0: f64, y0: f64, x1: f64, y1: f64) {
    if !x0.is_finite() || !y0.is_finite() || !x1.is_finite() || !y1.is_finite() {
        return;
    }
    let px0 = (x0.floor().max(0.0) as u32).min(img_w);
    let py0 = (y0.floor().max(0.0) as u32).min(img_h);
    let px1 = (x1.ceil().max(0.0) as u32).min(img_w);
    let py1 = (y1.ceil().max(0.0) as u32).min(img_h);
    if px0 >= px1 || py0 >= py1 {
        return;
    }
    let stride = (img_w as usize) * 4;
    for y in py0 as usize..py1 as usize {
        let row = &mut rgba[y * stride..(y + 1) * stride];
        for x in px0 as usize..px1 as usize {
            let off = x * 4;
            row[off..off + 4].copy_from_slice(&MASK_RGBA);
        }
    }
}

// ============================================================================
// Bounded RFC 2083 PNG Decoder & Encoder (8-bit RGB/RGBA, non-interlaced)
// ============================================================================

fn decode_png_rgba(data: &[u8]) -> Result<(u32, u32, Vec<u8>), &'static str> {
    if data.len() < 33 || &data[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("Screenshot payload is not a valid PNG.");
    }
    let mut pos = 8usize;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut bpp = 0usize;
    let mut seen_ihdr = false;
    let mut seen_iend = false;
    let mut idat = Vec::new();

    while pos + 12 <= data.len() {
        let len = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        let ctype = &data[pos + 4..pos + 8];
        let body_start = pos + 8;
        let body_end = body_start.checked_add(len).ok_or("Corrupt PNG chunk length.")?;
        let crc_end = body_end.checked_add(4).ok_or("Corrupt PNG chunk bounds.")?;
        if crc_end > data.len() {
            return Err("Truncated PNG chunk.");
        }
        let body = &data[body_start..body_end];
        let want_crc = u32::from_be_bytes(data[body_end..crc_end].try_into().unwrap());
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(ctype);
        hasher.update(body);
        if hasher.finalize() != want_crc {
            return Err("PNG chunk CRC mismatch.");
        }
        pos = crc_end;

        match ctype {
            b"IHDR" => {
                if seen_ihdr || body.len() != 13 {
                    return Err("Invalid PNG IHDR chunk.");
                }
                width = u32::from_be_bytes(body[0..4].try_into().unwrap());
                height = u32::from_be_bytes(body[4..8].try_into().unwrap());
                let bit_depth = body[8];
                let color_type = body[9];
                let comp = body[10];
                let filt = body[11];
                let interlace = body[12];
                if width == 0
                    || height == 0
                    || width > MAX_MASK_DIM
                    || height > MAX_MASK_DIM
                    || (width as usize).checked_mul(height as usize).is_none_or(|p| p > MAX_MASK_PIXELS)
                    || bit_depth != 8
                    || comp != 0
                    || filt != 0
                    || interlace != 0
                {
                    return Err("Unsupported or oversized PNG screenshot format.");
                }
                bpp = match color_type {
                    2 => 3,
                    6 => 4,
                    _ => return Err("Unsupported PNG color type for screenshot masking."),
                };
                seen_ihdr = true;
            }
            b"IDAT" => {
                if !seen_ihdr {
                    return Err("PNG IDAT before IHDR.");
                }
                if idat.len().saturating_add(body.len()) > MAX_COMPRESSED_IDAT {
                    return Err("PNG IDAT payload exceeded safety cap.");
                }
                idat.extend_from_slice(body);
            }
            b"IEND" => {
                seen_iend = true;
                break;
            }
            _ => {}
        }
    }
    if !seen_ihdr || !seen_iend || idat.is_empty() {
        return Err("Incomplete PNG screenshot stream.");
    }

    let row_bytes = (width as usize) * bpp;
    let expected_raw = (height as usize) * (1 + row_bytes);
    let mut raw = Vec::with_capacity(expected_raw);
    let mut dec = ZlibDecoder::new(idat.as_slice()).take((expected_raw + 1) as u64);
    dec.read_to_end(&mut raw)
        .map_err(|_| "Failed to decompress PNG screenshot stream.")?;
    if raw.len() != expected_raw {
        return Err("Decompressed PNG screenshot size mismatch.");
    }

    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    let mut prev = vec![0u8; row_bytes];
    let mut curr = vec![0u8; row_bytes];
    for y in 0..height as usize {
        let src_off = y * (1 + row_bytes);
        let filter = raw[src_off];
        let src = &raw[src_off + 1..src_off + 1 + row_bytes];
        match filter {
            0 => curr.copy_from_slice(src),
            1 => {
                for i in 0..row_bytes {
                    let a = if i >= bpp { curr[i - bpp] } else { 0 };
                    curr[i] = src[i].wrapping_add(a);
                }
            }
            2 => {
                for i in 0..row_bytes {
                    curr[i] = src[i].wrapping_add(prev[i]);
                }
            }
            3 => {
                for i in 0..row_bytes {
                    let a = if i >= bpp { u16::from(curr[i - bpp]) } else { 0 };
                    let b = u16::from(prev[i]);
                    curr[i] = src[i].wrapping_add(((a + b) >> 1) as u8);
                }
            }
            4 => {
                for i in 0..row_bytes {
                    let a = if i >= bpp { curr[i - bpp] } else { 0 };
                    let b = prev[i];
                    let c = if i >= bpp { prev[i - bpp] } else { 0 };
                    curr[i] = src[i].wrapping_add(paeth(a, b, c));
                }
            }
            _ => return Err("Invalid PNG scanline filter type."),
        }
        let dst = &mut rgba[y * (width as usize) * 4..(y + 1) * (width as usize) * 4];
        if bpp == 4 {
            dst.copy_from_slice(&curr);
        } else {
            for x in 0..width as usize {
                dst[x * 4] = curr[x * 3];
                dst[x * 4 + 1] = curr[x * 3 + 1];
                dst[x * 4 + 2] = curr[x * 3 + 2];
                dst[x * 4 + 3] = 255;
            }
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    Ok((width, height, rgba))
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let ai = i32::from(a);
    let bi = i32::from(b);
    let ci = i32::from(c);
    let p = ai + bi - ci;
    let pa = (p - ai).abs();
    let pb = (p - bi).abs();
    let pc = (p - ci).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn encode_png_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, &'static str> {
    let stride = (width as usize) * 4;
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::fast());
    for y in 0..height as usize {
        let row = &rgba[y * stride..(y + 1) * stride];
        enc.write_all(&[0])
            .and_then(|_| enc.write_all(row))
            .map_err(|_| "Failed to compress masked PNG screenshot.")?;
    }
    let zlib = enc
        .finish()
        .map_err(|_| "Failed to finish masked PNG screenshot stream.")?;

    let mut out = Vec::with_capacity(zlib.len() + 64);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8] = 8;
    ihdr[9] = 6;
    write_png_chunk(&mut out, b"IHDR", &ihdr);
    write_png_chunk(&mut out, b"IDAT", &zlib);
    write_png_chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

fn write_png_chunk(out: &mut Vec<u8>, tag: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(body);
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(tag);
    hasher.update(body);
    out.extend_from_slice(&hasher.finalize().to_be_bytes());
}

// ============================================================================
// Pure-Rust Baseline Sequential RGB 4:4:4 JPEG Encoder (ITU-T T.81 Annex K)
// ============================================================================

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

const STD_Y_QUANT: [u8; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
    92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];

const STD_C_QUANT: [u8; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

const DC_Y_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_Y_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const DC_C_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const DC_C_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const AC_Y_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 125];
const AC_Y_VALS: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
    0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0,
    0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
    0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5,
    0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2,
    0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];
const AC_C_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 119];
const AC_C_VALS: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
    0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0,
    0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26,
    0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
    0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
    0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5,
    0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
    0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda,
    0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];

fn scale_quant(base: &[u8; 64], quality: u8) -> [u8; 64] {
    let q = u32::from(quality.clamp(1, 100));
    let scale = if q < 50 { 5000 / q } else { 200 - q * 2 };
    let mut out = [1u8; 64];
    for i in 0..64 {
        let v = ((u32::from(base[i]) * scale + 50) / 100).clamp(1, 255);
        out[i] = v as u8;
    }
    out
}

fn build_huff(bits: &[u8; 16], vals: &[u8]) -> [(u16, u8); 256] {
    let mut table = [(0u16, 0u8); 256];
    let mut code = 0u16;
    let mut k = 0usize;
    for len in 1..=16u8 {
        for _ in 0..bits[(len - 1) as usize] {
            if k < vals.len() {
                table[vals[k] as usize] = (code, len);
                k += 1;
            }
            code = code.wrapping_add(1);
        }
        code <<= 1;
    }
    table
}

struct BitWriter<'a> {
    out: &'a mut Vec<u8>,
    buf: u32,
    bits: u8,
}

impl<'a> BitWriter<'a> {
    fn write(&mut self, code: u16, len: u8) {
        self.buf = (self.buf << len) | u32::from(code);
        self.bits += len;
        while self.bits >= 8 {
            self.bits -= 8;
            let byte = ((self.buf >> self.bits) & 0xff) as u8;
            self.out.push(byte);
            if byte == 0xff {
                self.out.push(0x00);
            }
        }
    }

    fn flush(&mut self) {
        if self.bits > 0 {
            let pad = 8 - self.bits;
            self.write((1u16 << pad) - 1, pad);
        }
    }
}

fn encode_jpeg_rgba(width: u32, height: u32, rgba: &[u8], quality: u8) -> Vec<u8> {
    let qy = scale_quant(&STD_Y_QUANT, quality);
    let qc = scale_quant(&STD_C_QUANT, quality);
    let dc_y = build_huff(&DC_Y_BITS, &DC_Y_VALS);
    let dc_c = build_huff(&DC_C_BITS, &DC_C_VALS);
    let ac_y = build_huff(&AC_Y_BITS, &AC_Y_VALS);
    let ac_c = build_huff(&AC_C_BITS, &AC_C_VALS);

    // Precompute cos table once per image encode.
    let mut cos_tbl = [[0.0f32; 8]; 8];
    for u in 0..8 {
        let cu = if u == 0 { std::f32::consts::FRAC_1_SQRT_2 } else { 1.0 };
        for x in 0..8 {
            let angle = ((2 * x + 1) * u) as f32 * std::f32::consts::PI / 16.0;
            cos_tbl[u][x] = 0.5 * cu * angle.cos();
        }
    }

    let mut out = Vec::with_capacity((width as usize * height as usize) / 4 + 1024);
    out.extend_from_slice(&[0xff, 0xd8]); // SOI
    // APP0 JFIF
    out.extend_from_slice(&[
        0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00,
        0x01, 0x00, 0x00,
    ]);
    // DQT Y & C (in zigzag order)
    for (id, qt) in [(0u8, &qy), (1u8, &qc)] {
        out.extend_from_slice(&[0xff, 0xdb, 0x00, 67, id]);
        for &z in &ZIGZAG {
            out.push(qt[z]);
        }
    }
    // SOF0 (Baseline, 8-bit, 3 components 4:4:4)
    let w16 = (width as u16).to_be_bytes();
    let h16 = (height as u16).to_be_bytes();
    out.extend_from_slice(&[
        0xff, 0xc0, 0x00, 17, 8, h16[0], h16[1], w16[0], w16[1], 3, 1, 0x11, 0, 2, 0x11, 1, 3,
        0x11, 1,
    ]);
    // DHT
    for (hdr, bits, vals) in [
        (0x00u8, &DC_Y_BITS, DC_Y_VALS.as_slice()),
        (0x10u8, &AC_Y_BITS, AC_Y_VALS.as_slice()),
        (0x01u8, &DC_C_BITS, DC_C_VALS.as_slice()),
        (0x11u8, &AC_C_BITS, AC_C_VALS.as_slice()),
    ] {
        let len = (2 + 1 + 16 + vals.len()) as u16;
        out.extend_from_slice(&[0xff, 0xc4]);
        out.extend_from_slice(&len.to_be_bytes());
        out.push(hdr);
        out.extend_from_slice(bits);
        out.extend_from_slice(vals);
    }
    // SOS
    out.extend_from_slice(&[0xff, 0xda, 0x00, 12, 3, 1, 0x00, 2, 0x11, 3, 0x11, 0, 63, 0]);

    let mut bw = BitWriter { out: &mut out, buf: 0, bits: 0 };
    let mut prev_dc = [0i32; 3];
    let w = width as usize;
    let h = height as usize;

    for by in (0..h).step_by(8) {
        for bx in (0..w).step_by(8) {
            let mut y_blk = [0.0f32; 64];
            let mut cb_blk = [0.0f32; 64];
            let mut cr_blk = [0.0f32; 64];
            for py in 0..8 {
                let sy = (by + py).min(h - 1);
                for px in 0..8 {
                    let sx = (bx + px).min(w - 1);
                    let off = (sy * w + sx) * 4;
                    let r = f32::from(rgba[off]);
                    let g = f32::from(rgba[off + 1]);
                    let b = f32::from(rgba[off + 2]);
                    let i = py * 8 + px;
                    y_blk[i] = 0.299 * r + 0.587 * g + 0.114 * b - 128.0;
                    cb_blk[i] = -0.168_736 * r - 0.331_264 * g + 0.5 * b;
                    cr_blk[i] = 0.5 * r - 0.418_688 * g - 0.081_312 * b;
                }
            }
            encode_block(&y_blk, &cos_tbl, &qy, &dc_y, &ac_y, &mut prev_dc[0], &mut bw);
            encode_block(&cb_blk, &cos_tbl, &qc, &dc_c, &ac_c, &mut prev_dc[1], &mut bw);
            encode_block(&cr_blk, &cos_tbl, &qc, &dc_c, &ac_c, &mut prev_dc[2], &mut bw);
        }
    }
    bw.flush();
    out.extend_from_slice(&[0xff, 0xd9]); // EOI
    out
}

fn encode_block(
    block: &[f32; 64],
    cos_tbl: &[[f32; 8]; 8],
    qt: &[u8; 64],
    dc_huff: &[(u16, u8); 256],
    ac_huff: &[(u16, u8); 256],
    prev_dc: &mut i32,
    bw: &mut BitWriter<'_>,
) {
    let mut tmp = [0.0f32; 64];
    for r in 0..8 {
        for u in 0..8 {
            let mut s = 0.0f32;
            for x in 0..8 {
                s += block[r * 8 + x] * cos_tbl[u][x];
            }
            tmp[r * 8 + u] = s;
        }
    }
    let mut zz = [0i32; 64];
    for (zi, &idx) in ZIGZAG.iter().enumerate() {
        let v = idx / 8;
        let u = idx % 8;
        let mut s = 0.0f32;
        for y in 0..8 {
            s += tmp[y * 8 + u] * cos_tbl[v][y];
        }
        zz[zi] = ((s / f32::from(qt[idx])).round() as i32).clamp(-1023, 1023);
    }

    // DC coefficient
    let diff = (zz[0] - *prev_dc).clamp(-2047, 2047);
    *prev_dc = zz[0];
    let (cat, bits) = jpeg_vli(diff);
    let (c, l) = dc_huff[cat as usize];
    bw.write(c, l);
    if cat > 0 {
        bw.write(bits, cat);
    }

    // AC coefficients
    let mut last_nz = 63usize;
    while last_nz > 0 && zz[last_nz] == 0 {
        last_nz -= 1;
    }
    let mut i = 1usize;
    while i <= last_nz {
        let mut zeros = 0u8;
        while zz[i] == 0 {
            zeros += 1;
            i += 1;
        }
        while zeros >= 16 {
            let (c, l) = ac_huff[0xf0];
            bw.write(c, l);
            zeros -= 16;
        }
        let (cat, bits) = jpeg_vli(zz[i]);
        let sym = (zeros << 4) | cat;
        let (c, l) = ac_huff[sym as usize];
        bw.write(c, l);
        bw.write(bits, cat);
        i += 1;
    }
    if last_nz < 63 {
        let (c, l) = ac_huff[0x00]; // EOB
        bw.write(c, l);
    }
}

fn jpeg_vli(v: i32) -> (u8, u16) {
    if v == 0 {
        return (0, 0);
    }
    let abs = v.unsigned_abs();
    let cat = (32 - abs.leading_zeros()) as u8;
    let bits = if v > 0 {
        abs as u16
    } else {
        ((1u32 << cat) - 1 - abs) as u16
    };
    (cat, bits)
}

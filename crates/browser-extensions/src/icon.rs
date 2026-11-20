//! Safe extension raster icon reading, magic-byte sniffing, pixel-dimension bounds, and
//! symlink-free path containment (`within_root`).

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use crate::integrity::read_regular_file_bounded;
use crate::manifest::{IconSpec, Manifest};

const ICON_TARGET: u32 = 40;
const ICON_MAX_BYTES: u64 = 65_536;
const ICON_MAX_DIM: u32 = 1024;

/// Reads the manifest's declared raster icon as a `data:` URL (`<= 64 KiB`, no symlinks, no SVG,
/// verified magic bytes matching file extension, and bounded pixel dimensions `1..=1024`).
pub fn extension_icon_data_url(dir: &Path, manifest: &Manifest) -> Option<String> {
    use base64::Engine as _;

    let file = closest_icon(&manifest.icons).or_else(|| match manifest.action_default_icon.as_ref()? {
        IconSpec::Single(s) => Some(s.clone()),
        IconSpec::Sized(map) => closest_icon(map),
    })?;
    let path = within_root(dir, &file)?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let expected_mime = match ext.as_str() {
        "png" => "image/png",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        _ => return None,
    };
    let bytes = read_regular_file_bounded(&path, ICON_MAX_BYTES)?;
    let (sniffed_mime, width, height) = inspect_raster_icon(&bytes)?;
    if sniffed_mime != expected_mime
        || width == 0
        || height == 0
        || width > ICON_MAX_DIM
        || height > ICON_MAX_DIM
    {
        return None;
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Some(format!("data:{sniffed_mime};base64,{b64}"))
}

/// Sniffs magic bytes and parses header pixel dimensions of supported raster formats (PNG, GIF,
/// WebP, JPEG, ICO) to reject spoofed MIME types and `< 64 KiB` pixel bombs before UI decoding.
pub fn inspect_raster_icon(bytes: &[u8]) -> Option<(&'static str, u32, u32)> {
    // PNG: 8-byte signature + IHDR (len 13, type "IHDR", big-endian w/h).
    if bytes.len() >= 24 && &bytes[0..8] == b"\x89PNG\r\n\x1a\n" {
        if &bytes[8..16] != b"\x00\x00\x00\x0dIHDR" {
            return None;
        }
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return Some(("image/png", w, h));
    }
    // GIF87a / GIF89a: little-endian u16 w/h at bytes 6..10.
    if bytes.len() >= 10 && (&bytes[0..6] == b"GIF87a" || &bytes[0..6] == b"GIF89a") {
        let w = u32::from(u16::from_le_bytes([bytes[6], bytes[7]]));
        let h = u32::from(u16::from_le_bytes([bytes[8], bytes[9]]));
        return Some(("image/gif", w, h));
    }
    // WebP: RIFF....WEBP + VP8X / VP8L / VP8  chunk.
    if bytes.len() >= 30 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        let chunk = &bytes[12..16];
        if chunk == b"VP8X" {
            let w = 1 + (u32::from(bytes[24]) | (u32::from(bytes[25]) << 8) | (u32::from(bytes[26]) << 16));
            let h = 1 + (u32::from(bytes[27]) | (u32::from(bytes[28]) << 8) | (u32::from(bytes[29]) << 16));
            return Some(("image/webp", w, h));
        }
        if chunk == b"VP8L" && bytes[20] == 0x2f {
            let bits = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
            let w = (bits & 0x3fff) + 1;
            let h = ((bits >> 14) & 0x3fff) + 1;
            return Some(("image/webp", w, h));
        }
        if chunk == b"VP8 " && &bytes[23..26] == b"\x9d\x01\x2a" {
            let w = u32::from(u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3fff);
            let h = u32::from(u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3fff);
            return Some(("image/webp", w, h));
        }
        return None;
    }
    // JPEG: SOI (FF D8) + scan markers for SOF0/SOF1/SOF2 (C0..=C2).
    if bytes.len() >= 4 && bytes[0] == 0xff && bytes[1] == 0xd8 && bytes[2] == 0xff {
        let mut i = 2usize;
        let mut steps = 0usize;
        while i + 4 <= bytes.len() && steps < 128 {
            steps += 1;
            if bytes[i] != 0xff {
                return None;
            }
            while i < bytes.len() && bytes[i] == 0xff {
                i += 1;
            }
            if i >= bytes.len() {
                return None;
            }
            let marker = bytes[i];
            i += 1;
            if marker == 0xd9 || marker == 0xda {
                return None;
            }
            if (0xd0..=0xd7).contains(&marker) || marker == 0x01 {
                continue;
            }
            if i + 2 > bytes.len() {
                return None;
            }
            let seg_len = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
            if seg_len < 2 || i.checked_add(seg_len)? > bytes.len() {
                return None;
            }
            if matches!(marker, 0xc0..=0xc2) {
                if seg_len < 7 {
                    return None;
                }
                let h = u32::from(u16::from_be_bytes([bytes[i + 3], bytes[i + 4]]));
                let w = u32::from(u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]));
                return Some(("image/jpeg", w, h));
            }
            i += seg_len;
        }
        return None;
    }
    // ICO: reserved=0, type=1, count in 1..=64; inspect EVERY embedded PNG IHDR or BMP DIB
    // payload header (never trust the 1-byte nominal directory size alone, blocking ICO-wrapped
    // 65535x65535 PNG/BMP pixel bombs).
    if bytes.len() >= 6 && &bytes[0..4] == b"\x00\x00\x01\x00" {
        let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        let header_end = 6usize.checked_add(count.checked_mul(16)?)?;
        if count == 0 || count > 64 || bytes.len() < header_end {
            return None;
        }
        let mut max_w = 0u32;
        let mut max_h = 0u32;
        for idx in 0..count {
            let base = 6 + idx * 16;
            let bytes_in_res = usize::try_from(u32::from_le_bytes([
                bytes[base + 8],
                bytes[base + 9],
                bytes[base + 10],
                bytes[base + 11],
            ]))
            .ok()?;
            let img_off = usize::try_from(u32::from_le_bytes([
                bytes[base + 12],
                bytes[base + 13],
                bytes[base + 14],
                bytes[base + 15],
            ]))
            .ok()?;
            let img_end = img_off.checked_add(bytes_in_res)?;
            if img_off < header_end || img_end > bytes.len() || bytes_in_res < 24 {
                return None;
            }
            let payload = &bytes[img_off..img_end];
            let (ew, eh) = if payload.len() >= 24 && &payload[0..8] == b"\x89PNG\r\n\x1a\n" {
                if &payload[8..16] != b"\x00\x00\x00\x0dIHDR" {
                    return None;
                }
                let w = u32::from_be_bytes([payload[16], payload[17], payload[18], payload[19]]);
                let h = u32::from_be_bytes([payload[20], payload[21], payload[22], payload[23]]);
                (w, h)
            } else if payload.len() >= 40 {
                let dib_size = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
                if dib_size < 40 || (dib_size as usize) > payload.len() {
                    return None;
                }
                let raw_w = i32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
                let raw_h = i32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
                if raw_w <= 0 || raw_h == 0 || !raw_h.unsigned_abs().is_multiple_of(2) {
                    return None;
                }
                // ICO BMP DIB stores height = 2 * actual height (XOR color mask + AND mask).
                let h = raw_h.unsigned_abs() / 2;
                (raw_w as u32, h)
            } else {
                return None;
            };
            if ew == 0 || eh == 0 {
                return None;
            }
            max_w = max_w.max(ew);
            max_h = max_h.max(eh);
        }
        return Some(("image/x-icon", max_w, max_h));
    }
    None
}

/// Resolves `rel` strictly inside `root`, refusing symlinks, `..`, absolute paths, and `root` itself.
pub fn within_root(root: &Path, rel: &str) -> Option<PathBuf> {
    let stripped = rel.trim_start_matches(['/', '\\']);
    if stripped.is_empty() {
        return None;
    }
    let rel_path = Path::new(stripped);
    if rel_path.is_absolute() {
        return None;
    }
    let mut cur = root.to_path_buf();
    let mut pushed = false;
    for c in rel_path.components() {
        match c {
            Component::CurDir => continue,
            Component::Normal(seg) => {
                pushed = true;
                cur.push(seg);
                if crate::integrity::is_reparse_or_symlink(&std::fs::symlink_metadata(&cur).ok()?) {
                    return None;
                }
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if !pushed {
        return None;
    }
    let canon_root = std::fs::canonicalize(root).ok()?;
    let canon_target = std::fs::canonicalize(&cur).ok()?;
    (canon_target != canon_root && canon_target.starts_with(&canon_root)).then_some(canon_target)
}

fn closest_icon(sizes: &HashMap<String, String>) -> Option<String> {
    let mut list: Vec<(u32, &String)> = sizes
        .iter()
        .filter_map(|(k, v)| Some((k.parse::<u32>().ok()?, v)))
        .collect();
    list.sort_by_key(|(sz, _)| *sz);
    list.iter()
        .find(|(sz, _)| *sz >= ICON_TARGET)
        .or_else(|| list.last())
        .map(|(_, v)| (*v).clone())
}

//! Dragging a timeline image out into another app (Finder, Messages, Slack) or into this app's
//! own composer: the drag carries the image's own bytes as a file, written to a temporary
//! folder when the drag starts.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use opencode_kit::model::Attachment;
use rune::{DragItem, El};

use super::MediaStore;
use crate::TimelineHost;

/// Where a timeline image's bytes come from: what dragging it carries and what its right-click
/// menu acts on.
#[derive(Clone, Debug)]
pub enum Dragged {
    /// A prompt attachment or a tool result's image.
    File(Attachment),
    /// A markdown image in a reply, by its source as written.
    Markdown { directory: Option<String>, src: String },
}

/// `e` dragged out as its image's file, once the pointer leaves the press by the platform's
/// threshold; a press released before that stays a press.
pub fn draggable<S: TimelineHost>(e: El, dragged: Dragged) -> El {
    e.drag_out(move |s: &S| s.media_store().drag_item(&dragged))
}

impl MediaStore {
    /// What dragging `d` out carries: its bytes as a file, `None` until they're here. The app's
    /// own windows take it too, so the composer can attach it.
    pub fn drag_item(&self, d: &Dragged) -> Option<DragItem> {
        Some(DragItem::new().file(self.image_file(d)?).within_app())
    }

    /// `d`'s encoded bytes and a name to give them, once they're here.
    pub fn image_bytes(&self, d: &Dragged) -> Option<(Arc<[u8]>, String)> {
        match d {
            Dragged::File(f) => Some((self.bytes(f)?, f.name.clone())),
            Dragged::Markdown { directory, src } => Some((self.markdown.bytes(directory.as_deref(), src)?, src.clone())),
        }
    }

    /// `d`'s bytes as a file in the drag folder, named for the image.
    pub fn image_file(&self, d: &Dragged) -> Option<PathBuf> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = d;
            None
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (bytes, hint) = self.image_bytes(d)?;
            write(&root(), &bytes, &hint)
        }
    }
}

/// The folder dragged and copied images are written to.
#[cfg(not(target_arch = "wasm32"))]
fn root() -> PathBuf {
    std::env::temp_dir().join("opencode-drag")
}

/// Whether `path` is a timeline image this app wrote for a drag or a copy: its own drag coming
/// back to it.
pub fn is_own(path: &Path) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        false
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let raw = path.to_string_lossy();
        if browser_policy::is_unc_or_device_path(&raw) {
            return false;
        }
        let root = root();
        path.starts_with(&root)
            || browser_policy::verify_within_root(&raw, &root.to_string_lossy(), true).is_some()
    }
}

/// Writes `bytes` once into a folder named by their hash under `root`, so equal names from
/// different images never collide and a second drag reuses the file. Only writes recognized
/// safe raster image formats (never untrusted attacker extensions or active SVG/HTML/scripts).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn write(root: &Path, bytes: &[u8], hint: &str) -> Option<PathBuf> {
    use sha2::{Digest, Sha256};
    if bytes.is_empty() || sniff_image_ext(bytes).is_none() {
        return None;
    }
    let digest = Sha256::digest(bytes);
    let dir = root.join(digest[..8].iter().map(|b| format!("{b:02x}")).collect::<String>());
    let path = dir.join(file_name(hint, bytes));
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file() && m.len() == bytes.len() as u64) {
        return Some(path);
    }
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

/// Sniffs `bytes` against an explicit allow-list of safe raster image formats. Never trusts a
/// URL or attachment extension.
pub(crate) fn sniff_image_ext(bytes: &[u8]) -> Option<&'static str> {
    match image::guess_format(bytes) {
        Ok(image::ImageFormat::Png) => Some("png"),
        Ok(image::ImageFormat::Jpeg) => Some("jpg"),
        Ok(image::ImageFormat::Gif) => Some("gif"),
        Ok(image::ImageFormat::WebP) => Some("webp"),
        Ok(image::ImageFormat::Bmp) => Some("bmp"),
        Ok(image::ImageFormat::Ico) => Some("ico"),
        _ => {
            if bytes.len() >= 12
                && &bytes[4..8] == b"ftyp"
                && matches!(&bytes[8..12], b"avif" | b"avis")
            {
                Some("avif")
            } else {
                None
            }
        }
    }
}

fn sanitize_stem(hint: &str) -> String {
    let last = hint.split(['?', '#']).next().unwrap_or("").rsplit(['/', '\\']).next().unwrap_or("");
    let raw_stem = match last.rsplit_once('.') {
        Some((s, _)) if !s.is_empty() => s,
        _ => last,
    };
    let clean: String = raw_stem
        .chars()
        .filter(|&c| {
            let n = c as u32;
            !c.is_control()
                && !matches!(n, 0x7f..=0x9f | 0x061c | 0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069 | 0xfeff)
                && !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
        })
        .take(64)
        .collect();
    let trimmed = clean.trim().trim_matches(['.', ' ']);
    let device_stem = trimmed.split('.').next().unwrap_or(trimmed);
    if trimmed.is_empty() || browser_policy::is_windows_device_name(device_stem) {
        "image".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Whether `bytes` look like an SVG document (used ONLY to suggest a `.svg` filename in the
/// user-confirmed Save Image As… dialog — never for drag-out or opening in a browser tab).
fn looks_like_svg(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let trimmed = bytes.trim_ascii_start();
    trimmed.get(..4).is_some_and(|h| h.eq_ignore_ascii_case(b"<svg"))
        || (trimmed.get(..5).is_some_and(|h| h.eq_ignore_ascii_case(b"<?xml"))
            && trimmed.windows(4).take(512).any(|w| w.eq_ignore_ascii_case(b"<svg")))
}

/// A safe file name for drag-out / reveal: sanitized stem from `hint` paired strictly with an
/// allow-listed raster extension sniffed from `bytes` (NEVER keeping `.bat`, `.hta`, `.cmd`,
/// `.svg`, or `.html`).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn file_name(hint: &str, bytes: &[u8]) -> String {
    let stem = sanitize_stem(hint);
    let ext = sniff_image_ext(bytes).unwrap_or("png");
    format!("{stem}.{ext}")
}

/// Suggested filename for the user-confirmed Save Image As… file dialog: preserves `.svg` when
/// `bytes` sniff as SVG so saved SVGs aren't misnamed `.png`; all other images keep sniffed
/// raster extensions only.
pub(crate) fn save_file_name(hint: &str, bytes: &[u8]) -> String {
    let stem = sanitize_stem(hint);
    let ext = sniff_image_ext(bytes).unwrap_or_else(|| if looks_like_svg(bytes) { "svg" } else { "png" });
    format!("{stem}.{ext}")
}


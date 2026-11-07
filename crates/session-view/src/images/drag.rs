//! Dragging a timeline image out into another app (Finder, Messages, Slack): the drag carries
//! the image's own bytes as a file, written to a temporary folder when the drag starts.

use std::path::{Path, PathBuf};

use opencode_kit::model::Attachment;
use rune::{DragItem, El};

use super::MediaStore;
use crate::TimelineHost;

/// Where a dragged image's bytes come from.
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
    /// What dragging `d` out carries: its bytes as a file, `None` until they're here.
    pub fn drag_item(&self, d: &Dragged) -> Option<DragItem> {
        let (bytes, hint) = match d {
            Dragged::File(f) => (self.bytes(f)?, f.name.clone()),
            Dragged::Markdown { directory, src } => (self.markdown.bytes(directory.as_deref(), src)?, src.clone()),
        };
        let path = write(&std::env::temp_dir().join("opencode-drag"), &bytes, &hint)?;
        Some(DragItem::new().file(path))
    }
}

/// Writes `bytes` once into a folder named by their hash under `root`, so equal names from
/// different images never collide and a second drag reuses the file.
pub(crate) fn write(root: &Path, bytes: &[u8], hint: &str) -> Option<PathBuf> {
    use sha2::{Digest, Sha256};
    if bytes.is_empty() {
        return None;
    }
    let digest = Sha256::digest(bytes);
    let dir = root.join(digest[..8].iter().map(|b| format!("{b:02x}")).collect::<String>());
    let path = dir.join(file_name(hint, bytes));
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64) {
        return Some(path);
    }
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

/// A file name for an image: the hint's last path segment (a URL's without its query), with
/// an extension that matches the bytes, or `image` when the hint has no usable name.
pub(crate) fn file_name(hint: &str, bytes: &[u8]) -> String {
    let last = hint.split(['?', '#']).next().unwrap_or("").rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = last.chars().map(|c| if c.is_control() || c == ':' { '-' } else { c }).collect();
    let clean = clean.trim().trim_start_matches('.');
    let (stem, ext) = match clean.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !e.is_empty() && e.len() <= 5 => (s, Some(e.to_ascii_lowercase())),
        _ => (clean, None),
    };
    let stem = if stem.is_empty() { "image" } else { stem };
    let sniffed = match image::guess_format(bytes) {
        Ok(image::ImageFormat::Png) => Some("png"),
        Ok(image::ImageFormat::Jpeg) => Some("jpg"),
        Ok(image::ImageFormat::Gif) => Some("gif"),
        Ok(image::ImageFormat::WebP) => Some("webp"),
        _ => None,
    };
    let ext = match (ext, sniffed) {
        (Some(e), Some("jpg")) if e == "jpeg" || e == "jpg" => e,
        (_, Some(s)) => s.to_owned(),
        (Some(e), None) => e,
        (None, None) => "png".to_owned(),
    };
    format!("{stem}.{ext}")
}

#[cfg(test)]
#[path = "drag_tests.rs"]
mod tests;

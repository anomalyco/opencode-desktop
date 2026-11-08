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
    /// What dragging `d` out carries: its bytes as a file, `None` until they're here.
    pub fn drag_item(&self, d: &Dragged) -> Option<DragItem> {
        Some(DragItem::new().file(self.image_file(d)?))
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
        let (bytes, hint) = self.image_bytes(d)?;
        write(&root(), &bytes, &hint)
    }
}

/// The folder dragged and copied images are written to.
fn root() -> PathBuf {
    std::env::temp_dir().join("opencode-drag")
}

/// Whether `path` is a timeline image this app wrote for a drag or a copy: its own drag coming
/// back to it.
pub fn is_own(path: &Path) -> bool {
    let root = root();
    path.starts_with(&root) || std::fs::canonicalize(path).ok().zip(std::fs::canonicalize(&root).ok()).is_some_and(|(p, r)| p.starts_with(r))
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

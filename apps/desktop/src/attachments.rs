//! Images and files on prompts: the composer's attachments (pasted, dropped or picked with ⌘U),
//! their chips, the images user messages and tool results carry in the transcript, the local
//! echo of a sent prompt, and the viewer a thumbnail opens.
//!
//! As in Desktop (`composer/attachments/attachments.ts`), media the model reads natively (PNG,
//! JPEG, GIF, WebP and PDF up to 20 MB) travels inline; any other file reaches it by path: its
//! own on a local server, else a copy uploaded to the server's temporary directory.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
#[cfg(test)]
use std::sync::{Arc, Mutex};

#[cfg(test)]
use rune::Cx;
use rune::{
    ClipboardContent, El, Image, ImageCache, SurfaceFit, TextStyle, Tx, Waker, col, el, icon, row,
    text,
};
use rune_theme_opencode::icons as ic;

use opencode_kit::client::{FileInput, PathAttachment, data_url};
use opencode_kit::model::{Attachment, Message, Transcript};

use crate::App;
use crate::palette::Pal;

mod images;
mod viewer;

pub(crate) use images::{Fetched, PreparedImage, inline_images, prepare_images};
use images::Media;
#[cfg(test)]
use images::{Fetch, fit};
pub use viewer::{Viewing, view_image, viewer};
use viewer::opens;
#[cfg(test)]
use viewer::shown;

/// The server's cap on one attachment.
const MAX_BYTES: usize = 20 << 20;
const SMALL: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);
/// `attachment-card.css`: 11/14, with room for Inter's descenders where it truncates.
pub(crate) const CARD_TITLE: TextStyle = TextStyle::new(11.0, 530.0).lh(14.0).tracking(0.05);
pub(crate) const CARD_SUBTITLE: TextStyle = TextStyle::new(11.0, 440.0).lh(14.0).tracking(0.05);

pub struct Attach {
    /// Waiting in the composer for the next send, inline.
    pub files: Vec<Attachment>,
    /// Waiting in the composer for the next send, by path on the server.
    pub paths: Vec<PathAttachment>,
    /// Uploads still copying to a remote server; sending waits for them.
    pub uploading: Vec<String>,
    uploads_tx: Sender<Result<PathAttachment, Failed>>,
    uploads: Receiver<Result<PathAttachment, Failed>>,
    /// A file drag over the page, which shows the drop overlay.
    pub drop: crate::dropzone::Dropzone,
    /// Prompts shown before the server's copy: (session, local id, text).
    pub echoes: Vec<(String, String, String)>,
    /// The image open in the viewer.
    pub viewing: Option<Viewing>,
    waker: Option<Waker>,
    cache: RefCell<Option<ImageCache>>,
    /// Decoded handles by the bytes' address, so a view doesn't re-hash megabytes each frame.
    images: RefCell<HashMap<(usize, usize), PreparedImage>>,
    /// `media:<hash>` refs: fetched once, decoded once, by hash.
    media: Media,
    refs: RefCell<HashMap<String, Image>>,
}

impl Default for Attach {
    fn default() -> Self {
        let (uploads_tx, uploads) = channel();
        Attach {
            files: Vec::new(),
            paths: Vec::new(),
            uploading: Vec::new(),
            uploads_tx,
            uploads,
            drop: Default::default(),
            echoes: Vec::new(),
            viewing: None,
            waker: None,
            cache: RefCell::new(None),
            images: RefCell::new(HashMap::new()),
            media: Media::default(),
            refs: RefCell::new(HashMap::new()),
        }
    }
}

impl Attach {
    /// Something to send besides text.
    pub fn any(&self) -> bool {
        !self.files.is_empty() || !self.paths.is_empty()
    }

    pub fn add_path(&mut self, r: PathAttachment) {
        if !self.paths.iter().any(|x| x.path == r.path) {
            self.paths.push(r);
        }
    }

    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    pub fn add(&mut self, a: Attachment) {
        if !self.files.iter().any(|f| f.bytes == a.bytes && f.media == a.media) {
            self.files.push(a);
        }
    }
}

/// The type by extension of what the model reads natively, else None.
fn native_mime(path: &Path) -> Option<&'static str> {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        Some("pdf") => Some("application/pdf"),
        _ => None,
    }
}

/// `attachmentMime`: native media by extension; otherwise text when the first 4 KB has no NUL
/// and few control bytes, else binary.
pub fn mime_for(path: &Path, head: &[u8]) -> &'static str {
    if let Some(m) = native_mime(path) {
        return m;
    }
    let control = head.iter().filter(|&&b| b < 9 || (b > 13 && b < 32)).count();
    if head.contains(&0) || (!head.is_empty() && control as f32 / head.len() as f32 > 0.3) {
        return "application/octet-stream";
    }
    "text/plain"
}

fn name_of(path: &Path) -> String {
    path.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_owned()
}

fn head(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut buf = Vec::with_capacity(4096);
    std::fs::File::open(path).and_then(|f| f.take(4096).read_to_end(&mut buf)).map_err(|e| format!("{}: {e}", name_of(path)))?;
    Ok(buf)
}

/// A native file small enough to go inline.
pub fn from_path(path: &Path) -> Result<Attachment, String> {
    let name = name_of(path);
    let mime = native_mime(path).ok_or_else(|| format!("{name} doesn't go inline"))?;
    let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
    if bytes.len() > MAX_BYTES {
        return Err(format!("{name} is over 20 MB"));
    }
    Ok(Attachment { mime: mime.into(), name, bytes: bytes.into(), media: None })
}

/// An upload that didn't reach the server: the file, to try again.
pub struct Failed {
    name: String,
    path: std::path::PathBuf,
    error: String,
}

impl App {
    /// Attaches a file from disk: inline when the model reads it natively and it fits, else by
    /// path, uploading a copy first when the server is on another machine.
    pub fn attach_path(&mut self, path: &Path) {
        let name = name_of(path);
        let size = match std::fs::metadata(path) {
            Ok(m) if m.is_file() => m.len() as usize,
            Ok(_) => return crate::status_toast::say(self, "Couldn't attach a file", format!("{name} is a folder. Only files can be attached.")),
            Err(e) => return crate::status_toast::fail(self, "Couldn't attach a file", format!("{name}: {}", crate::status_toast::plain(&e.to_string()))),
        };
        if native_mime(path).is_some() && size <= MAX_BYTES {
            match from_path(path) {
                Ok(a) => self.attach.add(a),
                Err(e) => crate::status_toast::fail(self, "Couldn't attach a file", crate::status_toast::plain(&e)),
            }
            return;
        }
        let mime = match head(path) {
            Ok(h) => mime_for(path, &h).to_owned(),
            Err(e) => return crate::status_toast::fail(self, "Couldn't attach a file", crate::status_toast::plain(&e)),
        };
        let server = match &self.source {
            crate::Source::Live { server, .. } if !server.is_local() => server.clone(),
            _ => return self.attach.add_path(PathAttachment { name, mime, path: path.to_string_lossy().into_owned() }),
        };
        let (out, wake, directory, path) = (self.attach.uploads_tx.clone(), self.attach.waker.clone(), self.composer_directory(), path.to_owned());
        self.attach.uploading.push(name.clone());
        std::thread::spawn(move || {
            let copied = std::fs::read(&path).map_err(|e| format!("{name}: {e}")).and_then(|bytes| server.upload(&directory, &name, &bytes));
            let _ = out.send(match copied {
                Ok(at) => Ok(PathAttachment { name, mime, path: at }),
                Err(error) => Err(Failed { name, path, error }),
            });
            if let Some(w) = wake {
                w();
            }
        });
    }

    /// Takes in finished uploads.
    pub fn poll_uploads(&mut self) -> bool {
        let mut changed = false;
        while let Ok(done) = self.attach.uploads.try_recv() {
            match done {
                Ok(r) => {
                    if let Some(i) = self.attach.uploading.iter().position(|n| *n == r.name) {
                        self.attach.uploading.remove(i);
                    }
                    self.attach.add_path(r);
                }
                Err(f) => {
                    if let Some(i) = self.attach.uploading.iter().position(|n| *n == f.name) {
                        self.attach.uploading.remove(i);
                    }
                    let line = crate::status_toast::plain(f.error.trim_start_matches("upload "));
                    crate::status_toast::fail_retry(self, "Upload failed", line, crate::status_toast::ToastAction::Attach(f.path));
                }
            }
            changed = true;
        }
        changed
    }

    /// ⌘U and the + menu's first item: the system's file picker, then the file.
    pub fn pick_attachment(&mut self, tx: &mut Tx) {
        let options = rune::FileDialogOptions { title: "Attach images and files".into(), directory: Some(self.composer_directory().into()), ..Default::default() };
        tx.open_file(options, |s: &mut App, result, tx: &mut Tx| match result {
            Ok(Some(path)) => {
                tx.instant();
                s.attach_path(&path);
                s.composer.focused = true;
            }
            Ok(None) => {}
            Err(e) => crate::status_toast::fail(s, "Couldn't open the file picker", e.to_string()),
        });
    }
}

/// The `Attached file:` lines a prompt's path attachments add to its text.
pub fn path_lines(text: &str, refs: &[PathAttachment]) -> String {
    let lines = refs.iter().map(|r| format!("Attached file: `{}`", r.path));
    (!text.trim().is_empty()).then(|| text.to_owned()).into_iter().chain(lines).collect::<Vec<_>>().join("\n")
}

/// The prompt's `files`: each attachment inline as a data URL, or a ref the server sent as its
/// `media:` URI, which the server re-admits from what it stored.
pub fn inputs(files: &[Attachment]) -> Vec<FileInput> {
    files
        .iter()
        .map(|f| match &f.media {
            Some(hash) => FileInput { uri: format!("media:{hash}"), name: f.name.clone(), mime: Some(f.mime.clone()), mention: None },
            None => FileInput { uri: data_url(&f.mime, &f.bytes), name: f.name.clone(), ..Default::default() },
        })
        .collect()
}

/// A paste into the composer: an image becomes an attachment (true: the editor skips it).
pub fn paste(s: &mut App, content: &ClipboardContent, _: &mut Tx) -> bool {
    let Some(img) = content.image() else { return false };
    let n = s.attach.files.len() + 1;
    s.attach.add(Attachment { mime: "image/png".into(), name: format!("pasted-image-{n}.png"), bytes: img.png().into(), media: None });
    true
}

/// Before merging a fetched page: settles the open echoes against the server's messages. An
/// echo whose id came back is done (the merge replaces it); one the server stored under another
/// id (a server that ignores the client id) is dropped once a user message with its text lands.
pub fn reconcile(echoes: &mut Vec<(String, String, String)>, t: &mut Transcript, fresh: &[Message]) {
    echoes.retain(|(session, id, body)| {
        if *session != t.session {
            return true;
        }
        if fresh.iter().any(|m| m.id() == id) {
            return false;
        }
        let known = |mid: &str| t.messages.iter().any(|m| m.id() == mid);
        let twin = fresh.iter().any(|m| matches!(m, Message::User { id: mid, text } if mid != id && text.trim() == body && !known(mid)));
        if twin {
            t.messages.retain(|m| m.id() != id);
        }
        !twin
    });
}

/// The composer's attachments (`editor.tsx`): an image is a 58×46 thumbnail, any other file a
/// 160 px card with its name and type, and each shows a round remove button on hover.
pub fn chips(s: &App, p: &Pal) -> Option<El> {
    if !s.attach.any() && s.attach.uploading.is_empty() && !s.quote.any() {
        return None;
    }
    // Quotes come first (`editor.tsx`).
    let mut r = row().key("attachments").w_full().px(8.0).pt(8.0).pb(4.0).gap(8.0).flex_wrap().children(crate::quote_cards::chips(s, p));
    for f in &s.attach.files {
        let id = identity(f);
        let img = s.attach.image(f);
        let w = if img.is_some() { 58.0 } else { 160.0 };
        let body = match img {
            Some(img) => {
                let thumb = el()
                    .key("thumb")
                    .size(58.0, 46.0)
                    .radius(6.0)
                    .fill(p.layer2)
                    .pixels_fit(img.pixels(), SurfaceFit::Fill)
                    // A ring over the image, not in place of its edge.
                    .child(el().key("thumb-ring").absolute(0.0, 0.0).size(58.0, 46.0).radius(6.0).border(0.5, p.border));
                opens(s, thumb, el().key("held").size(58.0, 46.0), format!("composer:{id}"), f, img)
            }
            None => card(p, &f.name, type_label(&f.name)),
        };
        let gone = id.clone();
        let remove = remove_button(p, &f.name, w, move |s| s.attach.files.retain(|f| identity(f) != gone));
        r = r.child(el().key(format!("chip-{id}")).hover_group().child(body).child(remove));
    }
    for f in &s.attach.paths {
        let gone = f.path.clone();
        let remove = remove_button(p, &f.name, 160.0, move |s| s.attach.paths.retain(|f| f.path != gone));
        r = r.child(el().key(format!("path-{}", f.path)).hover_group().child(card(p, &f.name, type_label(&f.name))).child(remove));
    }
    for (i, name) in s.attach.uploading.iter().enumerate() {
        r = r.child(el().key(format!("uploading-{i}")).child(card(p, name, "Uploading…".into())));
    }
    Some(r)
}

/// The round button on a chip's corner that shows on hover, for a chip `w` wide.
pub(crate) fn remove_button(p: &Pal, name: &str, w: f32, on: impl Fn(&mut App) + 'static) -> El {
    el()
        .key("remove")
        .role("button")
        .label(format!("Remove {name}"))
        // Desktop's `-top-1 -end-1`.
        .absolute(w - 12.0, -4.0)
        .size(16.0, 16.0)
        .radius(8.0)
        .center()
        .fill(p.icon_muted)
        .outline(1.0, 0.0, p.icon_contrast)
        .group_opacity(0.0, 1.0)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            on(s);
        })
        .child(icon(&ic::XMARK_SMALL, 12.0).color(p.icon_contrast))
}

fn card(p: &Pal, title: &str, subtitle: String) -> El {
    col()
        .key("card")
        .w(160.0)
        .padding(8.0)
        .gap(6.0)
        .radius(6.0)
        .fill(p.hover)
        .border(0.5, p.border)
        .child(text(title.to_owned()).font(CARD_TITLE).color(p.text).truncate_fade().w_full())
        .child(text(subtitle).font(CARD_SUBTITLE).color(p.muted).truncate_fade())
}

/// A file's type as the card's second line: its extension, upper-cased (Desktop's `typeLabel`).
fn type_label(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() => ext.to_uppercase(),
        _ => "File".into(),
    }
}

/// Whether `gallery` shows anything for `files`, without loading them.
pub fn shows(files: &[Attachment]) -> bool {
    files.iter().any(|f| f.is_image() || f.unavailable().is_some())
}

/// Images in the transcript, at most 320×240 points (2× pixels), right-aligned for a user. A
/// ref still on its way holds a box; one whose bytes are gone shows the server's placeholder.
pub fn gallery(s: &App, p: &Pal, key: &str, files: &[Attachment], end: bool) -> Option<El> {
    let images: Vec<&Attachment> = files.iter().filter(|f| shows(std::slice::from_ref(f))).collect();
    if images.is_empty() {
        return None;
    }
    let mut r = row().key(key).w_full().gap(8.0).flex_wrap();
    if end {
        r = r.justify_end();
    }
    for (i, f) in images.into_iter().enumerate() {
        let k = format!("img-{i}");
        r = r.child(match s.attach.image(f) {
            Some(img) if img.error().is_none() => {
                let (w, h) = s.attach.thumbnail_size(f);
                let held = el().key(format!("{k}-held")).size(w, h);
                let thumb = el().key(k).label(f.name.clone()).size(w, h).radius(8.0).fill(p.layer2).border(0.5, p.border_muted);
                opens(s, thumb.pixels_fit(img.pixels(), SurfaceFit::Fit), held, format!("{key}:{}", identity(f)), f, img)
            }
            None if s.attach.fetched(f) == Some(Fetched::Loading) => el().key(format!("{k}-loading")).label(f.name.clone()).size(160.0, 120.0).radius(8.0).fill(p.layer2),
            _ => {
                let label = f.unavailable().map_or_else(|| f.name.clone(), str::to_owned);
                row().key(k).h(32.0).px(10.0).center_y().radius(8.0).fill(p.layer2).child(text(label).font(SMALL).color(p.muted))
            }
        });
    }
    Some(col().w_full().pb(8.0).child(r))
}

/// One attachment's bytes (or, for a ref, the attachment itself), so each thumbnail has its own
/// shared-element id.
fn identity(f: &Attachment) -> String {
    match &f.media {
        Some(hash) => format!("{:x}:{hash}", f as *const Attachment as usize),
        None => format!("{:x}:{}", f.bytes.as_ptr() as usize, f.bytes.len()),
    }
}

#[cfg(test)]
mod tests;

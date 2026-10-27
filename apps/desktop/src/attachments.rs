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
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};

use rune::{
    ClipboardContent, Color, Cx, El, Image, ImageCache, Spring, SpringPair, SurfaceFit, TextStyle, Theme, Transition, Tx, Waker, col, el,
    icon, row, text,
};
use rune_theme_opencode::icons as ic;

use opencode_kit::client::{FileInput, PathAttachment, data_url};
use opencode_kit::model::{Attachment, Message, Transcript};

use crate::App;
use crate::palette::Pal;

/// The server's cap on one attachment.
const MAX_BYTES: usize = 20 << 20;
const SMALL: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);
/// `attachment-card.css`: 11/14, with room for Inter's descenders where it truncates.
const CARD_TITLE: TextStyle = TextStyle::new(11.0, 530.0).lh(14.0).tracking(0.05);
const CARD_SUBTITLE: TextStyle = TextStyle::new(11.0, 440.0).lh(14.0).tracking(0.05);

pub struct Attach {
    /// Waiting in the composer for the next send, inline.
    pub files: Vec<Attachment>,
    /// Waiting in the composer for the next send, by path on the server.
    pub paths: Vec<PathAttachment>,
    /// Uploads still copying to a remote server; sending waits for them.
    pub uploading: Vec<String>,
    uploads_tx: Sender<Result<PathAttachment, String>>,
    uploads: Receiver<Result<PathAttachment, String>>,
    /// A file drag is over the page, which shows the drop overlay.
    pub drop_over: bool,
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

/// Header dimensions are available before decode; the same handle crosses from prefetch to view.
#[derive(Clone)]
pub(crate) struct PreparedImage {
    bytes: Arc<[u8]>,
    image: Image,
    size: Option<[u32; 2]>,
}

fn dimensions(bytes: &[u8]) -> Option<[u32; 2]> {
    let (w, h) = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?.into_dimensions().ok()?;
    (w > 0 && h > 0).then_some([w, h])
}

impl PreparedImage {
    fn new(cache: &ImageCache, bytes: Arc<[u8]>) -> Self {
        Self { size: dimensions(&bytes), image: cache.load(bytes.clone()), bytes }
    }

    pub fn weight(&self) -> usize {
        // RGBA plus the mip chain; encoded bytes are counted by the page cache.
        self.size.map_or(0, |[w, h]| w as usize * h as usize * 16 / 3)
    }

    #[cfg(test)]
    pub fn wait(&self) { self.image.wait().unwrap(); }
}

/// Called only on the page worker. Newest attachments first, at most eight megapixels per
/// page; older history stays demand-loaded. The page cache also counts decoded pixel weight.
pub(crate) fn prepare_images(messages: &[Message], cache: &ImageCache) -> Vec<PreparedImage> {
    use opencode_kit::model::{Part, user_files};
    let mut prepared = Vec::new();
    let mut pixels = 0u64;
    let mut add = |f: &Attachment| {
        if !decodable(&f.mime) || f.media.is_some() { return; }
        let Some([w, h]) = dimensions(&f.bytes) else { return };
        let count = u64::from(w) * u64::from(h);
        if pixels + count > 8_000_000 || prepared.len() >= 8 { return; }
        pixels += count;
        prepared.push(PreparedImage::new(cache, f.bytes.clone()));
    };
    for m in messages.iter().rev() {
        match m {
            Message::User { id, .. } => { for f in user_files(id).iter().rev() { add(f); } }
            Message::Assistant(a) => for p in a.parts.iter().rev() {
                if let Part::Tool { files, .. } = p { for f in files.iter().rev() { add(f); } }
            },
            _ => {}
        }
    }
    prepared
}

/// Reads a `media:` ref's bytes (`Server::media` live; a stub in tests).
pub type Fetch = Arc<dyn Fn(&str) -> Result<Vec<u8>, String> + Send + Sync>;

/// Where a ref's bytes are.
#[derive(Clone, Debug, PartialEq)]
pub enum Fetched {
    Loading,
    Ready(Arc<[u8]>),
    Failed(String),
    /// Handed to the image cache, which holds the pixels now.
    Decoded,
}

/// Ref fetches, on one worker thread that takes the newest request first: a view asks top to
/// bottom, and the bottom of a transcript is what shows.
#[derive(Default)]
struct Media {
    fetch: Option<Fetch>,
    state: Arc<Mutex<HashMap<String, Fetched>>>,
    queue: RefCell<Option<Arc<(Mutex<Vec<String>>, Condvar)>>>,
}

impl Media {
    fn get(&self, hash: &str, waker: Option<&Waker>) -> Fetched {
        let mut state = self.state.lock().unwrap();
        if let Some(f) = state.get(hash) {
            return f.clone();
        }
        let Some(fetch) = self.fetch.clone() else { return Fetched::Failed("no server".into()) };
        state.insert(hash.to_owned(), Fetched::Loading);
        drop(state);
        let mut queue = self.queue.borrow_mut();
        let q = queue.get_or_insert_with(|| {
            let q = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
            spawn_fetcher(Arc::downgrade(&q), self.state.clone(), fetch, waker.cloned().unwrap_or_else(|| Arc::new(|| {})));
            q
        });
        q.0.lock().unwrap().push(hash.to_owned());
        q.1.notify_one();
        Fetched::Loading
    }
}

fn spawn_fetcher(queue: Weak<(Mutex<Vec<String>>, Condvar)>, state: Arc<Mutex<HashMap<String, Fetched>>>, fetch: Fetch, wake: Waker) {
    std::thread::Builder::new()
        .name("opencode-media".into())
        .spawn(move || {
            loop {
                // Ends once the app (the queue's owner) is gone.
                let Some(q) = queue.upgrade() else { return };
                let next = {
                    let mut stack = q.0.lock().unwrap();
                    if stack.is_empty() {
                        stack = q.1.wait_timeout(stack, std::time::Duration::from_secs(1)).unwrap().0;
                    }
                    stack.pop()
                };
                drop(q);
                let Some(hash) = next else { continue };
                let result = match fetch(&hash) {
                    Ok(bytes) => Fetched::Ready(bytes.into()),
                    Err(e) => Fetched::Failed(e),
                };
                state.lock().unwrap().insert(hash, result);
                wake();
            }
        })
        .expect("spawn media thread");
}

/// What the cache decodes; other types show as a named chip.
fn decodable(mime: &str) -> bool {
    matches!(mime, "image/png" | "image/jpeg" | "image/jpg")
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
            drop_over: false,
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
    /// A story controls byte arrival with the replay clock, then uses the real decoder.
    pub(crate) fn fixture_media(&self, hash: &str, bytes: Option<Vec<u8>>) {
        let ready = bytes.is_some();
        self.media.state.lock().unwrap().insert(hash.into(), bytes.map_or(Fetched::Loading, |b| Fetched::Ready(b.into())));
        if ready {
            let f = Attachment { mime: "image/png".into(), media: Some(hash.into()), ..Default::default() };
            self.image(&f).unwrap().wait().unwrap();
        }
    }

    /// Start uncached/ref reads in the newest message window too. Older history stays
    /// demand-loaded; preparing a page is not the same as finishing its image decodes.
    pub(crate) fn transcript_ready(&self, t: &Transcript) -> bool {
        use opencode_kit::model::{Part, user_files};
        let mut ready = true;
        let mut check = |f: &Attachment| {
            if !decodable(&f.mime) { return; }
            let loaded = self.image(f).is_some_and(|i| i.is_ready())
                || matches!(self.fetched(f), Some(Fetched::Failed(_)));
            ready &= loaded;
        };
        for message in t.messages.iter().rev().take(crate::session_pages::LATEST) {
            match message {
                Message::User { id, .. } => for file in user_files(id).iter() { check(file); },
                Message::Assistant(a) => for part in &a.parts {
                    if let Part::Tool { files, .. } = part { for file in files { check(file); } }
                },
                _ => {}
            }
        }
        ready
    }

    #[cfg(test)]
    pub(crate) fn image_handles(&self) -> Vec<Image> {
        self.images.borrow().values().map(|p| p.image.clone()).chain(self.refs.borrow().values().cloned()).collect()
    }

    pub(crate) fn adopt(&self, prepared: &[PreparedImage]) {
        let mut images = self.images.borrow_mut();
        // A refreshed page has new byte addresses. Release handles once neither a page nor
        // the transcript owns their source; repeated opens must not retain every old copy.
        images.retain(|_, p| Arc::strong_count(&p.bytes) > 1);
        for p in prepared {
            let mut p = p.clone();
            // A foreground refresh has its own decoder. Identical bytes already on screen
            // keep their ready pixels, instead of replacing them with a pending handle.
            if let Some(old) = images.values().find(|old| old.image.id() == p.image.id() && old.image.is_ready()) {
                p.image = old.image.clone();
            }
            images.insert((p.bytes.as_ptr() as usize, p.bytes.len()), p);
        }
    }

    fn thumbnail_size(&self, f: &Attachment) -> (f32, f32) {
        // Refs carry no dimensions: keep their reserved box after the fetch as well.
        if f.media.is_some() { return (160.0, 120.0); }
        self.images.borrow().get(&(f.bytes.as_ptr() as usize, f.bytes.len())).and_then(|p| p.size)
            .map_or((160.0, 120.0), |[w, h]| fit(w as f32 / 2.0, h as f32 / 2.0, 320.0, 240.0))
    }

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

    /// Where `media:` refs' bytes come from.
    pub fn set_fetch(&mut self, fetch: Fetch) {
        self.media.fetch = Some(fetch);
    }

    /// The decoded image (PNG and JPEG; None for other types, which show as a named chip, and
    /// for a ref whose bytes haven't arrived, which starts fetching them).
    pub fn image(&self, a: &Attachment) -> Option<Image> {
        if !decodable(&a.mime) {
            return None;
        }
        if let Some(hash) = &a.media {
            if let Some(img) = self.refs.borrow().get(hash) {
                return Some(img.clone());
            }
            let Fetched::Ready(bytes) = self.media.get(hash, self.waker.as_ref()) else { return None };
            let img = self.load(bytes);
            self.refs.borrow_mut().insert(hash.clone(), img.clone());
            self.media.state.lock().unwrap().insert(hash.clone(), Fetched::Decoded);
            return Some(img);
        }
        let key = (a.bytes.as_ptr() as usize, a.bytes.len());
        if let Some(p) = self.images.borrow().get(&key) {
            return Some(p.image.clone());
        }
        let img = self.load(a.bytes.clone());
        self.images.borrow_mut().insert(key, PreparedImage { bytes: a.bytes.clone(), image: img.clone(), size: dimensions(&a.bytes) });
        Some(img)
    }

    /// A ref's fetch, once `image` has asked for it (None for inline files).
    pub fn fetched(&self, a: &Attachment) -> Option<Fetched> {
        let hash = a.media.as_ref()?;
        self.media.state.lock().unwrap().get(hash).cloned()
    }

    fn load(&self, bytes: Arc<[u8]>) -> Image {
        let mut cache = self.cache.borrow_mut();
        cache.get_or_insert_with(|| ImageCache::new(self.waker.clone().unwrap_or_else(|| Arc::new(|| {})))).load(bytes)
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

impl App {
    /// Attaches a file from disk: inline when the model reads it natively and it fits, else by
    /// path, uploading a copy first when the server is on another machine.
    pub fn attach_path(&mut self, path: &Path) {
        let name = name_of(path);
        let size = match std::fs::metadata(path) {
            Ok(m) if m.is_file() => m.len() as usize,
            Ok(_) => return self.status = format!("{name} isn't a file"),
            Err(e) => return self.status = format!("{name}: {e}"),
        };
        if native_mime(path).is_some() && size <= MAX_BYTES {
            match from_path(path) {
                Ok(a) => self.attach.add(a),
                Err(e) => self.status = e,
            }
            return;
        }
        let mime = match head(path) {
            Ok(h) => mime_for(path, &h).to_owned(),
            Err(e) => return self.status = e,
        };
        let server = match &self.source {
            crate::Source::Live { server, .. } if !server.is_local() => server.clone(),
            _ => return self.attach.add_path(PathAttachment { name, mime, path: path.to_string_lossy().into_owned() }),
        };
        let (out, wake, directory, path) = (self.attach.uploads_tx.clone(), self.attach.waker.clone(), self.composer_directory(), path.to_owned());
        self.attach.uploading.push(name.clone());
        std::thread::spawn(move || {
            let copied = std::fs::read(&path).map_err(|e| format!("{name}: {e}")).and_then(|bytes| server.upload(&directory, &name, &bytes));
            let _ = out.send(copied.map(|path| PathAttachment { name, mime, path }));
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
                Err(e) => {
                    self.attach.uploading.pop();
                    self.status = e;
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
            Err(e) => s.status = e.to_string(),
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

pub fn drop_hover(s: &mut App, over: bool, tx: &mut Tx) {
    tx.instant();
    s.attach.drop_over = over;
}

pub fn drop_files(s: &mut App, paths: &[std::path::PathBuf], tx: &mut Tx) {
    tx.instant();
    s.attach.drop_over = false;
    for p in paths {
        s.attach_path(p);
    }
}

/// Desktop's drop overlay (`composer/dropzone.tsx`) over a `w`×`h` page while a file drag is
/// over it: a wash, faint diagonal stripes fading out from the centre, an upload badge, and the
/// label for a model that reads images and PDFs, as `attach_path` sends them. It appears and
/// leaves at once.
pub fn drop_overlay(s: &App, p: &Pal, (w, h): (f32, f32)) -> Option<El> {
    if !s.attach.drop_over || w <= 0.0 || h <= 0.0 {
        return None;
    }
    let (wash, stripe, card) = if p.dark { (0.06, 0.14, p.layer1) } else { (0.04, 0.07, p.bg_base) };
    let badge = el()
        .size(40.0, 40.0)
        .radius(20.0)
        .fill(card)
        .shadow(4.0, 12.0, p.shadow)
        .center()
        .child(icon(&ic::ARROW_UP, 20.0).color(p.icon_muted));
    Some(
        el().key("drop-overlay")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(p.text.alpha(wash))
            .child(el().absolute(0.0, 0.0).w_full().h_full().fill(p.bg_base.alpha(0.25)).shape(stripes(w, h, p.text.alpha(stripe * 0.6))))
            .center()
            .child(col().gap(20.0).center().child(badge).child(text("Drop images, PDFs, or files to add").font(DROP_LABEL).color(p.text))),
    )
}

const DROP_LABEL: TextStyle = TextStyle::new(15.0, 530.0).lh(24.0);

/// `repeating-linear-gradient(135deg, …12px, ink 12px 24px)` across the session column,
/// masked by Desktop's horizontal fade (12%–88%) and centred ellipse (59%×40%: full, 0.72 at
/// 58%, gone at the rim). Each stripe is cut into short quads that take the mask at their centre.
fn stripes(w: f32, h: f32, ink: Color) -> rune::Shape {
    const PERIOD: f32 = 24.0;
    const STEP: f32 = 12.0;
    let k = std::f32::consts::FRAC_1_SQRT_2;
    let cw = w.min(crate::view::CONTENT_W + 48.0);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let at = |t: f32, u: f32| [cx + (t + u) * k, cy + (t - u) * k];
    let mask = |[x, y]: [f32; 2]| {
        let fx = (x - (cx - cw / 2.0)) / cw;
        let side = (fx / 0.12).min((1.0 - fx) / 0.12).clamp(0.0, 1.0);
        let r = (((x - cx) / (0.59 * cw)).powi(2) + ((y - cy) / (0.4 * h)).powi(2)).sqrt();
        let radial = if r < 0.58 { 1.0 - 0.28 * r / 0.58 } else { (0.72 * (1.0 - r) / 0.42).max(0.0) };
        side * radial
    };
    let reach = (cw + h) * k / 2.0;
    let mut shape = rune::Shape::new();
    let mut t = -(reach / PERIOD).ceil() * PERIOD + PERIOD / 2.0;
    while t < reach {
        let mut u = -reach;
        while u < reach {
            let a = ink.a * mask(at(t + PERIOD / 4.0, u + STEP / 2.0));
            if a > 0.002 {
                let quad = vec![at(t, u), at(t + PERIOD / 2.0, u), at(t + PERIOD / 2.0, u + STEP), at(t, u + STEP)];
                shape = shape.fill(quad, ink.alpha(a));
            }
            u += STEP;
        }
        t += PERIOD;
    }
    shape
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
    if !s.attach.any() && s.attach.uploading.is_empty() {
        return None;
    }
    let mut r = row().key("attachments").w_full().px(8.0).pt(8.0).pb(4.0).gap(8.0).flex_wrap();
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
        let remove = el()
            .key("remove")
            .role("button")
            .label(format!("Remove {}", f.name))
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
                s.attach.files.retain(|f| identity(f) != gone);
            })
            .child(icon(&ic::XMARK_SMALL, 12.0).color(p.icon_contrast));
        r = r.child(el().key(format!("chip-{id}")).hover_group().child(body).child(remove));
    }
    for f in &s.attach.paths {
        let gone = f.path.clone();
        let remove = el()
            .key("remove")
            .role("button")
            .label(format!("Remove {}", f.name))
            .absolute(160.0 - 12.0, -4.0)
            .size(16.0, 16.0)
            .radius(8.0)
            .center()
            .fill(p.icon_muted)
            .outline(1.0, 0.0, p.icon_contrast)
            .group_opacity(0.0, 1.0)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.attach.paths.retain(|f| f.path != gone);
            })
            .child(icon(&ic::XMARK_SMALL, 12.0).color(p.icon_contrast));
        r = r.child(el().key(format!("path-{}", f.path)).hover_group().child(card(p, &f.name, type_label(&f.name))).child(remove));
    }
    for (i, name) in s.attach.uploading.iter().enumerate() {
        r = r.child(el().key(format!("uploading-{i}")).child(card(p, name, "Uploading…".into())));
    }
    Some(r)
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
        .child(text(title.to_owned()).font(CARD_TITLE).color(p.text).truncate().w_full())
        .child(text(subtitle).font(CARD_SUBTITLE).color(p.muted).truncate())
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

fn fit(w: f32, h: f32, max_w: f32, max_h: f32) -> (f32, f32) {
    let k = (max_w / w.max(1.0)).min(max_h / h.max(1.0)).min(1.0);
    ((w * k).max(24.0), (h * k).max(24.0))
}

// The viewer: Desktop's image preview (`@opencode/ui` `image-preview`). A click on a thumbnail
// grows it into the image, fitted to the window over a scrim; Escape or a click outside the
// image shrinks it back into its thumbnail.

pub struct Viewing {
    /// The thumbnail's `El::matched` id.
    id: String,
    name: String,
    image: Image,
}

/// Desktop flies on a 0.3 s spring launched with a push, as a spring from rest starts slowly;
/// from rest, 0.25 s lands on the same shape (the Timeline knob's default).
fn flight(s: &App) -> Spring {
    Spring::new(s.knobs.get(&crate::knobs::IMAGE_FLIGHT), 0.0)
}

/// One attachment's bytes (or, for a ref, the attachment itself), so each thumbnail has its own
/// shared-element id.
fn identity(f: &Attachment) -> String {
    match &f.media {
        Some(hash) => format!("{:x}:{hash}", f as *const Attachment as usize),
        None => format!("{:x}:{}", f.bytes.as_ptr() as usize, f.bytes.len()),
    }
}

/// A thumbnail that opens the viewer; while the viewer shows it, an empty box of its size (keyed
/// apart, so the thumbnail leaves the tree) holds its place, and the image flies out of it and
/// back into it.
fn opens(s: &App, thumb: El, held: El, id: String, f: &Attachment, image: Image) -> El {
    if s.attach.viewing.as_ref().is_some_and(|v| v.id == id) {
        return held;
    }
    let name = f.name.clone();
    thumb.matched(id.clone()).motion(flight(s)).on_press(move |s: &mut App| {
        s.attach.viewing = Some(Viewing { id: id.clone(), name: name.clone(), image: image.clone() });
    })
}

/// Opens the viewer on `f` in the gallery keyed `key`, as pressing its thumbnail does (the
/// storybook's images story). False while its image isn't loaded.
pub fn view_image(s: &mut App, key: &str, f: &Attachment) -> bool {
    let Some(image) = s.attach.image(f) else { return false };
    s.attach.viewing = Some(Viewing { id: format!("{key}:{}", identity(f)), name: f.name.clone(), image });
    true
}

/// Where the viewer shows an image of `size` pixels in a `w`×`h` window: a pixel a point, shrunk
/// to fit inside 56 points top and bottom and 3% of the width (20 to 48 points) at the sides,
/// centred.
fn shown(size: Option<[u32; 2]>, w: f32, h: f32) -> [f32; 4] {
    let side = (w * 0.03).clamp(8.0, 36.0) + 12.0;
    let (bw, bh) = ((w - 2.0 * side).max(1.0), (h - 2.0 * 56.0).max(1.0));
    let [iw, ih] = size.map_or([bw, bh], |[a, b]| [a.max(1) as f32, b.max(1) as f32]);
    let k = (bw / iw).min(bh / ih).min(1.0);
    let (sw, sh) = (iw * k, ih * k);
    [((w - sw) / 2.0).round(), ((h - sh) / 2.0).round(), sw, sh]
}

fn scrim(dark: bool) -> Color {
    static SCRIM: OnceLock<[Color; 2]> = OnceLock::new();
    let c = |dark| Theme::from_json(include_str!("../assets/theme.json"), dark).unwrap_or_else(|e| panic!("{e}")).color("overlay-simple-overlay-scrim");
    SCRIM.get_or_init(|| [c(false), c(true)])[dark as usize]
}

/// The open viewer, over the whole window: a scrim, and the image.
pub fn viewer(s: &App, p: &Pal, cx: &Cx<App>) -> Vec<El> {
    let Some(v) = &s.attach.viewing else { return Vec::new() };
    let win = cx.layout("window").unwrap_or([0.0, 0.0, 1280.0, 820.0]);
    let [x, y, w, h] = shown(v.image.size(), win[2], win[3]);
    let fade = SpringPair::new(Spring::new(0.15, 0.0), Spring::new(0.12, 0.0));
    let scrim = el().key("image-viewer-scrim").absolute(0.0, 0.0).w_full().h_full().fill(scrim(p.dark)).transition(Transition::opacity().springs(fade));
    let image = el()
        .key("image-viewer")
        .role("dialog")
        .label(v.name.clone())
        .matched(v.id.clone())
        .motion(flight(s))
        .size(w, h)
        .radius(6.0)
        .pixels_fit(v.image.pixels(), SurfaceFit::Fill)
        .transition(Transition::opacity().springs(fade))
        .modal(true, |s: &mut App| s.attach.viewing = None)
        .absolute(x, y);
    vec![scrim, image]
}

#[cfg(test)]
mod tests;

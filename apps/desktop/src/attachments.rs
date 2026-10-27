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
mod tests {
    use super::*;

    fn user(id: &str, text: &str) -> Message {
        Message::User { id: id.into(), text: text.into() }
    }

    #[test]
    fn opening_does_not_fetch_images_from_older_history() {
        let mut attach = Attach::default();
        attach.set_fetch(Arc::new(|_| panic!("older image fetched before its row mounted")));
        let file = Attachment { mime:"image/png".into(), media:Some("b".repeat(64)), ..Default::default() };
        opencode_kit::model::set_user_files("older_open_image", vec![file]);
        let mut messages = vec![user("older_open_image", "Old screenshot")];
        messages.extend((0..crate::session_pages::LATEST).map(|i| user(&format!("newer_open_{i}"), "Recent text")));
        assert!(attach.transcript_ready(&Transcript { session:"history_open".into(), messages }));
        assert!(attach.image_handles().is_empty());
    }

    #[test]
    fn the_servers_copy_replaces_the_echo() {
        let mut t = Transcript { session: "s".into(), messages: vec![user("msg_1", "older"), user("msg_5", "hi")] };
        let mut echoes = vec![("s".to_owned(), "msg_5".to_owned(), "hi".to_owned())];
        let fresh = vec![user("msg_1", "older"), user("msg_5", "hi")];
        reconcile(&mut echoes, &mut t, &fresh);
        t.merge(fresh);
        assert!(echoes.is_empty());
        assert_eq!(t.messages.iter().map(Message::id).collect::<Vec<_>>(), ["msg_1", "msg_5"]);
    }

    #[test]
    fn an_echo_under_another_id_drops_when_the_servers_lands() {
        let mut t = Transcript { session: "s".into(), messages: vec![user("msg_1", "hi"), user("msg_9", "hi")] };
        let mut echoes = vec![("s".to_owned(), "msg_9".to_owned(), "hi".to_owned())];
        // An unrelated page first: the echo stays.
        reconcile(&mut echoes, &mut t, &[user("msg_1", "hi")]);
        assert_eq!(echoes.len(), 1);
        let fresh = vec![user("msg_1", "hi"), user("msg_7", "hi")];
        reconcile(&mut echoes, &mut t, &fresh);
        t.merge(fresh);
        assert!(echoes.is_empty());
        assert_eq!(t.messages.iter().map(Message::id).collect::<Vec<_>>(), ["msg_1", "msg_7"]);
    }

    #[test]
    fn a_live_send_shows_one_message_under_the_id_it_sends() {
        use opencode_kit::client::{Server, Write};
        let (w, writes) = std::sync::mpsc::channel();
        let mut app = App::empty();
        app.source = crate::Source::Live { server: Server::new("http://127.0.0.1:9"), requests: None, writes: Some(w), allow_send: true };
        app.route = crate::Route::Session("ses_a".into());
        app.composer = rune::TextEdit::new("look at this");
        app.attach.add(Attachment { mime: "image/png".into(), name: "a.png".into(), bytes: Arc::from(&b"ab"[..]), media: None });
        app.send(&mut Tx::default());
        let Ok(Write::Send { session, prompt }) = writes.try_recv() else { panic!("no prompt") };
        let id = prompt.id.clone().unwrap();
        assert_eq!((session.as_str(), prompt.text.as_str(), prompt.files.len()), ("ses_a", "look at this", 1));
        assert!(app.attach.files.is_empty());
        assert_eq!(opencode_kit::model::user_files(&id).len(), 1, "the echo shows the image");
        // The server's copy comes back under the same id: still one message.
        let t = app.transcripts.get_mut("ses_a").unwrap();
        let fresh = vec![user(&id, "look at this")];
        reconcile(&mut app.attach.echoes, t, &fresh);
        t.merge(fresh);
        assert_eq!(t.messages.len(), 1);
        assert!(app.attach.echoes.is_empty());
    }

    #[test]
    fn a_drop_anywhere_on_the_page_attaches_under_an_instant_overlay() {
        let mut app = App::empty();
        app.load_fixture(crate::FIXTURE);
        crate::open_scene(&mut app);
        app.finish_stream();
        app.zoom = 1.4;
        let mut rt = rune::Runtime::new(app, crate::view::app, (1280.0, 820.0), rune::Env::new(true, 1.0));
        rt.poll = Some(crate::poll);
        rt.frame(0.0);
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
        }
        let dir = std::env::temp_dir().join(format!("oc-desktop-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shot = dir.join("CleanShot 2026-10-26 at 8.00.42 PM@2x.png");
        std::fs::write(&shot, png(4, 4)).unwrap();
        let notes = dir.join("notes.txt");
        std::fs::write(&notes, "hello").unwrap();
        let panel = rect(&rt, "panel").unwrap();
        // Over the timeline, well above the composer.
        let at = (panel[0] + panel[2] / 2.0, panel[1] + panel[3] * 0.3);
        rt.drag_files(vec![shot.clone(), notes.clone()], Some(at));
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.drop_hovered().as_deref(), Some("panel"));
        let overlay = rect(&rt, "drop-overlay").expect("the overlay shows");
        assert!(overlay.iter().zip(panel).all(|(a, b)| (a - b).abs() < 0.5), "it covers the page: {overlay:?} vs {panel:?}");
        assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Drop images, PDFs, or files to add")), "Desktop's label");
        rt.drop_files(vec![shot.clone(), notes.clone()], Some(at));
        rt.frame(1.0 / 60.0);
        std::fs::remove_dir_all(&dir).ok();
        assert!(rect(&rt, "drop-overlay").is_none(), "it leaves at once");
        // The image travels inline; the text file reaches the model by path, as a file chip.
        assert_eq!(rt.state.attach.files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["CleanShot 2026-10-26 at 8.00.42 PM@2x.png"]);
        let paths: Vec<_> = rt.state.attach.paths.iter().map(|f| (f.name.as_str(), f.mime.as_str())).collect();
        assert_eq!(paths, [("notes.txt", "text/plain")]);
        assert!(rect(&rt, &format!("path-{}", notes.display())).is_some(), "the file shows as a chip");
    }

    #[test]
    fn dropped_images_become_data_urls() {
        let dir = std::env::temp_dir().join(format!("oc-desktop-attach-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Shot.PNG");
        std::fs::write(&path, b"ab").unwrap();
        let a = from_path(&path).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!((a.mime.as_str(), a.name.as_str()), ("image/png", "Shot.PNG"));
        assert_eq!(inputs(&[a]), [FileInput { uri: "data:image/png;base64,YWI=".into(), name: "Shot.PNG".into(), ..Default::default() }]);
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(&vec![200; (w * h * 4) as usize]).unwrap();
        out
    }

    fn rect(rt: &rune::Runtime<App>, path: &str) -> Option<[f32; 4]> {
        rt.elements().into_iter().find(|e| e.path.ends_with(path) && !e.exiting).map(|e| e.rect)
    }

    /// Every edge heads one way from `from` to `to`, never more than a fifth of the way in one
    /// frame, and lands.
    fn flies(frames: &[[f32; 4]], from: [f32; 4], to: [f32; 4]) {
        let mut last = from;
        for (n, f) in frames.iter().enumerate() {
            for k in 0..4 {
                let (travel, step) = (to[k] - from[k], f[k] - last[k]);
                assert!(step * travel.signum() >= -0.01, "frame {n} edge {k} turns back: {last:?} -> {f:?}");
                assert!(step.abs() <= travel.abs() * 0.2 + 0.5, "frame {n} edge {k} jumps {step} of {travel}");
            }
            last = *f;
        }
        for k in 0..4 {
            assert!((last[k] - to[k]).abs() < 0.5, "lands: {last:?} vs {to:?}");
        }
    }

    #[test]
    fn a_thumbnail_grows_into_the_viewer_and_escape_shrinks_it_back() {
        use rune::{Env, Key, KeyEvent, Mods, Runtime};
        let mut app = App::empty();
        app.load_fixture(crate::FIXTURE);
        crate::open_scene(&mut app);
        app.finish_stream();
        let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
        rt.on_key = Some(crate::on_key);
        rt.poll = Some(crate::poll);
        rt.frame(0.0);
        let a = Attachment { mime: "image/png".into(), name: "shot.png".into(), bytes: png(1600, 1000).into(), media: None };
        let added = a.clone();
        rt.update(move |s, _| s.attach.add(added));
        rt.state.attach.image(&a).unwrap().wait().unwrap();
        for _ in 0..40 {
            rt.frame(1.0 / 60.0);
        }
        let chip = &format!("chip-{}/thumb", identity(&a));
        let thumb = rect(&rt, chip).unwrap();
        let (x, y) = rt.find(chip).unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        let mut open = Vec::new();
        for _ in 0..45 {
            rt.frame(1.0 / 60.0);
            open.push(rect(&rt, "/image-viewer").expect("the viewer is open"));
        }
        assert!(rect(&rt, chip).is_none(), "the thumbnail flew out of the chip");
        let window = rect(&rt, "/window").unwrap();
        let large = shown(Some([1600, 1000]), window[2], window[3]);
        assert!(large[2] > 1000.0, "fitted to the window: {large:?}");
        flies(&open, thumb, large);
        // 0.25 s: most of the way by 15 frames.
        assert!((open[14][2] - large[2]).abs() < (large[2] - thumb[2]) * 0.1, "still far at 0.25 s: {:?}", open[14]);
        assert!(rect(&rt, "/image-viewer-scrim").is_some());

        rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
        let mut close = Vec::new();
        for _ in 0..45 {
            rt.frame(1.0 / 60.0);
            close.push(rect(&rt, chip).expect("the thumbnail is back"));
        }
        assert!(rt.state.attach.viewing.is_none());
        assert!(rect(&rt, "/image-viewer").is_none());
        assert!(matches!(rt.state.route, crate::Route::Session(_)), "Escape only closes the viewer");
        flies(&close, large, thumb);
    }

    #[test]
    fn a_sent_image_opens_from_the_transcript_and_a_click_outside_closes_it() {
        use opencode_kit::client::Server;
        use rune::{Env, Runtime};
        let (w, _writes) = std::sync::mpsc::channel();
        let mut app = App::empty();
        app.source = crate::Source::Live { server: Server::new("http://127.0.0.1:9"), requests: None, writes: Some(w), allow_send: true };
        app.route = crate::Route::Session("ses_a".into());
        app.composer = rune::TextEdit::new("look");
        let a = Attachment { mime: "image/png".into(), name: "sent.png".into(), bytes: png(800, 400).into(), media: None };
        app.attach.add(a.clone());
        app.send(&mut Tx::default());
        let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
        rt.state.attach.image(&a).unwrap().wait().unwrap();
        for _ in 0..40 {
            rt.frame(1.0 / 60.0);
        }
        let thumb = rect(&rt, "/img-0").expect("the sent image shows in the transcript");
        let (x, y) = rt.find("img-0").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        let mut open = Vec::new();
        for _ in 0..45 {
            rt.frame(1.0 / 60.0);
            open.push(rect(&rt, "/image-viewer").unwrap());
        }
        let window = rect(&rt, "/window").unwrap();
        let large = shown(Some([800, 400]), window[2], window[3]);
        assert_eq!(large[2..], [800.0, 400.0], "a pixel a point when it fits");
        flies(&open, thumb, large);
        // A click on the image keeps it open; one on the scrim closes it.
        rt.pointer_down(window[2] / 2.0, window[3] / 2.0);
        rt.pointer_up(window[2] / 2.0, window[3] / 2.0);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.attach.viewing.is_some());
        rt.pointer_down(10.0, 10.0);
        rt.pointer_up(10.0, 10.0);
        let mut close = Vec::new();
        for _ in 0..45 {
            rt.frame(1.0 / 60.0);
            close.push(rect(&rt, "/img-0").unwrap());
        }
        assert!(rt.state.attach.viewing.is_none());
        flies(&close, large, thumb);
    }

    fn media(hash: &str, name: &str) -> Attachment {
        Attachment { mime: "image/png".into(), name: name.into(), media: Some(hash.into()), ..Default::default() }
    }

    /// A fetcher that counts its calls and answers from `bytes` (Err when None).
    fn stub(bytes: Option<Vec<u8>>) -> (Fetch, Arc<Mutex<Vec<String>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let fetch: Fetch = Arc::new(move |hash: &str| {
            seen.lock().unwrap().push(hash.to_owned());
            bytes.clone().ok_or_else(|| format!("GET /api/experimental/media/{hash}: 404"))
        });
        (fetch, calls)
    }

    fn settle(a: &Attach, f: &Attachment) -> Fetched {
        for _ in 0..500 {
            match a.fetched(f) {
                Some(Fetched::Loading) | None => std::thread::sleep(std::time::Duration::from_millis(2)),
                Some(done) => return done,
            }
        }
        panic!("the fetch never finished")
    }

    #[test]
    fn a_ref_fetches_once_by_hash_and_decodes() {
        let (fetch, calls) = stub(Some(png(6, 4)));
        let mut a = Attach::default();
        a.set_fetch(fetch);
        let hash = "c".repeat(64);
        let (first, again) = (media(&hash, "shot.png"), media(&hash, "same.png"));
        assert!(a.image(&first).is_none(), "nothing until the bytes land");
        assert_eq!(a.fetched(&first), Some(Fetched::Loading));
        assert!(matches!(settle(&a, &first), Fetched::Ready(_)));
        assert_eq!(a.image(&first).unwrap().wait().unwrap(), [6, 4]);
        // The pixels are the cache's now; the same hash elsewhere is a hit.
        assert_eq!(a.fetched(&first), Some(Fetched::Decoded));
        assert_eq!(a.image(&again).unwrap().id(), a.image(&first).unwrap().id());
        assert_eq!(*calls.lock().unwrap(), [hash]);
        // An inline file never fetches.
        let inline = Attachment { mime: "image/png".into(), name: "inline.png".into(), bytes: png(2, 2).into(), media: None };
        assert_eq!(a.image(&inline).unwrap().wait().unwrap(), [2, 2]);
        assert_eq!((a.fetched(&inline), calls.lock().unwrap().len()), (None, 1));
    }

    #[test]
    fn a_failed_fetch_shows_the_name_and_isnt_retried_each_frame() {
        let (fetch, calls) = stub(None);
        let mut a = Attach::default();
        a.set_fetch(fetch);
        let f = media(&"d".repeat(64), "gone.png");
        assert!(a.image(&f).is_none());
        assert!(matches!(settle(&a, &f), Fetched::Failed(e) if e.contains("404")));
        assert!(a.image(&f).is_none());
        assert_eq!(calls.lock().unwrap().len(), 1);
        // Without a server (fixtures), a ref is a named chip at once.
        assert!(matches!(Attach::default().media.get("e", None), Fetched::Failed(_)));
    }

    const GALLERY: &str = "msg_gallery_test";

    fn gallery_view(s: &App, _: &mut Cx<App>) -> El {
        let p = crate::palette::pal(true);
        col().key("root").children(gallery(s, p, "images", &opencode_kit::model::user_files(GALLERY), true))
    }

    #[test]
    fn the_transcript_holds_a_box_for_a_ref_then_the_image_and_shows_the_unavailable_placeholder() {
        let (fetch, _) = stub(Some(png(400, 200)));
        let mut app = App::empty();
        app.attach.set_fetch(fetch);
        let hash = "f".repeat(64);
        let placeholder = Attachment {
            mime: "text/plain".into(),
            name: "gone.png".into(),
            bytes: Arc::from(&b"[media unavailable: gone.png (image/png)]"[..]),
            media: None,
        };
        opencode_kit::model::set_user_files(GALLERY, vec![media(&hash, "shot.png"), placeholder]);
        let mut rt = rune::Runtime::new(app, gallery_view, (800.0, 600.0), rune::Env::new(true, 1.0));
        rt.frame(0.0);
        assert_eq!(rect(&rt, "img-0-loading").map(|r| [r[2], r[3]]), Some([160.0, 120.0]));
        let unavailable = rt.elements().into_iter().filter_map(|e| e.text).find(|t| t.starts_with("[media unavailable"));
        assert_eq!(unavailable.as_deref(), Some("[media unavailable: gone.png (image/png)]"));
        let first = media(&hash, "shot.png");
        settle(&rt.state.attach, &first);
        // The worker's wake re-runs the view.
        rt.update(|_, _| {});
        rt.frame(1.0 / 60.0);
        rt.state.attach.image(&first).unwrap().wait().unwrap();
        rt.update(|_, _| {});
        for _ in 0..60 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rect(&rt, "img-0-loading").is_none());
        assert_eq!(rect(&rt, "/img-0").map(|r| [r[2], r[3]]), Some([160.0, 120.0]), "a ref keeps its reserved box");
        opencode_kit::model::set_user_files(GALLERY, Vec::new());
    }

    #[test]
    fn a_ref_withdrawn_from_the_queue_goes_back_as_its_uri() {
        use opencode_kit::client::{Server, Write};
        use opencode_kit::model::Delivery;
        let (w, writes) = std::sync::mpsc::channel();
        let mut app = App::empty();
        app.source = crate::Source::Live { server: Server::new("http://127.0.0.1:9"), requests: None, writes: Some(w), allow_send: true };
        app.route = crate::Route::Session("ses_q".into());
        let hash = "a1".repeat(32);
        // As an opted-in `session.inbox.list` returns a queued prompt with a ref.
        let listed = serde_json::json!({"sessionID": "ses_q", "items": [{"id": "msg_queued", "type": "user", "delivery": "queue",
            "payload": {"text": "again", "files": [{"uri": format!("media:{hash}"), "mime": "image/png", "name": "again.png", "bytes": 3}]}}]});
        app.apply_inbox("session.inbox.listed", &listed, "ses_q");
        app.undo_queued("ses_q", "msg_queued");
        let Ok(Write::Cancel { item, .. }) = writes.try_recv() else { panic!("no cancel") };
        assert_eq!(item, "msg_queued");
        assert_eq!(app.attach.files, [media(&hash, "again.png")]);
        app.composer = rune::TextEdit::new("again");
        app.send(&mut Tx::default());
        let Ok(Write::Send { prompt, .. }) = writes.try_recv() else { panic!("no prompt") };
        assert_eq!(
            prompt.files,
            [FileInput { uri: format!("media:{hash}"), name: "again.png".into(), mime: Some("image/png".into()), mention: None }]
        );
        // Queued again, it keeps the ref.
        app.attach.add(media(&hash, "again.png"));
        app.admit("ses_q", opencode_kit::client::Prompt { files: inputs(&app.attach.files), ..Default::default() }, Delivery::Queue);
        let Ok(Write::Send { prompt, .. }) = writes.try_recv() else { panic!("no queued prompt") };
        assert_eq!((prompt.files[0].uri.as_str(), prompt.delivery), (format!("media:{hash}").as_str(), Some(Delivery::Queue)));
    }

    #[test]
    fn fits_in_the_box() {
        assert_eq!(fit(1280.0, 800.0, 320.0, 240.0), (320.0, 200.0));
        assert_eq!(fit(100.0, 50.0, 320.0, 240.0), (100.0, 50.0));
    }

    #[test]
    fn prefetched_pixels_and_header_dimensions_survive_the_first_paint() {
        use opencode_kit::model::{Assistant, Part, ToolStatus};
        let f = Attachment { mime: "image/png".into(), name: "wide.png".into(), bytes: png(800, 200).into(), media: None };
        let messages = vec![Message::Assistant(Assistant { parts: vec![Part::Tool {
            id: "shot".into(), name: "read".into(), title: "image".into(), status: ToolStatus::Completed,
            input: serde_json::Value::Null, metadata: serde_json::Value::Null, files: vec![f.clone()],
        }], ..Default::default() })];
        let (wake, woke) = channel();
        let cache = ImageCache::new(Arc::new(move || { let _ = wake.send(()); }));
        let prepared = std::thread::spawn(move || prepare_images(&messages, &cache)).join().unwrap();
        assert_eq!(prepared.len(), 1);
        let attach = Attach::default();
        attach.adopt(&prepared);
        let before = attach.thumbnail_size(&f);
        assert_eq!(before, (320.0, 80.0));
        prepared[0].wait();
        woke.recv_timeout(std::time::Duration::from_secs(5)).expect("prefetched decode wakes without input");
        assert_eq!(attach.image(&f).unwrap().id(), prepared[0].image.id());
        assert!(attach.image(&f).unwrap().is_ready(), "the click adopts ready pixels rather than starting another decode");
        assert_eq!(attach.thumbnail_size(&f), before, "no size change when pixels land");
    }

    #[test]
    fn files_classify_as_desktop_does() {
        let p = Path::new;
        assert_eq!(mime_for(p("a.PNG"), b""), "image/png");
        assert_eq!(mime_for(p("a.pdf"), b"%PDF"), "application/pdf");
        assert_eq!(mime_for(p("Makefile"), b"all:\n\tcargo build\n"), "text/plain");
        assert_eq!(mime_for(p("a.bin"), b"ab\0cd"), "application/octet-stream");
        assert_eq!(mime_for(p("a.dat"), &[1, 2, 3, b'a']), "application/octet-stream");
        assert_eq!(mime_for(p("empty"), b""), "text/plain");
        let r = |path: &str| PathAttachment { name: String::new(), mime: String::new(), path: path.into() };
        assert_eq!(path_lines("  ", &[r("/a"), r("/b")]), "Attached file: `/a`\nAttached file: `/b`");
        assert_eq!(path_lines("hi", &[]), "hi");
    }
}

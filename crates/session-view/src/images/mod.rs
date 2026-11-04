//! Media on attachments: `media:` ref fetches on a worker thread, decoded handles and the
//! prepared-image cache behind a transcript's thumbnails.

pub mod viewer;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, Weak};

use rune::{El, Image, ImageCache, SurfaceFit, TextStyle, Waker, col, el, row, text};

use opencode_kit::model::{Attachment, Message, Transcript};

use crate::TimelineHost;
use crate::palette::Pal;

pub use viewer::{Viewing, opens, shown, view_image, viewer};

/// Newest message window checked by `transcript_ready` (matches `session_pages::LATEST`).
pub const LATEST: usize = 12;

const SMALL: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);

/// Decoded image cache, `media:` ref fetcher, and active image lightbox state.
#[derive(Default)]
pub struct MediaStore {
    /// The image open in the viewer.
    pub viewing: Option<Viewing>,
    pub waker: Option<Waker>,
    cache: RefCell<Option<ImageCache>>,
    /// Decoded handles by the bytes' address, so a view doesn't re-hash megabytes each frame.
    images: RefCell<HashMap<(usize, usize), PreparedImage>>,
    /// `media:<hash>` refs: fetched once, decoded once, by hash.
    pub media: Media,
    refs: RefCell<HashMap<String, Image>>,
}

/// Header dimensions are available before decode; the same handle crosses from prefetch to view.
#[derive(Clone)]
pub struct PreparedImage {
    bytes: Arc<[u8]>,
    pub image: Image,
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

    pub fn handle(&self) -> Image { self.image.clone() }

    pub fn weight(&self) -> usize {
        // RGBA plus the mip chain; encoded bytes are counted by the page cache.
        self.size.map_or(0, |[w, h]| w as usize * h as usize * 16 / 3)
    }

    pub fn wait(&self) { let _ = self.image.wait(); }
}

/// Called only on the page worker. Newest attachments first, at most eight megapixels per
/// page; older history stays demand-loaded. The page cache also counts decoded pixel weight.
pub fn prepare_images(messages: &[Message], cache: &ImageCache) -> Vec<PreparedImage> {
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

/// How many decodable images a page carries inline, and their encoded bytes.
pub fn inline_images(messages: &[Message]) -> (usize, usize) {
    use opencode_kit::model::{Part, user_files};
    let (mut count, mut bytes) = (0, 0);
    let mut add = |f: &Attachment| if decodable(&f.mime) && f.media.is_none() { count += 1; bytes += f.bytes.len(); };
    for m in messages {
        match m {
            Message::User { id, .. } => user_files(id).iter().for_each(&mut add),
            Message::Assistant(a) => for p in &a.parts { if let Part::Tool { files, .. } = p { files.iter().for_each(&mut add); } },
            _ => {}
        }
    }
    (count, bytes)
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
pub struct Media {
    fetch: Option<Fetch>,
    state: Arc<Mutex<HashMap<String, Fetched>>>,
    queue: RefCell<Option<Arc<(Mutex<Vec<String>>, Condvar)>>>,
}

impl Media {
    pub fn get(&self, hash: &str, waker: Option<&Waker>) -> Fetched {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
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
        q.0.lock().unwrap_or_else(|e| e.into_inner()).push(hash.to_owned());
        q.1.notify_one();
        Fetched::Loading
    }
}

fn spawn_fetcher(queue: Weak<(Mutex<Vec<String>>, Condvar)>, state: Arc<Mutex<HashMap<String, Fetched>>>, fetch: Fetch, wake: Waker) {
    if let Err(e) = std::thread::Builder::new()
        .name("opencode-media".into())
        .spawn(move || {
            loop {
                // Ends once the app (the queue's owner) is gone.
                let Some(q) = queue.upgrade() else { return };
                let next = {
                    let mut stack = q.0.lock().unwrap_or_else(|e| e.into_inner());
                    if stack.is_empty() {
                        stack = match q.1.wait_timeout(stack, std::time::Duration::from_secs(1)) {
                            Ok((g, _)) => g,
                            Err(e) => e.into_inner().0,
                        };
                    }
                    stack.pop()
                };
                drop(q);
                let Some(hash) = next else { continue };
                let result = match fetch(&hash) {
                    Ok(bytes) => Fetched::Ready(bytes.into()),
                    Err(e) => Fetched::Failed(e),
                };
                state.lock().unwrap_or_else(|e| e.into_inner()).insert(hash, result);
                wake();
            }
        })
    {
        panic!("spawn media thread: {e}");
    }
}

/// What the cache decodes; other types show as a named chip.
pub fn decodable(mime: &str) -> bool {
    matches!(mime, "image/png" | "image/jpeg" | "image/jpg")
}

impl MediaStore {
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    /// A story controls byte arrival with the replay clock, then uses the real decoder.
    pub fn fixture_media(&self, hash: &str, bytes: Option<Vec<u8>>) {
        let ready = bytes.is_some();
        self.media.state.lock().unwrap_or_else(|e| e.into_inner()).insert(hash.into(), bytes.map_or(Fetched::Loading, |b| Fetched::Ready(b.into())));
        if ready {
            let f = Attachment { mime: "image/png".into(), media: Some(hash.into()), ..Default::default() };
            if let Some(img) = self.image(&f) {
                let _ = img.wait();
            }
        }
    }

    /// Start uncached/ref reads in the newest message window too. Older history stays
    /// demand-loaded; preparing a page is not the same as finishing its image decodes.
    pub fn transcript_ready(&self, t: &Transcript) -> bool {
        use opencode_kit::model::{Part, user_files};
        let mut ready = true;
        let mut check = |f: &Attachment| {
            if !decodable(&f.mime) { return; }
            let loaded = self.image(f).is_some_and(|i| i.is_ready())
                || matches!(self.fetched(f), Some(Fetched::Failed(_)));
            ready &= loaded;
        };
        for message in t.messages.iter().rev().take(LATEST) {
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

    pub fn image_handles(&self) -> Vec<Image> {
        self.images.borrow().values().map(|p| p.image.clone()).chain(self.refs.borrow().values().cloned()).collect()
    }

    pub fn adopt(&self, prepared: &[PreparedImage]) {
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

    pub fn thumbnail_size(&self, f: &Attachment) -> (f32, f32) {
        // Refs carry no dimensions: keep their reserved box after the fetch as well.
        if f.media.is_some() { return (160.0, 120.0); }
        self.images.borrow().get(&(f.bytes.as_ptr() as usize, f.bytes.len())).and_then(|p| p.size)
            .map_or((160.0, 120.0), |[w, h]| fit(w as f32 / 2.0, h as f32 / 2.0, 320.0, 240.0))
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
            self.media.state.lock().unwrap_or_else(|e| e.into_inner()).insert(hash.clone(), Fetched::Decoded);
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
        self.media.state.lock().unwrap_or_else(|e| e.into_inner()).get(hash).cloned()
    }

    fn load(&self, bytes: Arc<[u8]>) -> Image {
        let mut cache = self.cache.borrow_mut();
        cache.get_or_insert_with(|| ImageCache::new(self.waker.clone().unwrap_or_else(|| Arc::new(|| {})))).load(bytes)
    }
}

pub fn fit(w: f32, h: f32, max_w: f32, max_h: f32) -> (f32, f32) {
    let k = (max_w / w.max(1.0)).min(max_h / h.max(1.0)).min(1.0);
    ((w * k).max(24.0), (h * k).max(24.0))
}

/// Whether `gallery` shows anything for `files`, without loading them.
pub fn shows(files: &[Attachment]) -> bool {
    files.iter().any(|f| f.is_image() || f.unavailable().is_some())
}

/// Images in the transcript, at most 320×240 points (2× pixels), right-aligned for a user. A
/// ref still on its way holds a box; one whose bytes are gone shows the server's placeholder.
pub fn gallery<S: TimelineHost>(s: &S, p: &Pal, key: &str, files: &[Attachment], end: bool) -> Option<El> {
    let images: Vec<&Attachment> = files.iter().filter(|f| shows(std::slice::from_ref(f))).collect();
    if images.is_empty() {
        return None;
    }
    let mut r = row().key(key).w_full().gap(8.0).flex_wrap();
    if end {
        r = r.justify_end();
    }
    let store = s.media_store();
    for (i, f) in images.into_iter().enumerate() {
        let k = format!("img-{i}");
        r = r.child(match store.image(f) {
            Some(img) if img.error().is_none() => {
                let (w, h) = store.thumbnail_size(f);
                let held = el().key(format!("{k}-held")).size(w, h);
                let thumb = el().key(k).label(f.name.clone()).size(w, h).radius(8.0).fill(p.layer2).border(0.5, p.border_muted);
                opens(s, thumb.pixels_fit(img.pixels(), SurfaceFit::Fit), held, format!("{key}:{}", identity(f)), f, img)
            }
            None if store.fetched(f) == Some(Fetched::Loading) => el().key(format!("{k}-loading")).label(f.name.clone()).size(160.0, 120.0).radius(8.0).fill(p.layer2),
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
pub fn identity(f: &Attachment) -> String {
    match &f.media {
        Some(hash) => format!("{:x}:{hash}", std::ptr::from_ref(f) as usize),
        None => format!("{:x}:{}", f.bytes.as_ptr() as usize, f.bytes.len()),
    }
}

//! Media on attachments: `media:` ref fetches on a worker thread, decoded handles and the
//! prepared-image cache behind a transcript's thumbnails.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, Weak};

use rune::{Image, ImageCache, Waker};

use opencode_kit::model::{Attachment, Message, Transcript};

use super::Attach;

/// Header dimensions are available before decode; the same handle crosses from prefetch to view.
#[derive(Clone)]
pub(crate) struct PreparedImage {
    pub(super) bytes: Arc<[u8]>,
    pub(super) image: Image,
    pub(super) size: Option<[u32; 2]>,
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

/// How many decodable images a page carries inline, and their encoded bytes.
pub(crate) fn inline_images(messages: &[Message]) -> (usize, usize) {
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
pub(super) struct Media {
    fetch: Option<Fetch>,
    state: Arc<Mutex<HashMap<String, Fetched>>>,
    queue: RefCell<Option<Arc<(Mutex<Vec<String>>, Condvar)>>>,
}

impl Media {
    pub(super) fn get(&self, hash: &str, waker: Option<&Waker>) -> Fetched {
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
pub(super) fn decodable(mime: &str) -> bool {
    matches!(mime, "image/png" | "image/jpeg" | "image/jpg")
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

    pub(super) fn thumbnail_size(&self, f: &Attachment) -> (f32, f32) {
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

}

pub(super) fn fit(w: f32, h: f32, max_w: f32, max_h: f32) -> (f32, f32) {
    let k = (max_w / w.max(1.0)).min(max_h / h.max(1.0)).min(1.0);
    ((w * k).max(24.0), (h * k).max(24.0))
}

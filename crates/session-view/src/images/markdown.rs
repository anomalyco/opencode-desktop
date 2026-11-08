//! Pictures for markdown in replies, as Desktop reads them: an image's file through the server
//! (`readLocalImage`: a relative path in the session's directory, an absolute one in its own),
//! a web image and an external link's favicon (`markdown.tsx`'s `markExternalLinkFavicons`)
//! from the web. Each is fetched once on a worker thread and decoded by Rune's image cache.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

use rune::{Image, ImageCache, Pixels, Waker};

/// Reads a file through the server: `(directory, path)` to its bytes.
pub type FileFetch = Arc<dyn Fn(&str, &str) -> Result<Vec<u8>, String> + Send + Sync>;
/// Fetches a URL from the web.
pub type WebFetch = Arc<dyn Fn(&str) -> Result<Vec<u8>, String> + Send + Sync>;

/// What a source is fetched from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Source {
    File { directory: String, path: String },
    Web(String),
}

/// Where a source's picture is.
enum Slot {
    Fetching,
    /// The decoding picture and its encoded bytes, which a drag out writes to a file.
    Decoding(Image, Arc<[u8]>),
    Failed,
}

/// The pictures markdown in replies shows, shared by every reply's view (`Rc`, so a view's
/// closures can hold it).
#[derive(Default)]
pub struct MdStore {
    files: RefCell<Option<FileFetch>>,
    web: RefCell<Option<WebFetch>>,
    waker: RefCell<Option<Waker>>,
    cache: RefCell<Option<ImageCache>>,
    jobs: RefCell<Option<Sender<Source>>>,
    slots: RefCell<HashMap<Source, Slot>>,
    /// Bytes (or a failure) the worker fetched, waiting for the view to decode them.
    fetched: Arc<Mutex<Vec<(Source, Result<Vec<u8>, String>)>>>,
    /// A fetch or decode finished since the host last polled: the view must run again.
    landed: Arc<AtomicBool>,
    /// Counts fetches and decodes that finished, for the rows that draw pictures to rebuild.
    version: Arc<AtomicU64>,
}

/// Image extensions Desktop reads (`runtime/server/image.ts`).
const IMAGE_TYPES: [&str; 9] = ["png", "jpeg", "jpg", "gif", "webp", "svg", "avif", "bmp", "ico"];

impl MdStore {
    pub fn set_waker(&self, waker: Waker) {
        *self.waker.borrow_mut() = Some(waker);
    }

    /// Where files and web pictures come from (the live server, the web); without them every
    /// image shows its alt text and every link its globe.
    pub fn set_fetch(&self, files: FileFetch, web: WebFetch) {
        *self.files.borrow_mut() = Some(files);
        *self.web.borrow_mut() = Some(web);
    }

    /// Changes whenever a picture lands or decodes: a memoized row that draws pictures keys on it.
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    /// Whether a picture landed since the last call; the host's `poll` returns it.
    pub fn poll(&self) -> bool {
        self.landed.swap(false, Ordering::AcqRel)
    }

    /// The picture for markdown image `src` in a reply of a session in `directory`, once it is
    /// fetching or decoding; `None` when there is none to show (an unknown type, a failed read).
    pub fn image(&self, directory: Option<&str>, src: &str) -> Option<Image> {
        self.get(image_source(directory, src)?)
    }

    /// Markdown image `src`'s encoded bytes, once they've landed.
    pub fn bytes(&self, directory: Option<&str>, src: &str) -> Option<Arc<[u8]>> {
        match self.slots.borrow().get(&image_source(directory, src)?)? {
            Slot::Decoding(_, bytes) => Some(bytes.clone()),
            Slot::Fetching | Slot::Failed => None,
        }
    }

    /// The favicon to draw before a link to `url`, once it has decoded: Google's favicon
    /// service for a public host, as Desktop asks; `None` for GitHub (its mark), private and
    /// local hosts, and until it loads.
    pub fn favicon(&self, url: &str) -> Option<Pixels> {
        let host = favicon_host(url)?;
        let image = self.get(Source::Web(format!("https://www.google.com/s2/favicons?domain={}&sz=32", encode(&host))))?;
        (image.is_ready() && image.error().is_none() && image.size().is_some_and(|[w, _]| w > 0)).then(|| image.pixels().clone())
    }

    fn get(&self, source: Source) -> Option<Image> {
        self.land();
        if let Some(slot) = self.slots.borrow().get(&source) {
            return match slot {
                Slot::Decoding(image, _) => Some(image.clone()),
                Slot::Fetching | Slot::Failed => None,
            };
        }
        let jobs = self.jobs()?;
        self.slots.borrow_mut().insert(source.clone(), Slot::Fetching);
        let _ = jobs.send(source);
        None
    }

    /// Hands the worker's fetched bytes to the image cache.
    fn land(&self) {
        let fetched = std::mem::take(&mut *self.fetched.lock().unwrap_or_else(|e| e.into_inner()));
        for (source, bytes) in fetched {
            let slot = match bytes {
                Ok(bytes) => match self.cache() {
                    Some(cache) => {
                        let bytes: Arc<[u8]> = bytes.into();
                        Slot::Decoding(cache.load(bytes.clone()), bytes)
                    }
                    None => Slot::Failed,
                },
                Err(_) => Slot::Failed,
            };
            self.slots.borrow_mut().insert(source, slot);
        }
    }

    fn cache(&self) -> Option<std::cell::Ref<'_, ImageCache>> {
        if self.cache.borrow().is_none() {
            let waker = self.waker.borrow().clone()?;
            let (landed, version) = (self.landed.clone(), self.version.clone());
            *self.cache.borrow_mut() = Some(ImageCache::new(Arc::new(move || {
                version.fetch_add(1, Ordering::AcqRel);
                landed.store(true, Ordering::Release);
                waker();
            })));
        }
        std::cell::Ref::filter_map(self.cache.borrow(), Option::as_ref).ok()
    }

    /// The worker's queue, started on first use.
    fn jobs(&self) -> Option<Sender<Source>> {
        if let Some(jobs) = self.jobs.borrow().as_ref() {
            return Some(jobs.clone());
        }
        let (files, web, waker) = (self.files.borrow().clone()?, self.web.borrow().clone()?, self.waker.borrow().clone()?);
        let (send, receive) = channel::<Source>();
        let (fetched, landed, version) = (self.fetched.clone(), self.landed.clone(), self.version.clone());
        std::thread::Builder::new()
            .name("markdown-media".into())
            .spawn(move || {
                while let Ok(source) = receive.recv() {
                    let bytes = match &source {
                        Source::File { directory, path } => files(directory, path),
                        Source::Web(url) => web(url),
                    };
                    fetched.lock().unwrap_or_else(|e| e.into_inner()).push((source, bytes));
                    version.fetch_add(1, Ordering::AcqRel);
                    landed.store(true, Ordering::Release);
                    waker();
                }
            })
            .ok()?;
        *self.jobs.borrow_mut() = Some(send.clone());
        Some(send)
    }
}

/// What image `src` reads: a web URL as is; a path (or `file:` URL) through the server, in its
/// own directory when absolute, else in the session's (`localImagePath`, `readLocalImage`).
fn image_source(directory: Option<&str>, src: &str) -> Option<Source> {
    let value = src.trim().replace(['\\'], "/").replace("%5C", "/").replace("%5c", "/");
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(Source::Web(src.trim().to_owned()));
    }
    if value.is_empty() || value.starts_with("//") || value.chars().any(char::is_control) {
        return None;
    }
    let path = match lower.strip_prefix("file://") {
        Some(_) => {
            let rest = &value[7..];
            let (host, path) = rest.split_once('/').map_or((rest, ""), |(h, p)| (h, p));
            if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
                return None;
            }
            let path = format!("/{path}");
            // `file:///C:/x` names a drive.
            if path.len() > 3 && path.as_bytes()[2] == b':' { path[1..].to_owned() } else { path }
        }
        None => value,
    };
    let path = decode(&path)?;
    let drive = path.len() > 2 && path.as_bytes()[1] == b':' && path.as_bytes()[0].is_ascii_alphabetic() && matches!(path.as_bytes()[2], b'/' | b'\\');
    if !drive && path.split_once(':').is_some_and(|(scheme, _)| !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))) {
        return None;
    }
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    if !IMAGE_TYPES.contains(&ext.as_str()) {
        return None;
    }
    if drive || path.starts_with('/') {
        let (dir, name) = path.rsplit_once('/')?;
        let dir = if dir.is_empty() || dir.ends_with(':') { format!("{dir}/") } else { dir.to_owned() };
        return Some(Source::File { directory: dir, path: name.to_owned() });
    }
    Some(Source::File { directory: directory?.to_owned(), path })
}

/// The host to ask a favicon for: a public name, not GitHub's (it keeps its mark), nor a
/// reserved, local or private one (`publicFaviconHost`, `privateFaviconSuffix`).
fn favicon_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").filter(|(s, _)| s.eq_ignore_ascii_case("http") || s.eq_ignore_ascii_case("https"))?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = authority.split(':').next()?.to_ascii_lowercase();
    if host == "github.com" || host == "home.arpa" || host.ends_with(".home.arpa") {
        return None;
    }
    const PRIVATE: [&str; 9] = [".alt", ".example", ".internal", ".invalid", ".local", ".localhost", ".onion", ".test", ".ts.net"];
    if PRIVATE.iter().any(|s| host.ends_with(s)) {
        return None;
    }
    let labels: Vec<&str> = host.split('.').collect();
    let tld = labels.last()?;
    let public = labels.len() >= 2
        && (2..=63).contains(&tld.len())
        && tld.chars().all(|c| c.is_ascii_lowercase())
        && labels.iter().all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    // Desktop asks with the URL's host, port included.
    public.then(|| authority.to_ascii_lowercase())
}

fn encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let v = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?, 16).ok()?;
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok().filter(|p| !p.chars().any(char::is_control) && !p.starts_with("//"))
}

/// What a reply's markdown needs to show pictures: the store and the session's directory.
#[derive(Clone)]
pub struct MdMedia {
    pub store: Rc<MdStore>,
    pub directory: Option<Rc<str>>,
    /// The image the viewer shows now (its matched id), so its place in the reply holds empty
    /// while it flies.
    pub viewing: Option<String>,
    pub flight: rune::Spring,
    /// The message the markdown is, whose items follow an image's in its right-click menu.
    pub message: Option<Rc<str>>,
}

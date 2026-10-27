//! Project icons beside a project's name, for any Rune app on OpenCode: the server's image for
//! the project (Desktop's `icon.override` or `icon.url`, PNG, JPEG or SVG), else the app's own
//! icon found in its directory, else a quiet procedural mark seeded by the project's name.
//!
//! ```ignore
//! let icons = project_icon::Icons::new(waker); // keep it in app state
//! row().child(icons.view(&palette, &project, 16.0)).child(text(&project.name))
//! ```
//!
//! Finding and decoding run on one worker thread, once per directory and source file's mtime,
//! and are checked again every two minutes. Repo icons are shrunk to 64 px in linear light with
//! premultiplied alpha, so 16 pt at 2× uses an exact mip; SVGs and marks are drawn at the size
//! they show at.

mod find;
mod funky;
mod pixels;
mod styles;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use rune::{Color, El, Image, ImageCache, SurfaceFit, TextStyle, Waker, el, text};

use find::{Data, Key};
use pixels::Buf;
pub use styles::{Look, Style};

/// The theme colours icons are built from.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    /// Background and border of Desktop's avatar colours, in its order: orange, yellow, cyan,
    /// green, red, pink, blue, purple, gray (`avatar-bg-*`, `avatar-border-*`).
    pub avatars: [(Color, Color); 9],
    /// Letters and ink on an avatar colour (`avatar-fg`).
    pub fg: Color,
    /// The neutral tile behind a transparent logo.
    pub tile: Color,
    /// The hairline around an image.
    pub ring: Color,
}

/// What an icon is looked up by.
#[derive(Clone, Copy, Debug, Default)]
pub struct Project<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub directory: &'a str,
    /// The server's image for it, as Desktop picks it (`ProjectIcon::source`).
    pub image: Option<&'a str>,
    /// A chosen avatar colour (`orange`, …), or empty.
    pub color: &'a str,
}

impl<'a> Project<'a> {
    /// Just a name: no image is looked up, so it always gets a mark.
    pub fn named(name: &'a str) -> Project<'a> {
        Project { name, ..Project::default() }
    }
}

impl<'a> From<&'a opencode_kit::model::Project> for Project<'a> {
    fn from(p: &'a opencode_kit::model::Project) -> Project<'a> {
        Project { id: &p.id, name: &p.name, directory: &p.directory, image: p.icon.source(), color: &p.icon.color }
    }
}

/// How long a found icon (or its absence) stands before the directory is looked at again.
const RECHECK: Duration = Duration::from_secs(120);
/// Repo icons' pixels per side: 16 pt at 2× is the 32 px mip; the full level covers 32 pt.
const PX: u32 = 64;

#[derive(Default)]
pub struct Icons {
    waker: Option<Waker>,
    sync: bool,
    scale: Cell<f32>,
    look: Cell<Look>,
    inner: RefCell<Option<Inner>>,
    marks: RefCell<HashMap<(String, bool, usize, u32, (Style, i32, i32)), Image>>,
    vectors: RefCell<HashMap<(u64, u32), Image>>,
}

struct Inner {
    cache: Arc<ImageCache>,
    jobs: Sender<Job>,
    found: Arc<Mutex<HashMap<String, Found>>>,
}

#[derive(Default)]
struct Found {
    checked: Option<Instant>,
    pending: bool,
    /// The server URL this was resolved with (its length and ends).
    server: (usize, u64),
    key: Option<Key>,
    icon: Option<Shown>,
}

#[derive(Clone)]
enum Shown {
    /// A full tile.
    Tile(Image),
    /// A transparent mark (a logo), drawn small on a neutral tile.
    Mark(Image),
    /// An SVG, rasterized at each size it shows at, over the avatar colour as Desktop draws it.
    Vector(Arc<[u8]>, u64),
}

struct Job {
    dir: String,
    name: String,
    server: Option<String>,
    previous: Option<Key>,
}

impl Icons {
    pub fn new(waker: Waker) -> Icons {
        let mut icons = Icons::default();
        icons.set_waker(waker);
        icons
    }

    /// Call from `AppSpec::setup`, before the first view.
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    /// Resolve and decode in the view, so a headless snapshot's first frame shows icons.
    pub fn set_sync(&mut self, sync: bool) {
        self.sync = sync;
    }

    /// Device pixels per point that marks and SVGs rasterize for (default 2).
    pub fn set_scale(&self, scale: f32) {
        self.scale.set(scale);
    }

    /// The mark for projects without an image (default: Gradient).
    pub fn set_look(&self, look: Look) {
        self.look.set(look);
    }

    pub fn look(&self) -> Look {
        self.look.get()
    }

    fn scale(&self) -> f32 {
        let s = self.scale.get();
        if s > 0.0 { s } else { 2.0 }
    }

    fn with<R>(&self, f: impl FnOnce(&Inner) -> R) -> R {
        let mut inner = self.inner.borrow_mut();
        let inner = inner.get_or_insert_with(|| {
            let wake = self.waker.clone().unwrap_or_else(|| Arc::new(|| {}));
            let cache = Arc::new(ImageCache::new(wake.clone()));
            let found: Arc<Mutex<HashMap<String, Found>>> = Default::default();
            let (jobs, rx) = channel::<Job>();
            let (c, fd) = (cache.clone(), found.clone());
            std::thread::Builder::new()
                .name("project-icons".into())
                .spawn(move || {
                    for job in rx {
                        let had = fd.lock().unwrap().get(&job.dir).is_some_and(|f| f.icon.is_some());
                        let done = run(&c, &job);
                        let wake_now = matches!(&done, Some((_, Some(Shown::Vector(..))))) || (done.is_none() && had);
                        settle(&fd, &job.dir, done);
                        if wake_now {
                            wake();
                        }
                    }
                })
                .expect("project icon worker");
            Inner { cache, jobs, found }
        });
        f(inner)
    }

    /// The project's real icon once it's found and decoded.
    fn found(&self, p: &Project) -> Option<Shown> {
        if p.directory.is_empty() && p.image.is_none() {
            return None;
        }
        let server = p.image.filter(|u| find::is_data_image(u));
        let stamp = server.map_or((0, 0), stamp);
        let dir = if p.directory.is_empty() { format!("id:{}", p.id) } else { p.directory.to_owned() };
        self.with(|inner| {
            let job = {
                let mut found = inner.found.lock().unwrap();
                let f = found.entry(dir.clone()).or_default();
                let stale = f.checked.is_none_or(|t| t.elapsed() > RECHECK) || f.server != stamp;
                (stale && !f.pending).then(|| {
                    f.pending = true;
                    f.server = stamp;
                    Job { dir: dir.clone(), name: p.name.to_owned(), server: server.map(str::to_owned), previous: f.key.clone() }
                })
            };
            if let Some(job) = job {
                if self.sync {
                    let done = run(&inner.cache, &job);
                    if let Some((_, Some(Shown::Tile(i) | Shown::Mark(i)))) = &done {
                        let _ = i.wait();
                    }
                    settle(&inner.found, &dir, done);
                } else {
                    let _ = inner.jobs.send(job);
                }
            }
            let found = inner.found.lock().unwrap();
            found.get(&dir)?.icon.clone().filter(|s| match s {
                Shown::Tile(i) | Shown::Mark(i) => i.is_ready() && i.error().is_none(),
                Shown::Vector(..) => true,
            })
        })
    }

    fn vector(&self, bytes: &Arc<[u8]>, hash: u64, size_pt: f32) -> Image {
        let px = (size_pt * self.scale()).ceil() as u32;
        if let Some(img) = self.vectors.borrow().get(&(hash, px)) {
            return img.clone();
        }
        let img = self.with(|inner| inner.cache.svg(bytes.clone(), size_pt, self.scale()));
        if self.sync {
            let _ = img.wait();
        }
        self.vectors.borrow_mut().insert((hash, px), img.clone());
        img
    }

    /// The project's icon at `size_pt` square: its image, else a mark in the current look.
    pub fn view<'a>(&self, pal: &Palette, project: impl Into<Project<'a>>, size_pt: f32) -> El {
        let project = project.into();
        self.image(pal, project, size_pt).unwrap_or_else(|| self.mark(pal, project, size_pt, self.look()))
    }

    /// The project's real icon at `size_pt` square, or None while there's none (no mark).
    pub fn image<'a>(&self, pal: &Palette, project: impl Into<Project<'a>>, size_pt: f32) -> Option<El> {
        let project = project.into();
        let radius = size_pt * styles::RADIUS;
        let variant = chosen_color(project.color);
        Some(match self.found(&project)? {
            Shown::Tile(img) => {
                let ring = el().key("ring").absolute(0.0, 0.0).size(size_pt, size_pt).radius(radius).border(0.5, pal.ring);
                el().size(size_pt, size_pt).radius(radius).pixels_fit(img.pixels(), SurfaceFit::Fill).child(ring)
            }
            Shown::Mark(img) => {
                let ring = el().key("ring").absolute(0.0, 0.0).size(size_pt, size_pt).radius(radius).border(0.5, pal.ring);
                let inner = el().size(size_pt * 0.75, size_pt * 0.75).pixels_fit(img.pixels(), SurfaceFit::Fit);
                el().size(size_pt, size_pt).radius(radius).fill(pal.tile).center().child(inner).child(ring)
            }
            Shown::Vector(bytes, hash) => {
                let img = self.vector(&bytes, hash, size_pt);
                let (bg, border) = pal.avatars[variant.unwrap_or(8)];
                let pic = el().absolute(0.0, 0.0).size(size_pt, size_pt).radius(radius).pixels_fit(img.pixels(), SurfaceFit::Fill);
                let ring = el().key("ring").absolute(0.0, 0.0).size(size_pt, size_pt).radius(radius).border(0.5, border);
                el().size(size_pt, size_pt).radius(radius).fill(bg).child(pic).child(ring)
            }
        })
    }

    /// The procedural mark alone, in `look`.
    pub fn mark<'a>(&self, pal: &Palette, project: impl Into<Project<'a>>, size_pt: f32, look: Look) -> El {
        let project = project.into();
        let seed = if project.name.is_empty() { project.id } else { project.name };
        let chosen = chosen_color(project.color);
        // Desktop shows gray unless a colour is chosen; the others pick one from the name.
        let variant = match look.style {
            Style::Desktop => chosen.unwrap_or(8),
            _ => chosen.unwrap_or((styles::hash(seed) % 8) as usize),
        };
        let px = (size_pt * self.scale()).ceil().clamp(1.0, 1024.0) as u32;
        let key = (seed.to_owned(), pal.dark, variant, px, look.key());
        let cached = self.marks.borrow().get(&key).cloned();
        let img = cached.unwrap_or_else(|| {
            let rgba = styles::draw(pal, seed, variant, &look, size_pt, px);
            let img = self.with(|inner| inner.cache.rgba(px, px, rgba));
            if self.sync {
                let _ = img.wait();
            }
            self.marks.borrow_mut().insert(key, img.clone());
            img
        });
        let radius = size_pt * styles::RADIUS;
        let mut tile = if img.is_ready() {
            el().size(size_pt, size_pt).radius(radius).pixels_fit(img.pixels(), SurfaceFit::Fill)
        } else {
            let (bg, border) = pal.avatars[variant];
            el().size(size_pt, size_pt).radius(radius).fill(bg).border(0.5, border)
        };
        if look.style.lettered() {
            let letter = seed.chars().find(|c| c.is_alphanumeric()).map_or("?".into(), |c| c.to_uppercase().to_string());
            // Desktop's 11 px semibold letter in a 16 px tile.
            let font = TextStyle::new(size_pt * 11.0 / 16.0, 530.0).lh(size_pt);
            tile = tile.center().child(text(letter).font(font).color(pal.fg));
        }
        if look.style == Style::Pop {
            let letter = seed.chars().find(|c| c.is_alphanumeric()).map_or("?".into(), |c| c.to_uppercase().to_string());
            let font = TextStyle::new(size_pt * 0.74, 860.0).lh(size_pt);
            let (ink, shadow) = funky::pop_ink();
            let off = (size_pt * 0.07).max(1.0);
            let layer = |dx: f32, color: Color| {
                el().absolute(dx, dx).size(size_pt, size_pt).center().child(text(letter.clone()).font(font).color(color))
            };
            tile = tile.child(layer(off * 0.5, shadow)).child(layer(-off * 0.5, ink));
        }
        tile
    }
}

fn settle(found: &Mutex<HashMap<String, Found>>, dir: &str, done: Option<(Key, Option<Shown>)>) {
    let mut found = found.lock().unwrap();
    let f = found.entry(dir.to_owned()).or_default();
    f.pending = false;
    f.checked = Some(Instant::now());
    match done {
        // The same source as before: keep its pixels.
        Some((key, None)) if f.key.as_ref() == Some(&key) => {}
        Some((key, shown)) => {
            f.key = Some(key);
            f.icon = shown;
        }
        None => {
            f.key = None;
            f.icon = None;
        }
    }
}

/// Desktop's avatar colour names, as indices into `Palette::avatars`.
fn chosen_color(name: &str) -> Option<usize> {
    let name = match name {
        "mint" => "cyan",
        "lime" => "green",
        n => n,
    };
    ["orange", "yellow", "cyan", "green", "red", "pink", "blue", "purple", "gray"].iter().position(|n| *n == name)
}

fn stamp(url: &str) -> (usize, u64) {
    let b = url.as_bytes();
    let ends = [&b[..b.len().min(256)], &b[b.len().saturating_sub(256)..]].concat();
    (b.len(), styles::fnv(&ends))
}

/// Resolves a job: its source and, when that changed, the decoded icon. None when there is no
/// icon at all.
fn run(cache: &ImageCache, job: &Job) -> Option<(Key, Option<Shown>)> {
    if let Some(url) = &job.server {
        let key = Key(format!("server:{:?}", stamp(url)), SystemTime::UNIX_EPOCH);
        if job.previous.as_ref() == Some(&key) {
            return Some((key, None));
        }
        match find::data_url(url) {
            Some(Data::Svg(bytes)) => {
                let hash = styles::fnv(&bytes);
                return Some((key, Some(Shown::Vector(bytes.into(), hash))));
            }
            Some(Data::Raster(bytes)) => {
                if let Ok(icon) = image::load_from_memory(&bytes) {
                    return Some((key, Some(finish(cache, Buf::from_rgba8(&icon.into_rgba8()), true))));
                }
            }
            None => {}
        }
    }
    if job.dir.is_empty() || job.dir.starts_with("id:") {
        return None;
    }
    let mut cands = find::candidates(Path::new(&job.dir), &job.name);
    cands.sort_by_key(|c| c.rank());
    for c in cands {
        let Some(key) = c.key() else { continue };
        if job.previous.as_ref() == Some(&key) {
            return Some((key, None));
        }
        if let Some((buf, trim)) = find::decode(&c) {
            return Some((key, Some(finish(cache, buf, trim))));
        }
    }
    None
}

fn finish(cache: &ImageCache, buf: Buf, trim: bool) -> Shown {
    let buf = if trim { buf.trim() } else { buf };
    let mark = buf.coverage() < 0.6;
    let image = cache.rgba(PX, PX, buf.resize(PX, PX).to_rgba8());
    if mark { Shown::Mark(image) } else { Shown::Tile(image) }
}

#[cfg(test)]
mod tests;

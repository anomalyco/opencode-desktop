//! Project icons beside a project's name: the server's image for the project (Desktop's
//! `icon.override` or `icon.url`), else the app's own icon found in its directory, else a quiet
//! procedural mark seeded by the project.
//!
//! Finding and decoding run on one worker thread, once per directory and source file's mtime.
//! Each icon is shrunk to `PX` square in linear light with premultiplied alpha, and the image
//! cache's mips give the 2× level exactly, so 16 pt stays crisp.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use image::{ImageBuffer, Rgba, RgbaImage, imageops::FilterType};
use opencode_kit::model::Project;
use rune::{Color, El, Image, ImageCache, SurfaceFit, Waker, el};
use serde_json::Value;

use crate::palette::Pal;

/// The mark for projects without an image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    /// The first letter on the avatar colour (what Desktop shows).
    Letter,
    /// One small geometric shape on the avatar colour.
    Glyph,
    /// A soft diagonal blend of two neighbouring avatar colours.
    Gradient,
    /// A mirrored 3×3 pattern on the avatar colour.
    Quilt,
}

pub const STYLE: Style = Style::Glyph;

impl Style {
    pub const ALL: [Style; 4] = [Style::Letter, Style::Glyph, Style::Gradient, Style::Quilt];

    fn name(self) -> &'static str {
        match self {
            Style::Letter => "letter",
            Style::Glyph => "glyph",
            Style::Gradient => "gradient",
            Style::Quilt => "quilt",
        }
    }

    /// `STYLE`, or `--icon-style letter|glyph|gradient|quilt` for comparing them.
    fn current() -> Style {
        static S: std::sync::OnceLock<Style> = std::sync::OnceLock::new();
        *S.get_or_init(|| crate::arg("--icon-style").and_then(|n| Style::ALL.into_iter().find(|s| s.name() == n)).unwrap_or(STYLE))
    }
}

const PT: f32 = 16.0;
/// Pixels per side: 16 pt at 2× is the 32 px mip; the full level covers zoom and 3× screens.
const PX: u32 = 64;
/// How long a found icon (or its absence) stands before the directory is looked at again.
const RECHECK: Duration = Duration::from_secs(120);
const MAX_BYTES: u64 = 8 << 20;
const MAX_SIDE: u32 = 4096;
/// Directories the repo walk never enters.
const SKIP: &[&str] = &[
    "node_modules", "target", "dist", "out", "DerivedData", "Pods", "vendor", "third_party", "artifacts", "examples",
    "fixtures", "tests", "test", "coverage", "tmp",
];

/// Icon files at fixed places: Tauri, Electron and extension builds, then web icons.
fn fixed() -> Vec<(String, Kind)> {
    let mut v: Vec<(String, Kind)> = [
        "src-tauri/icons/icon.png", "src-tauri/icons/128x128@2x.png", "src-tauri/icons/icon.icns", "build/icon.png",
        "build/icon.icns", "resources/icon.png", "resources/icon.icns", "icons/icon.png", "extension/icons/icon.png",
        "assets/app-icon.png", "assets/icon.png", "icon.png",
    ]
    .iter()
    .map(|f| (f.to_string(), Kind::App))
    .collect();
    for d in ["", "public/", "static/", "site/", "site/public/", "site/static/", "assets/", "app/", "src/app/", "web/public/"] {
        v.push((format!("{d}apple-touch-icon.png"), Kind::Touch));
        for f in ["favicon.png", "icon.png", "icon.jpg"] {
            v.push((format!("{d}{f}"), Kind::Web));
        }
        for f in ["logo.png", "logo.jpg", "logo.jpeg"] {
            v.push((format!("{d}{f}"), Kind::Logo));
        }
    }
    v
}

#[derive(Default)]
pub struct Icons {
    waker: Option<Waker>,
    inner: RefCell<Option<Inner>>,
    marks: RefCell<HashMap<(String, Style, bool, usize), Image>>,
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
struct Shown {
    image: Image,
    /// A transparent mark (a logo) rather than a full tile: it sits on a neutral tile.
    mark: bool,
}

/// Where an icon came from and when that changed, so an unchanged file isn't decoded again.
#[derive(Clone, Debug, PartialEq)]
struct Key(String, SystemTime);

struct Job {
    dir: String,
    name: String,
    server: Option<String>,
    previous: Option<Key>,
}

impl Icons {
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
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
                        let gone = done.is_none() && had;
                        settle(&fd, &job.dir, done);
                        if gone {
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
    fn image(&self, p: &Project) -> Option<Shown> {
        if p.directory.is_empty() {
            return None;
        }
        let server = p.icon.source().filter(|u| raster_data_url(u));
        let stamp = server.map_or((0, 0), stamp);
        self.with(|inner| {
            let job = {
                let mut found = inner.found.lock().unwrap();
                let f = found.entry(p.directory.clone()).or_default();
                let stale = f.checked.is_none_or(|t| t.elapsed() > RECHECK) || f.server != stamp;
                (stale && !f.pending).then(|| {
                    f.pending = true;
                    f.server = stamp;
                    Job { dir: p.directory.clone(), name: p.name.clone(), server: server.map(str::to_owned), previous: f.key.clone() }
                })
            };
            if let Some(job) = job {
                if crate::headless() {
                    // Snapshots draw the first frame, so they resolve and decode right here.
                    let done = run(&inner.cache, &job);
                    if let Some((_, Some(shown))) = &done {
                        let _ = shown.image.wait();
                    }
                    settle(&inner.found, &p.directory, done);
                } else {
                    let _ = inner.jobs.send(job);
                }
            }
            let found = inner.found.lock().unwrap();
            found.get(&p.directory)?.icon.clone().filter(|s| s.image.is_ready() && s.image.error().is_none())
        })
    }

    /// A procedural mark, drawn once per seed, style and appearance.
    fn mark(&self, p: &Pal, seed: &str, style: Style, variant: usize) -> Image {
        let key = (seed.to_owned(), style, p.dark, variant);
        if let Some(img) = self.marks.borrow().get(&key) {
            return img.clone();
        }
        let rgba = draw_mark(p, seed, style, variant);
        let img = self.with(|inner| inner.cache.rgba(PX, PX, rgba));
        if crate::headless() {
            let _ = img.wait();
        }
        self.marks.borrow_mut().insert(key, img.clone());
        img
    }

    /// The 16 pt icon for a project (or, without one, for a name).
    pub fn badge(&self, p: &Pal, project: Option<&Project>, name: &str) -> El {
        if let Some(shown) = project.and_then(|x| self.image(x)) {
            let ring = el().key("ring").absolute(0.0, 0.0).size(PT, PT).radius(4.0).border(0.5, p.ring);
            if shown.mark {
                let inner = el().size(12.0, 12.0).pixels_fit(shown.image.pixels(), SurfaceFit::Fit);
                return el().size(PT, PT).radius(4.0).fill(p.layer3).center().child(inner).child(ring);
            }
            return el().size(PT, PT).radius(4.0).pixels_fit(shown.image.pixels(), SurfaceFit::Fill).child(ring);
        }
        let style = Style::current();
        let seed = project.map(|x| x.id.as_str()).filter(|x| !x.is_empty()).unwrap_or(name);
        let variant = project.and_then(|x| chosen_color(&x.icon.color)).unwrap_or_else(|| (hash(seed) % p.avatars.len() as u64) as usize);
        if style == Style::Letter {
            return crate::view::avatar(p, name);
        }
        let img = self.mark(p, seed, style, variant);
        let (bg, border) = p.avatars[variant];
        if !img.is_ready() {
            return el().size(PT, PT).radius(4.0).fill(bg).border(0.5, border);
        }
        el().size(PT, PT).radius(4.0).pixels_fit(img.pixels(), SurfaceFit::Fill)
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

/// Desktop's avatar colour names, as indices into `Pal::avatars`.
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
    (b.len(), fnv(&ends))
}

// ---------------------------------------------------------------------------------------------
// Finding

/// Resolves a job: its source and, when that changed, the decoded icon. None when there is no
/// icon at all.
fn run(cache: &ImageCache, job: &Job) -> Option<(Key, Option<Shown>)> {
    if let Some(url) = &job.server {
        let key = Key(format!("server:{:?}", stamp(url)), SystemTime::UNIX_EPOCH);
        if job.previous.as_ref() == Some(&key) {
            return Some((key, None));
        }
        if let Some(icon) = data_url_bytes(url).and_then(|b| image::load_from_memory(&b).ok()).map(|i| i.into_rgba8()) {
            return Some((key, Some(finish(cache, Buf::from_rgba8(&icon), true))));
        }
    }
    let mut cands = candidates(Path::new(&job.dir), &job.name);
    cands.sort_by_key(|c| c.rank());
    for c in cands {
        let Some(key) = c.key() else { continue };
        if job.previous.as_ref() == Some(&key) {
            return Some((key, None));
        }
        if let Some((buf, trim)) = decode(&c) {
            return Some((key, Some(finish(cache, buf, trim))));
        }
    }
    None
}

fn finish(cache: &ImageCache, buf: Buf, trim: bool) -> Shown {
    let buf = if trim { buf.trim() } else { buf };
    let mark = buf.coverage() < 0.6;
    let small = buf.resize(PX, PX);
    Shown { image: cache.rgba(PX, PX, small.to_rgba8()), mark }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    /// A macOS, Tauri or Electron app icon (`.icns`, `AppIcon.appiconset`, `build/icon.png`).
    App,
    /// An Icon Composer `.icon`, flattened here over its fill.
    Composer,
    Touch,
    Web,
    Logo,
}

#[derive(Debug)]
struct Cand {
    path: PathBuf,
    kind: Kind,
    side: u32,
    square: bool,
    depth: usize,
    named: bool,
    /// For a catalog or `.icon`: the image inside it to read.
    file: Option<PathBuf>,
}

impl Cand {
    fn rank(&self) -> (bool, bool, Kind, bool, bool, usize, PathBuf) {
        (!self.square, self.side < 64, self.kind, self.side < 128, !self.named, self.depth, self.path.clone())
    }

    fn key(&self) -> Option<Key> {
        let file = self.file.as_ref().unwrap_or(&self.path);
        let mut t = mtime(file)?;
        if self.kind == Kind::Composer {
            t = t.max(mtime(&self.path.join("icon.json"))?);
        }
        Some(Key(file.to_string_lossy().into_owned(), t))
    }
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

fn candidates(root: &Path, name: &str) -> Vec<Cand> {
    let mut out = Vec::new();
    let lname = name.to_lowercase();
    let depth_of = |p: &Path| p.strip_prefix(root).map_or(0, |r| r.components().count());
    for (rel, kind) in fixed() {
        let path = root.join(&rel);
        if let Some(c) = file_cand(&path, kind, depth_of(&path), &lname) {
            out.push(c);
        }
    }
    let mut visited = 0usize;
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            visited += 1;
            if visited > 4000 {
                return out;
            }
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with('.') {
                continue;
            }
            let path = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if n.ends_with(".appiconset") {
                    out.extend(catalog(&path, depth + 1, &lname));
                } else if n.ends_with(".icon") {
                    out.extend(composer(&path, depth + 1, &lname));
                } else if n.ends_with(".xcassets") || (depth < 3 && !SKIP.contains(&n.as_str())) {
                    stack.push((path, depth + 1));
                }
            } else if n.ends_with(".icns") {
                out.extend(file_cand(&path, Kind::App, depth + 1, &lname));
            }
        }
    }
    out
}

fn named(path: &Path, lname: &str) -> bool {
    !lname.is_empty() && path.to_string_lossy().to_lowercase().rsplit('/').take(3).any(|c| c.contains(lname))
}

fn file_cand(path: &Path, kind: Kind, depth: usize, lname: &str) -> Option<Cand> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_BYTES {
        return None;
    }
    let (w, h) = dims(path)?;
    Some(Cand { path: path.to_owned(), kind, side: w.min(h), square: square(w, h), depth, named: named(path, lname), file: None })
}

fn square(w: u32, h: u32) -> bool {
    w.max(h) as f32 / w.min(h).max(1) as f32 <= 1.15
}

/// An `AppIcon.appiconset`: its smallest image of at least 256 px, else its largest.
fn catalog(dir: &Path, depth: usize, lname: &str) -> Option<Cand> {
    let mut best: Option<(PathBuf, u32, u32)> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten().take(128) {
        let p = e.path();
        let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
        if !matches!(ext.as_str(), "png" | "jpg" | "jpeg") || std::fs::metadata(&p).map_or(true, |m| m.len() > MAX_BYTES) {
            continue;
        }
        let Some((w, h)) = dims(&p) else { continue };
        let better = |s: u32, b: u32| if s >= 256 && b >= 256 { s < b } else { s > b };
        if best.as_ref().is_none_or(|b| better(w.min(h), b.1.min(b.2))) {
            best = Some((p, w, h));
        }
    }
    let (file, w, h) = best?;
    Some(Cand { path: dir.to_owned(), kind: Kind::App, side: w.min(h), square: square(w, h), depth, named: named(dir, lname), file: Some(file) })
}

/// An Icon Composer `.icon` whose layers include a PNG or JPEG.
fn composer(dir: &Path, depth: usize, lname: &str) -> Option<Cand> {
    let manifest = read_json(&dir.join("icon.json"))?;
    let has_raster = layers(&manifest).any(|(name, _, _)| {
        let n = name.to_lowercase();
        (n.ends_with(".png") || n.ends_with(".jpg") || n.ends_with(".jpeg")) && dir.join("Assets").join(name).is_file()
    });
    if !has_raster {
        return None;
    }
    Some(Cand { path: dir.to_owned(), kind: Kind::Composer, side: 1024, square: true, depth, named: named(dir, lname), file: None })
}

fn read_json(p: &Path) -> Option<Value> {
    if std::fs::metadata(p).ok()?.len() > 1 << 20 {
        return None;
    }
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

/// A manifest's layers, top first: image name, scale and translation in points.
fn layers(v: &Value) -> impl Iterator<Item = (&str, f32, (f32, f32))> {
    v["groups"].as_array().into_iter().flatten().flat_map(|g| g["layers"].as_array().into_iter().flatten()).filter_map(|l| {
        let name = l["image-name"].as_str()?;
        let pos = &l["position"];
        let scale = pos["scale"].as_f64().unwrap_or(1.0) as f32;
        let t = &pos["translation-in-points"];
        Some((name, scale, (t[0].as_f64().unwrap_or(0.0) as f32, t[1].as_f64().unwrap_or(0.0) as f32)))
    })
}

/// Width and height without decoding: PNG and JPEG headers, or an ICNS's best PNG.
fn dims(path: &Path) -> Option<(u32, u32)> {
    let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
    let d = if ext == "icns" {
        let bytes = std::fs::read(path).ok()?;
        let png = icns_png(&bytes)?;
        png_dims(png)?
    } else {
        image::image_dimensions(path).ok()?
    };
    (d.0 > 0 && d.1 > 0 && d.0 <= MAX_SIDE && d.1 <= MAX_SIDE).then_some(d)
}

fn png_dims(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || &b[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let be = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    Some((be(16), be(20)))
}

/// The PNG representation in an ICNS that best fits: the smallest of at least 256 px, else the
/// largest. Legacy (non-PNG) representations are skipped.
fn icns_png(b: &[u8]) -> Option<&[u8]> {
    if b.len() < 8 || &b[..4] != b"icns" {
        return None;
    }
    let mut best: Option<(&[u8], u32)> = None;
    let mut at = 8;
    while at + 8 <= b.len() {
        let len = u32::from_be_bytes([b[at + 4], b[at + 5], b[at + 6], b[at + 7]]) as usize;
        if len < 8 || at + len > b.len() {
            break;
        }
        let data = &b[at + 8..at + len];
        if let Some((w, h)) = png_dims(data) {
            let s = w.min(h);
            let better = |o: u32| if s >= 256 && o >= 256 { s < o } else { s > o };
            if w == h && best.is_none_or(|(_, o)| better(o)) {
                best = Some((data, s));
            }
        }
        at += len;
    }
    best.map(|(d, _)| d)
}

/// The candidate's pixels, and whether to trim transparent margins.
fn decode(c: &Cand) -> Option<(Buf, bool)> {
    if c.kind == Kind::Composer {
        return flatten_composer(&c.path).map(|b| (b, false));
    }
    let file = c.file.as_ref().unwrap_or(&c.path);
    let bytes = std::fs::read(file).ok()?;
    let img = if file.extension().is_some_and(|x| x.eq_ignore_ascii_case("icns")) {
        image::load_from_memory_with_format(icns_png(&bytes)?, image::ImageFormat::Png).ok()?
    } else {
        image::load_from_memory(&bytes).ok()?
    };
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return None;
    }
    Some((Buf::from_rgba8(&img.into_rgba8()), true))
}

/// An Icon Composer icon on a 256 px canvas: its solid or gradient fill, then its raster layers.
fn flatten_composer(dir: &Path) -> Option<Buf> {
    const C: u32 = 256;
    let v = read_json(&dir.join("icon.json"))?;
    let fill = &v["fill"];
    let stops: Vec<[f32; 4]> = match fill["solid"].as_str() {
        Some(s) => composer_color(s).into_iter().collect(),
        None => fill["linear-gradient"].as_array().into_iter().flatten().filter_map(|c| c.as_str().and_then(composer_color)).collect(),
    };
    let stops = if stops.is_empty() { vec![[1.0, 1.0, 1.0, 1.0]] } else { stops };
    let o = &fill["orientation"];
    let pt = |k: &str, d: (f32, f32)| (o[k]["x"].as_f64().map_or(d.0, |x| x as f32), o[k]["y"].as_f64().map_or(d.1, |y| y as f32));
    let (a, b) = (pt("start", (0.5, 0.0)), pt("stop", (0.5, 1.0)));
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = (dx * dx + dy * dy).max(1e-6);
    let mut canvas = Buf::new(C, C);
    for y in 0..C {
        for x in 0..C {
            let (u, v) = ((x as f32 + 0.5) / C as f32, (y as f32 + 0.5) / C as f32);
            let t = (((u - a.0) * dx + (v - a.1) * dy) / len2).clamp(0.0, 1.0);
            canvas.px[(y * C + x) as usize] = gradient(&stops, t);
        }
    }
    let k = C as f32 / 1024.0;
    let mut drew = false;
    let all: Vec<_> = layers(&v).collect();
    for (name, scale, (tx, ty)) in all.into_iter().rev() {
        let file = dir.join("Assets").join(name);
        let lower = name.to_lowercase();
        if !(lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg")) {
            continue;
        }
        if std::fs::metadata(&file).map_or(true, |m| m.len() > MAX_BYTES) {
            continue;
        }
        let Some(img) = std::fs::read(&file).ok().and_then(|b| image::load_from_memory(&b).ok()) else { continue };
        if img.width() > MAX_SIDE || img.height() > MAX_SIDE {
            continue;
        }
        let w = ((img.width() as f32 * scale * k).round() as u32).clamp(1, C * 2);
        let h = ((img.height() as f32 * scale * k).round() as u32).clamp(1, C * 2);
        let layer = Buf::from_rgba8(&img.into_rgba8()).resize(w, h);
        let ox = ((C as f32 - w as f32) / 2.0 + tx * k).round() as i64;
        let oy = ((C as f32 - h as f32) / 2.0 + ty * k).round() as i64;
        canvas.over(&layer, ox, oy);
        drew = true;
    }
    drew.then_some(canvas)
}

/// `gray:g,a`, `srgb:r,g,b,a` or `display-p3:r,g,b,a` (read as sRGB) to linear premultiplied.
fn composer_color(s: &str) -> Option<[f32; 4]> {
    let (space, rest) = s.split_once(':')?;
    let n: Vec<f32> = rest.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    let (r, g, b, a) = match (space, n.as_slice()) {
        ("gray", [g, a]) => (*g, *g, *g, *a),
        ("srgb" | "extended-srgb" | "display-p3", [r, g, b, a]) => (*r, *g, *b, *a),
        _ => return None,
    };
    let a = a.clamp(0.0, 1.0);
    Some([lin(r.clamp(0.0, 1.0)) * a, lin(g.clamp(0.0, 1.0)) * a, lin(b.clamp(0.0, 1.0)) * a, a])
}

fn gradient(stops: &[[f32; 4]], t: f32) -> [f32; 4] {
    if stops.len() == 1 {
        return stops[0];
    }
    let f = t * (stops.len() - 1) as f32;
    let i = (f.floor() as usize).min(stops.len() - 2);
    let u = f - i as f32;
    std::array::from_fn(|c| stops[i][c] + (stops[i + 1][c] - stops[i][c]) * u)
}

fn raster_data_url(u: &str) -> bool {
    ["data:image/png;", "data:image/jpeg;", "data:image/jpg;"].iter().any(|p| u.starts_with(p))
}

fn data_url_bytes(u: &str) -> Option<Vec<u8>> {
    let (head, body) = u.split_once(',')?;
    head.ends_with(";base64").then(|| base64(body))?
}

fn base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------------------------
// Pixels: linear light, premultiplied alpha.

fn lin(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn enc(c: f32) -> f32 {
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

struct Buf {
    w: u32,
    h: u32,
    px: Vec<[f32; 4]>,
}

impl Buf {
    fn new(w: u32, h: u32) -> Buf {
        Buf { w, h, px: vec![[0.0; 4]; (w * h) as usize] }
    }

    fn from_rgba8(img: &RgbaImage) -> Buf {
        let table: Vec<f32> = (0..256).map(|i| lin(i as f32 / 255.0)).collect();
        let px = img.pixels().map(|p| {
            let a = p[3] as f32 / 255.0;
            [table[p[0] as usize] * a, table[p[1] as usize] * a, table[p[2] as usize] * a, a]
        });
        Buf { w: img.width(), h: img.height(), px: px.collect() }
    }

    fn to_rgba8(&self) -> Vec<u8> {
        let q = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
        self.px
            .iter()
            .flat_map(|&[r, g, b, a]| {
                if a <= 1e-4 {
                    return [0, 0, 0, 0];
                }
                [q(enc(r / a)), q(enc(g / a)), q(enc(b / a)), q(a)]
            })
            .collect()
    }

    fn coverage(&self) -> f32 {
        self.px.iter().map(|p| p[3]).sum::<f32>() / self.px.len().max(1) as f32
    }

    /// Crops to the visible pixels, as a centred square.
    fn trim(self) -> Buf {
        let (mut x0, mut y0, mut x1, mut y1) = (self.w, self.h, 0, 0);
        for y in 0..self.h {
            for x in 0..self.w {
                if self.px[(y * self.w + x) as usize][3] > 0.03 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        if x1 <= x0 || y1 <= y0 {
            return self;
        }
        let side = (x1 - x0).max(y1 - y0);
        if side == self.w && side == self.h {
            return self;
        }
        let (cx, cy) = ((x0 + x1) as i64, (y0 + y1) as i64);
        let (ox, oy) = ((cx - side as i64) / 2, (cy - side as i64) / 2);
        let mut out = Buf::new(side, side);
        out.over(&self, -ox, -oy);
        out
    }

    /// Draws `src` over this at (`ox`, `oy`).
    fn over(&mut self, src: &Buf, ox: i64, oy: i64) {
        for y in 0..src.h as i64 {
            let ty = y + oy;
            if ty < 0 || ty >= self.h as i64 {
                continue;
            }
            for x in 0..src.w as i64 {
                let tx = x + ox;
                if tx < 0 || tx >= self.w as i64 {
                    continue;
                }
                let s = src.px[(y * src.w as i64 + x) as usize];
                let d = &mut self.px[(ty * self.w as i64 + tx) as usize];
                for c in 0..4 {
                    d[c] = s[c] + d[c] * (1.0 - s[3]);
                }
            }
        }
    }

    /// Halves with a box filter while that stays at least twice the target, then Lanczos.
    fn resize(self, w: u32, h: u32) -> Buf {
        let mut b = self;
        while b.w >= w * 4 && b.h >= h * 4 {
            b = b.halve();
        }
        if b.w == w && b.h == h {
            return b;
        }
        let flat: Vec<f32> = b.px.iter().flatten().copied().collect();
        let img: ImageBuffer<Rgba<f32>, Vec<f32>> = ImageBuffer::from_raw(b.w, b.h, flat).expect("buffer size");
        let out = image::imageops::resize(&img, w, h, FilterType::Lanczos3);
        let raw = out.into_raw();
        let px = raw.chunks_exact(4).map(|c| {
            let a = c[3].clamp(0.0, 1.0);
            [c[0].clamp(0.0, a), c[1].clamp(0.0, a), c[2].clamp(0.0, a), a]
        });
        Buf { w, h, px: px.collect() }
    }

    fn halve(&self) -> Buf {
        let (w, h) = (self.w / 2, self.h / 2);
        let mut out = Buf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let at = |dx: u32, dy: u32| self.px[((y * 2 + dy) * self.w + x * 2 + dx) as usize];
                let (a, b, c, d) = (at(0, 0), at(1, 0), at(0, 1), at(1, 1));
                out.px[(y * w + x) as usize] = std::array::from_fn(|i| (a[i] + b[i] + c[i] + d[i]) * 0.25);
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------
// Procedural marks

fn fnv(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &c| (h ^ c as u64).wrapping_mul(0x0100_0000_01b3))
}

fn hash(s: &str) -> u64 {
    mix(fnv(s.as_bytes()))
}

fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn premul(c: Color, a: f32) -> [f32; 4] {
    let a = c.a * a;
    [c.r * a, c.g * a, c.b * a, a]
}

fn blend(d: [f32; 4], s: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|i| s[i] + d[i] * (1.0 - s[3]))
}

/// Signed distance to a rounded box centred at the origin.
fn round_box(x: f32, y: f32, half: f32, r: f32) -> f32 {
    let (qx, qy) = (x.abs() - half + r, y.abs() - half + r);
    (qx.max(0.0).hypot(qy.max(0.0))) + qx.max(qy).min(0.0) - r
}

/// A mark's pixels (straight-alpha sRGB), sampled 4×4 per pixel over a 16 pt tile.
fn draw_mark(p: &Pal, seed: &str, style: Style, variant: usize) -> Vec<u8> {
    let h = hash(seed);
    let (bg, border) = p.avatars[variant];
    let fg = p.avatar_fg;
    let shade = |x: f32, y: f32| -> [f32; 4] {
        let d = round_box(x - 8.0, y - 8.0, 8.0, 4.0);
        if d > 0.0 {
            return [0.0; 4];
        }
        let mut c = match style {
            Style::Gradient => {
                // Towards a neighbouring hue (never gray), along a seeded diagonal.
                // `Pal::avatars` indices around the colour wheel: red, orange, yellow, green, cyan,
                // blue, purple, pink.
                const WHEEL: [usize; 8] = [4, 0, 1, 3, 2, 6, 7, 5];
                let other = match WHEEL.iter().position(|&i| i == variant) {
                    Some(at) => WHEEL[(at + if h >> 8 & 1 == 0 { 1 } else { 7 }) % 8],
                    None => WHEEL[(h >> 8) as usize % 8],
                };
                let angle = ((h >> 16) & 0xffff) as f32 / 65535.0 * std::f32::consts::TAU;
                let t = (((x - 8.0) * angle.cos() + (y - 8.0) * angle.sin()) / 11.3 + 0.5).clamp(0.0, 1.0);
                let t = t * t * (3.0 - 2.0 * t);
                let base = bg.mix(p.avatars[other].0, t);
                let (hx, hy) = (if h & 1 == 0 { 3.0 } else { 13.0 }, 3.0);
                let glow = (1.0 - ((x - hx).hypot(y - hy) / 12.0)).clamp(0.0, 1.0);
                blend(premul(base, 1.0), premul(fg, 0.16 * glow * glow))
            }
            _ => premul(bg, 1.0),
        };
        let ink = match style {
            Style::Glyph => glyph(h, x - 8.0, y - 8.0),
            Style::Quilt => quilt(h, x, y),
            _ => 0.0,
        };
        if ink > 0.0 {
            c = blend(c, premul(fg, 0.92 * ink));
        }
        if d > -0.5 {
            c = blend(c, premul(border, if style == Style::Gradient { 0.5 } else { 1.0 }));
        }
        c
    };
    let n = 4;
    let k = PT / PX as f32;
    let mut buf = Buf::new(PX, PX);
    for py in 0..PX {
        for px in 0..PX {
            let mut acc = [0.0f32; 4];
            for sy in 0..n {
                for sx in 0..n {
                    let x = (px as f32 + (sx as f32 + 0.5) / n as f32) * k;
                    let y = (py as f32 + (sy as f32 + 0.5) / n as f32) * k;
                    let s = shade(x, y);
                    for i in 0..4 {
                        acc[i] += s[i];
                    }
                }
            }
            buf.px[(py * PX + px) as usize] = acc.map(|v| v / (n * n) as f32);
        }
    }
    buf.to_rgba8()
}

/// One of eight small shapes, centred, in tile points: 1 inside, 0 outside.
fn glyph(h: u64, x: f32, y: f32) -> f32 {
    let inside = |b: bool| if b { 1.0 } else { 0.0 };
    match (h >> 24) % 8 {
        0 => inside(x.hypot(y) <= 2.75),
        1 => inside((x.hypot(y) - 2.9).abs() <= 0.8),
        2 => inside(x.abs() + y.abs() <= 3.4),
        3 => inside(y <= 2.4 && y >= -3.0 && x.abs() <= (y + 3.0) * 0.62),
        4 => inside(round_box(x, y, 2.6, 0.9) <= 0.0),
        5 => inside(x.hypot(y + 1.2) <= 3.3 && y >= -1.2),
        6 => inside((x - 1.9).hypot(y) <= 1.6 || (x + 1.9).hypot(y) <= 1.6),
        _ => inside((x.abs() <= 0.85 && y.abs() <= 3.2) || (y.abs() <= 0.85 && x.abs() <= 3.2)),
    }
}

/// A mirrored 3×3 grid of cells inside a 2.5 pt margin; at least three cells are filled.
fn quilt(h: u64, x: f32, y: f32) -> f32 {
    let (m, cell) = (2.5, 11.0 / 3.0);
    let (gx, gy) = ((x - m) / cell, (y - m) / cell);
    if !(0.0..3.0).contains(&gx) || !(0.0..3.0).contains(&gy) {
        return 0.0;
    }
    let (cx, cy) = (gx as usize, gy as usize);
    // Six free cells (two columns, three rows); the third column mirrors the first.
    let mut bits = (h >> 32) as u32 & 0x3f;
    if bits.count_ones() < 3 {
        bits |= 0b010_101;
    }
    let col = if cx == 2 { 0 } else { cx };
    if bits >> (cy * 2 + col) & 1 == 0 {
        return 0.0;
    }
    let (fx, fy) = (gx.fract(), gy.fract());
    let gap = 0.08;
    if fx < gap || fx > 1.0 - gap || fy < gap || fy > 1.0 - gap { 0.0 } else { 1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, alpha: u8) -> Vec<u8> {
        let img = RgbaImage::from_pixel(w, h, Rgba([200, 40, 40, alpha]));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn icns_picks_the_fitting_png() {
        let mut icns = b"icns\0\0\0\0".to_vec();
        for (ty, side) in [(b"ic07", 128), (b"ic09", 512), (b"ic08", 256)] {
            let data = png(side, side, 255);
            icns.extend_from_slice(ty);
            icns.extend_from_slice(&(data.len() as u32 + 8).to_be_bytes());
            icns.extend_from_slice(&data);
        }
        assert_eq!(png_dims(icns_png(&icns).unwrap()), Some((256, 256)));
    }

    #[test]
    fn data_urls_decode() {
        let bytes = png(4, 4, 255);
        let enc: String = {
            const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            bytes
                .chunks(3)
                .flat_map(|c| {
                    let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
                    (0..4).map(move |i| if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' })
                })
                .collect()
        };
        let url = format!("data:image/png;base64,{enc}");
        assert!(raster_data_url(&url));
        assert_eq!(data_url_bytes(&url).unwrap(), bytes);
        assert!(!raster_data_url("data:image/svg+xml;base64,AAAA"));
    }

    #[test]
    fn finds_the_best_square_icon() {
        let root = std::env::temp_dir().join(format!("project-icon-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("public")).unwrap();
        std::fs::create_dir_all(root.join("App/Assets.xcassets/AppIcon.appiconset")).unwrap();
        std::fs::write(root.join("public/favicon.png"), png(32, 32, 255)).unwrap();
        std::fs::write(root.join("logo.png"), png(600, 200, 255)).unwrap();
        std::fs::write(root.join("App/Assets.xcassets/AppIcon.appiconset/icon_128.png"), png(128, 128, 255)).unwrap();
        std::fs::write(root.join("App/Assets.xcassets/AppIcon.appiconset/icon_512.png"), png(512, 512, 255)).unwrap();
        let mut c = candidates(&root, "demo");
        c.sort_by_key(|c| c.rank());
        assert_eq!(c[0].kind, Kind::App);
        assert!(c[0].file.as_ref().unwrap().ends_with("icon_512.png"));
        assert!(c.iter().any(|x| x.kind == Kind::Web));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn trim_centres_visible_pixels() {
        let mut img = RgbaImage::new(100, 100);
        for y in 10..90 {
            for x in 20..80 {
                img.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        let b = Buf::from_rgba8(&img).trim();
        assert_eq!((b.w, b.h), (80, 80));
        assert!(b.coverage() > 0.7 && b.coverage() < 0.8);
    }

    #[test]
    fn marks_are_stable_and_quiet() {
        let p = crate::palette::pal(true);
        for style in [Style::Glyph, Style::Gradient, Style::Quilt] {
            let a = draw_mark(p, "abc", style, 3);
            assert_eq!(a, draw_mark(p, "abc", style, 3));
            // Corners stay transparent (the 4 pt radius).
            assert_eq!(a[3], 0);
        }
    }
}

//! An image file open in the Files panel: Desktop's `ImageStage` (`@opencode/plugin` file
//! `artifact-view.tsx:281-506`), fitted with room to spare, zoomed with a click, a pinch or its
//! `PreviewControls`, and panned once zoomed (rune-kit's `Stage`). An SVG previews this way too
//! and switches to its source, as Markdown does.

use rune::{El, Image, ImageCache, SurfaceFit, el};
use rune_kit::{Stage, StageZoom};
use rune_theme_opencode::icons as ic;

use super::SessionFiles;
use crate::App;
use crate::palette::Pal;

/// A picture file's decoded image and its zoom.
pub struct FileImage {
    pub image: Image,
    /// Shown while `image` decodes on a reload, so replacing a picture never flashes blank.
    previous: Option<Image>,
    pub zoom: StageZoom,
    svg: bool,
    /// The file's size in bytes.
    bytes: u64,
    /// Its decode has finished and a view has run since.
    landed: bool,
}

/// What Desktop shows as a picture, by extension (`util/src/artifact.ts`): a format the image
/// cache can't read shows as a binary file.
pub fn is_image(path: &str) -> bool {
    extension(path).is_some_and(|e| matches!(e.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "bmp" | "ico" | "tif" | "tiff" | "heic"))
}

pub fn is_svg(path: &str) -> bool {
    extension(path).is_some_and(|e| e == "svg")
}

fn extension(path: &str) -> Option<String> {
    let name = opencode_kit::path::basename(path);
    name.rsplit_once('.').filter(|(stem, _)| !stem.is_empty()).map(|(_, e)| e.to_ascii_lowercase())
}

impl FileImage {
    pub fn new(cache: &ImageCache, bytes: Vec<u8>, svg: bool) -> FileImage {
        let image = cache.load(bytes.clone());
        let landed = image.is_ready();
        FileImage {
            bytes: bytes.len() as u64,
            image,
            previous: None,
            zoom: StageZoom::default(),
            svg,
            landed,
        }
    }

    /// Keeps `old`'s zoom and decoded pixels until the reloaded image finishes decoding.
    pub fn carry_from(&mut self, old: FileImage) {
        if !self.image.is_ready() {
            self.previous = old.shown_ready().cloned();
        }
        self.zoom = old.zoom;
    }

    /// The decoded handle currently shown (the new one once ready, else the previous revision's).
    pub fn shown(&self) -> &Image {
        if self.image.is_ready() {
            &self.image
        } else {
            self.previous.as_ref().unwrap_or(&self.image)
        }
    }

    fn shown_ready(&self) -> Option<&Image> {
        let img = self.shown();
        (img.is_ready() && img.error().is_none()).then_some(img)
    }

    /// Its own size in points: an SVG rasterizes at twice its own.
    fn natural(&self) -> Option<[f32; 2]> {
        let [w, h] = self.shown().size()?;
        Some(if self.svg { [w as f32 / 2.0, h as f32 / 2.0] } else { [w as f32, h as f32] })
    }

    /// What the toolbar says of it (`artifact-view.tsx` `meta`): its dimensions once decoded and
    /// its size, `1200 x 520 · 23 kB`.
    pub fn meta(&self) -> Vec<String> {
        let dimensions = self.natural().filter(|[w, h]| *w > 0.0 && *h > 0.0).map(|[w, h]| format!("{} x {}", w.round(), h.round()));
        dimensions.into_iter().chain([opencode_i18n::bytes(self.bytes)]).collect()
    }

    /// True once, when the decode finishes.
    /// Whether landed would report the picture decoded: polls write the files only then.
    pub fn lands(&self) -> bool {
        !self.landed && self.image.is_ready()
    }

    pub fn landed(&mut self) -> bool {
        if self.landed || !self.image.is_ready() { return false; }
        self.previous = None;
        self.landed = true;
        true
    }

    /// Decoded and drawable; a file that failed shows as binary.
    pub fn shows(&self) -> bool {
        self.shown().error().is_none()
    }
}

/// The open picture's zoom, for the stage's handlers: the active file tab's.
fn zoom(s: &mut App) -> &mut StageZoom {
    let active = s.active_file_tab().and_then(|(sid, path)| Some((sid, path?)));
    let files = &mut *s.files;
    match active.and_then(|(sid, path)| files.sessions.get_mut(&sid)?.images.get_mut(&path)) {
        Some(img) => &mut img.zoom,
        None => &mut files.spare_zoom,
    }
}

/// The stage for `img`, filling the panel under the toolbar.
pub fn stage(s: &App, p: &Pal, img: &FileImage) -> El {
    let k = crate::palette::kit(p);
    let natural = img.natural().unwrap_or([1.0, 1.0]);
    let size = s.files.stage_size.get();
    let picture = el().key("artifact-media").pixels_fit(img.shown().pixels(), SurfaceFit::Fill);
    let stage = Stage::new(zoom, natural, picture)
        .grows(img.svg)
        .icons(&ic::OUTLINE_MAGNIFIER_PLUS, &ic::OUTLINE_MAGNIFIER_MINUS, &ic::OUTLINE_EXPAND)
        .labels(
            opencode_i18n::t!("ui.preview.zoomIn"),
            opencode_i18n::t!("ui.preview.zoomOut"),
            opencode_i18n::t!("ui.preview.fit"),
            opencode_i18n::t!("ui.preview.controls"),
        )
        .build(&img.zoom, k, size);
    let sizer = s.files.stage_size.clone();
    stage.on_layout_tx(move |_: &mut App, [_, _, w, h], tx| {
        sizer.set([w, h]);
        tx.instant();
    })
}

/// Whether the open file shows as a picture: a decoded image, or an SVG in preview.
pub fn showing<'a>(ws: &'a SessionFiles, path: &str) -> Option<&'a FileImage> {
    let img = ws.images.get(path).filter(|i| i.shows())?;
    if img.svg && !super::markdown::shows_rendered(ws, path) { return None; }
    Some(img)
}

//! An image file open in the Files panel: Desktop's `ImageStage` (`@opencode/plugin` file
//! `artifact-view.tsx:281-506`), fitted with room to spare, zoomed with a click, a pinch or its
//! `PreviewControls`, and panned once zoomed (rune-kit's `Stage`). An SVG previews this way too
//! and switches to its source, as Markdown does.

use rune::{El, Image, ImageCache, SurfaceFit, el};
use rune_kit::{Stage, StageZoom};
use rune_theme_opencode::icons as ic;
use opencode_ui::palette::{Pal, kit};

use crate::{FilesHost, FilesPane, SessionFiles};

/// Decode lifecycle of a picture file: while `next` decodes on a reload, `previous` stays visible
/// so replacing a picture never flashes blank; once landed, only the decoded image remains.
enum Decode {
    Decoding { next: Image, previous: Option<Image> },
    Landed(Image),
}

enum DecodeEvent {
    CarryPrevious(Option<Image>),
    CheckReady,
}

enum DecodeEffect {
    None,
    Landed,
}

impl Decode {
    fn new(image: Image) -> Self {
        if image.is_ready() {
            Self::Landed(image)
        } else {
            Self::Decoding { next: image, previous: None }
        }
    }

    fn shown(&self) -> &Image {
        match self {
            Self::Landed(image) => image,
            Self::Decoding { next, previous } => {
                if next.is_ready() {
                    next
                } else {
                    previous.as_ref().unwrap_or(next)
                }
            }
        }
    }

    fn lands(&self) -> bool {
        matches!(self, Self::Decoding { next, .. } if next.is_ready())
    }

    fn step(&mut self, event: DecodeEvent) -> DecodeEffect {
        match event {
            DecodeEvent::CarryPrevious(prev) => {
                if let Self::Decoding { next, previous } = self
                    && !next.is_ready()
                {
                    *previous = prev;
                }
                DecodeEffect::None
            }
            DecodeEvent::CheckReady => {
                let ready = match self {
                    Self::Decoding { next, .. } if next.is_ready() => Some(next.clone()),
                    _ => None,
                };
                if let Some(image) = ready {
                    *self = Self::Landed(image);
                    DecodeEffect::Landed
                } else {
                    DecodeEffect::None
                }
            }
        }
    }
}

/// A picture file's decoded image and its zoom.
pub struct FileImage {
    decode: Decode,
    pub zoom: StageZoom,
    svg: bool,
    /// The file's size in bytes.
    bytes: u64,
}

/// What Desktop shows as a picture, by extension (`util/src/artifact.ts`): a format the image
/// cache can't read shows as a binary file.
pub fn is_image(path: &str) -> bool {
    extension(path).is_some_and(|e| {
        matches!(
            e.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "bmp" | "ico" | "tif" | "tiff" | "heic"
        )
    })
}

pub fn is_svg(path: &str) -> bool {
    extension(path).is_some_and(|e| e == "svg")
}

fn extension(path: &str) -> Option<String> {
    let name = opencode_kit::path::basename(path);
    name.rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map(|(_, e)| e.to_ascii_lowercase())
}

impl FileImage {
    pub fn new(cache: &ImageCache, bytes: Vec<u8>, svg: bool) -> FileImage {
        let len = bytes.len() as u64;
        let decode = Decode::new(cache.load(bytes));
        FileImage {
            bytes: len,
            decode,
            zoom: StageZoom::default(),
            svg,
        }
    }

    /// Keeps `old`'s zoom and decoded pixels until the reloaded image finishes decoding.
    pub fn carry_from(&mut self, old: FileImage) {
        self.decode.step(DecodeEvent::CarryPrevious(old.shown_ready().cloned()));
        self.zoom = old.zoom;
    }

    /// The decoded handle currently shown (the new one once ready, else the previous revision's).
    pub fn shown(&self) -> &Image {
        self.decode.shown()
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
        let dimensions = self
            .natural()
            .filter(|[w, h]| *w > 0.0 && *h > 0.0)
            .map(|[w, h]| format!("{} x {}", w.round(), h.round()));
        dimensions.into_iter().chain([opencode_i18n::bytes(self.bytes)]).collect()
    }

    /// Whether `landed` would report the picture decoded: polls write the files only then.
    pub fn lands(&self) -> bool {
        self.decode.lands()
    }

    /// True once, when the decode finishes.
    pub fn landed(&mut self) -> bool {
        matches!(self.decode.step(DecodeEvent::CheckReady), DecodeEffect::Landed)
    }

    /// Decoded and drawable; a file that failed shows as binary.
    pub fn shows(&self) -> bool {
        self.shown().error().is_none()
    }
}

/// The open picture's zoom, for the stage's handlers: the active file tab's.
fn zoom_dyn(host: &mut dyn FilesHost) -> &mut StageZoom {
    let active = host.active_file_tab().and_then(|(sid, path)| Some((sid, path?)));
    let FilesPane { sessions, spare_zoom, .. } = host.files_mut();
    match active.and_then(|(sid, path)| sessions.get_mut(&sid)?.images.get_mut(&path)) {
        Some(img) => &mut img.zoom,
        None => spare_zoom,
    }
}

fn zoom<H: FilesHost>(s: &mut H) -> &mut StageZoom {
    zoom_dyn(s)
}

/// The stage for `img`, filling the panel under the toolbar.
pub fn stage<H: FilesHost + 'static>(s: &H, p: &Pal, img: &FileImage) -> El {
    let k = kit(p);
    let natural = img.natural().unwrap_or([1.0, 1.0]);
    let size = s.files().stage_size.get();
    let picture = el().key("artifact-media").pixels_fit(img.shown().pixels(), SurfaceFit::Fill);
    let stage = Stage::new(zoom::<H>, natural, picture)
        .grows(img.svg)
        .icons(&ic::OUTLINE_MAGNIFIER_PLUS, &ic::OUTLINE_MAGNIFIER_MINUS, &ic::OUTLINE_EXPAND)
        .labels(
            opencode_i18n::t!("ui.preview.zoomIn"),
            opencode_i18n::t!("ui.preview.zoomOut"),
            opencode_i18n::t!("ui.preview.fit"),
            opencode_i18n::t!("ui.preview.controls"),
        )
        .build(&img.zoom, k, size);
    let sizer = s.files().stage_size.clone();
    stage.on_layout_tx(move |s: &mut H, [_, _, w, h], tx| {
        let prev = sizer.get();
        if (prev[0] - w).abs() > 0.5 || (prev[1] - h).abs() > 0.5 {
            sizer.set([w, h]);
            s.touch_files();
            tx.instant();
        } else {
            tx.live_only();
        }
    })
}

/// Whether the open file shows as a picture: a decoded image, or an SVG in preview.
pub fn showing<'a>(ws: &'a SessionFiles, path: &str) -> Option<&'a FileImage> {
    let img = ws.images.get(path).filter(|i| i.shows())?;
    if img.svg && !super::markdown::shows_rendered(ws, path) {
        return None;
    }
    Some(img)
}

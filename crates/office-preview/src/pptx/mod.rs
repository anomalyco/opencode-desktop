//! PowerPoint (`presentation.worker.ts`): the deck opens with no faces of its own, every family it
//! names is registered with the face PowerPoint would draw it with (`presentationFace`), and each
//! slide is laid out and becomes a scene on request. After BetterOffice's native viewer
//! (`pptx_editing.rs`, Apache-2.0).

mod paint;
mod scene;

use std::collections::HashMap;

use anyhow::{Context, Result};
use betteroffice_fonts::{FontSource, LastResortOffice};
use betteroffice_pptx::{OpenOptions, PptxPackage, Presentation};
use serde_json::Value;
use vello::peniko::{Blob, FontData, ImageAlphaType, ImageData, ImageFormat};

use crate::fonts::{FontPacks, cjk_text};
use crate::shared::PageScene;
use crate::PageInfo;

/// Px per English Metric Unit: a slide's size from its EMU.
const EMU_PER_PX: f64 = 9525.0;

pub struct Pptx {
    deck: Presentation,
    fonts: HashMap<u32, FontData>,
    images: HashMap<String, Result<ImageData, String>>,
    label: Option<(FontData, Vec<u8>)>,
    pages: Vec<PageInfo>,
    /// The deck holds Chinese, Japanese or Korean text.
    pub wants_cjk: bool,
}

impl Pptx {
    pub fn open(bytes: &[u8], packs: &FontPacks) -> Result<Pptx> {
        let mut deck = Presentation::open_with_options(bytes, &OpenOptions { font_source: FontSource::Backstop, ..OpenOptions::default() })?;
        let model = serde_json::to_value(deck.model()).unwrap_or_default();
        let mut families = Vec::new();
        collect_families(&model, &mut families);
        let mut fonts = HashMap::new();
        for family in families.iter().map(String::as_str).chain(["Calibri", "Arial"]) {
            for (bold, italic) in [(false, false), (true, false), (false, true), (true, true)] {
                if let Some((_, bytes)) = packs.face(family, bold, italic, LastResortOffice::PowerPoint)
                    && let Ok(id) = deck.register_font(family, bold, italic, &bytes)
                {
                    fonts.insert(id, FontData::new(Blob::from(bytes), 0));
                }
            }
        }
        let text = model.to_string();
        let wants_cjk = cjk_text(&text);
        let scripts = ["arabic", "hebrew"].into_iter().chain(packs.has_cjk().then_some(["cjk-sc", "cjk-tc", "cjk-jp", "cjk-kr"]).into_iter().flatten());
        for handle in scripts.flat_map(|s| [false, true].map(|bold| packs.script_face(s, bold))).flatten() {
            if let Ok(bytes) = handle.load()
                && let Ok(id) = deck.register_fallback_font(&handle.family, handle.bold, handle.italic, &bytes)
            {
                fonts.insert(id, FontData::new(Blob::from(bytes), 0));
            }
        }
        let label = packs.face("Arial", false, false, LastResortOffice::PowerPoint).map(|(_, bytes)| (FontData::new(Blob::from(bytes.clone()), 0), bytes));
        let snapshot = deck.snapshot().context("snapshot opened PPTX")?;
        let (width, height) = (snapshot.width_emu as f64 / EMU_PER_PX, snapshot.height_emu as f64 / EMU_PER_PX);
        let pages = deck.slides().iter().map(|s| PageInfo { width, height, hidden: s.hidden == Some(true), notes: s.notes.trim().to_owned() }).collect();
        let images = decode_media(deck.package());
        Ok(Pptx { deck, fonts, images, label, pages, wants_cjk })
    }

    pub fn pages(&self) -> Vec<PageInfo> {
        self.pages.clone()
    }

    pub fn scene(&self, slide: usize) -> Result<PageScene> {
        let rendered = self.deck.render_slide(slide).with_context(|| format!("layout slide {}", slide + 1))?;
        let images = |asset: &str, effects: &[pptx_render::ImageEffect]| -> Result<ImageData, String> {
            let mut image = match self.images.get(asset) {
                Some(Ok(image)) => image.clone(),
                Some(Err(reason)) => return Err(reason.clone()),
                None => return Err(format!("image asset {asset} is missing")),
            };
            if !effects.is_empty() {
                let mut pixels = image.data.data().to_vec();
                pptx_render::apply_image_effects(&mut pixels, effects);
                image.data = Blob::from(pixels);
            }
            Ok(image)
        };
        let resources = scene::Resources { fonts: &self.fonts, images: &images, label: self.label.as_ref() };
        scene::translate_slide(&rendered.display_list, &resources).map_err(anyhow::Error::msg)
    }
}

fn decode_media(package: &PptxPackage) -> HashMap<String, Result<ImageData, String>> {
    package
        .media
        .iter()
        .map(|part| {
            let decoded = if image::guess_format(&part.bytes).ok() == Some(image::ImageFormat::Tiff) && ooxml_drawingml::media::tiff_bounded_pixels(&part.bytes).is_none() {
                Err("unsupported TIFF encoding".to_owned())
            } else {
                image::load_from_memory(&part.bytes).map_err(|e| format!("image {} is undecodable: {e}", part.part_path))
            };
            let image = decoded.map(|image| {
                let rgba = image.into_rgba8();
                let (width, height) = rgba.dimensions();
                ImageData { data: Blob::from(rgba.into_raw()), format: ImageFormat::Rgba8, alpha_type: ImageAlphaType::Alpha, width, height }
            });
            (part.part_path.clone(), image)
        })
        .collect()
}

/// Every family a deck names (`typeface`, `latin`, `fontFamily`...), theme references left out,
/// in the order met.
fn collect_families(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, v) in map {
                if let (Some(name), true) = (v.as_str(), matches!(k.as_str(), "typeface" | "latin" | "fontFamily" | "family" | "font" | "ea" | "cs")) {
                    let name = name.trim();
                    if !name.is_empty() && !name.starts_with('+') && !out.iter().any(|f| f.eq_ignore_ascii_case(name)) && out.len() < 64 {
                        out.push(name.to_owned());
                    }
                }
                collect_families(v, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| collect_families(v, out)),
        _ => {}
    }
}

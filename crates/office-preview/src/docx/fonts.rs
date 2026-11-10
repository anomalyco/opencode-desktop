//! A Word file's faces: each font requirement the layout reports resolves to a chain of bundled
//! faces (Word's substitutes, a face per script, then Word's last resort), registered once for
//! measurement and once for drawing under the same id. After BetterOffice's native viewer
//! (`fonts.rs`, Apache-2.0) and the `betteroffice-docx` facade's registry.

use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result, anyhow};
use betteroffice_fonts::LastResortOffice;
use docx_raster::FontChains;
use ooxml_text::{FontId, FontStore, ShapeDirection, ShapeFeature, shape_with_direction};
use serde_json::Value;
use vello::Glyph;
use vello::peniko::{Blob, FontData};

use crate::fonts::FontPacks;

type ShapedRuns = Vec<(u32, Vec<Glyph>)>;

pub struct FontRegistry {
    faces: HashMap<u32, FontData>,
    pub store: FontStore,
    pub chains: FontChains,
    pub chain_ids: BTreeMap<String, Vec<u32>>,
}

impl FontRegistry {
    /// Registers the faces `requirements` (the layout's `[{key, family, bold, italic, scripts}]`)
    /// need. Measurement fonts are per thread, so a document opens on a thread of its own.
    pub fn load(requirements: &Value, packs: &FontPacks) -> Result<Self> {
        docx_layout::clear_measure_fonts();
        let mut store = FontStore::new();
        let mut faces = HashMap::new();
        let mut by_bytes: HashMap<(String, bool, bool), u32> = HashMap::new();
        let mut chain_ids = BTreeMap::new();
        let mut register = |family: &str, bold: bool, italic: bool, bytes: Vec<u8>| -> Result<u32> {
            if let Some(id) = by_bytes.get(&(family.to_owned(), bold, italic)) {
                return Ok(*id);
            }
            let engine = docx_layout::register_measure_font(&bytes).map_err(|_| anyhow!("register measurement font {family}"))?;
            let raster = store.register(bytes.clone()).with_context(|| format!("register font {family}"))?;
            if engine != raster.to_u32() {
                return Err(anyhow!("font registry ids diverged at {family}"));
            }
            faces.insert(engine, FontData::new(Blob::from(bytes), 0));
            by_bytes.insert((family.to_owned(), bold, italic), engine);
            Ok(engine)
        };
        for requirement in requirements.as_array().context("font requirements are not an array")? {
            let key = requirement["key"].as_str().context("font requirement has no key")?;
            let family = requirement["family"].as_str().filter(|f| !f.trim().is_empty()).unwrap_or("Calibri");
            let (bold, italic) = (requirement["bold"] == true, requirement["italic"] == true);
            let scripts: Vec<String> = requirement["scripts"].as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_owned)).collect();
            let mut chain = Vec::new();
            for handle in packs.resolve(family, bold, italic, &scripts, LastResortOffice::Word) {
                let Ok(bytes) = handle.load() else { continue };
                if let Ok(id) = register(&handle.family, handle.bold, handle.italic, bytes)
                    && !chain.contains(&id)
                {
                    chain.push(id);
                }
            }
            if chain.is_empty() {
                chain.push(FontId::base_latin(bold, italic).to_u32());
            }
            chain_ids.insert(key.to_owned(), chain);
        }
        for (bold, italic, bytes) in base_latin::faces() {
            faces.insert(FontId::base_latin(bold, italic).to_u32(), FontData::new(Blob::new(std::sync::Arc::new(bytes)), 0));
        }
        let chains = chain_ids.iter().map(|(key, ids)| (key.clone(), ids.iter().copied().map(FontId::from_u32).collect())).collect();
        Ok(Self { faces, store, chains, chain_ids })
    }

    pub fn face(&self, id: u32) -> Option<&FontData> {
        self.faces.get(&id)
    }

    /// Shapes a text run the layout left unshaped (its CSS `font` shorthand), through the
    /// requirement's chain, character by character to the first face that has the glyph.
    pub fn shape_fallback(&self, text: &str, font: &str, rtl: bool, small_caps: bool, letter_spacing: f32, word_spacing: f32) -> Result<(f32, ShapedRuns)> {
        let spec = parse_css_font(font)?;
        let key = format!("{}|{}|{}", spec.family.to_lowercase(), u8::from(spec.bold), u8::from(spec.italic));
        let chain = self.chains.get(&key).cloned().unwrap_or_else(|| vec![FontId::base_latin(spec.bold, spec.italic)]);
        let mut segments: Vec<(FontId, usize, usize)> = Vec::new();
        for (start, ch) in text.char_indices() {
            let font = self.store.resolve_with_family(&chain, ch, spec.bold, spec.italic, &spec.family).ok_or_else(|| anyhow!("text fallback has no face"))?;
            let end = start + ch.len_utf8();
            match segments.last_mut() {
                Some(last) if last.0 == font => last.2 = end,
                _ => segments.push((font, start, end)),
            }
        }
        if rtl {
            segments.reverse();
        }
        let direction = if rtl { ShapeDirection::Rtl } else { ShapeDirection::Ltr };
        let features = if spec.small_caps || small_caps { vec![ShapeFeature { tag: *b"smcp", value: 1 }] } else { Vec::new() };
        let mut pen = 0.0f32;
        let mut runs = Vec::new();
        let count = segments.len();
        for (segment, &(font_id, start, end)) in segments.iter().enumerate() {
            let text = &text[start..end];
            let shaped = shape_with_direction(&self.store, font_id, text, spec.size, &features, direction)?;
            let glyphs = shaped
                .iter()
                .enumerate()
                .map(|(index, glyph)| {
                    let placed = Glyph { id: glyph.glyph_id, x: pen + glyph.x_offset, y: -glyph.y_offset };
                    pen += glyph.x_advance;
                    if shaped.get(index + 1).is_none_or(|next| next.cluster != glyph.cluster) {
                        if text.get(glyph.cluster as usize..).and_then(|t| t.chars().next()) == Some(' ') {
                            pen += word_spacing;
                        }
                        if index + 1 < shaped.len() || segment + 1 < count {
                            pen += letter_spacing;
                        }
                    }
                    placed
                })
                .collect();
            runs.push((font_id.to_u32(), glyphs));
        }
        Ok((spec.size, runs))
    }
}

struct CssFont {
    family: String,
    size: f32,
    bold: bool,
    italic: bool,
    small_caps: bool,
}

fn parse_css_font(font: &str) -> Result<CssFont> {
    let mut size_match = None;
    let mut cursor = 0usize;
    for token in font.split_whitespace() {
        let index = cursor + font[cursor..].find(token).with_context(|| format!("unsupported font shorthand {font}"))?;
        cursor = index + token.len();
        if token.ends_with("px") {
            size_match = Some((index, token));
            break;
        }
    }
    let (size_index, size_token) = size_match.with_context(|| format!("font has no size: {font}"))?;
    let size = size_token.strip_suffix("px").and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite() && *v > 0.0).with_context(|| format!("invalid font size {size_token}"))?;
    let (mut bold, mut italic, mut small_caps) = (false, false, false);
    for token in font[..size_index].split_whitespace() {
        match token {
            "normal" => {}
            "italic" | "oblique" => italic = true,
            "small-caps" => small_caps = true,
            "bold" | "bolder" => bold = true,
            value => {
                let weight = value.parse::<u16>().with_context(|| format!("unsupported font token {value}"))?;
                if !(1..=1000).contains(&weight) {
                    return Err(anyhow!("invalid font weight {value}"));
                }
                bold = weight >= 600;
            }
        }
    }
    let family = first_family(font[size_index + size_token.len()..].trim()).trim_matches(['\'', '"']).trim().to_owned();
    if family.is_empty() {
        return Err(anyhow!("font has no family: {font}"));
    }
    Ok(CssFont { family, size, bold, italic, small_caps })
}

fn first_family(value: &str) -> &str {
    let mut quote = None;
    for (index, c) in value.char_indices() {
        match c {
            '\'' | '"' if quote == Some(c) => quote = None,
            '\'' | '"' if quote.is_none() => quote = Some(c),
            ',' if quote.is_none() => return &value[..index],
            _ => {}
        }
    }
    value
}

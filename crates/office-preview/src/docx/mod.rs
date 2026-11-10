//! Word (`document.worker.ts`): the editing engine seeds from the file, lays it out with measured
//! bundled faces, and builds its display list; each page then becomes a scene on request. After
//! BetterOffice's native viewer (`document.rs` `load_docx`, Apache-2.0).

mod fonts;
mod images;
mod paint;
mod scene;

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use docx_edit::{EngineSession, seed_from_docx};
use docx_layout::display_list::DisplayList;
use docx_parse::{S9PackageWire, S9ParseOptions, parse_docx_s9_wire};
use serde_json::{Value, json};

use crate::fonts::FontPacks;
use crate::shared::PageScene;
use crate::{PageInfo, cjk_requested};

pub struct Docx {
    display_list: DisplayList,
    fonts: fonts::FontRegistry,
    images: images::ImageRegistry,
    /// A requirement named a Chinese, Japanese or Korean script.
    pub wants_cjk: bool,
}

impl Docx {
    pub fn open(bytes: &[u8], packs: &FontPacks) -> Result<Docx> {
        let package = parse_docx_s9_wire(bytes, S9ParseOptions::default())?.document.package;
        let engine = EngineSession::new(0x0056_454c_4c4f);
        seed_from_docx(engine.doc(), bytes).map_err(anyhow::Error::msg)?;
        let probe = region_request(&package, None)?;
        let requirements: Value = serde_json::from_str(&engine.layout_font_requirements_json(&probe.to_string()).map_err(anyhow::Error::msg)?)?;
        let wants_cjk = requirements.as_array().into_iter().flatten().flat_map(|r| r["scripts"].as_array().into_iter().flatten()).any(|s| s.as_str().is_some_and(cjk_requested));
        let fonts = fonts::FontRegistry::load(&requirements, packs)?;
        let request = region_request(&package, Some(&fonts.chain_ids))?;
        engine.layout_document_with_regions_retained_json(&request.to_string()).map_err(anyhow::Error::msg)?;
        engine.build_display_list_frame(&json!({ "fontChains": fonts.chain_ids }).to_string(), 0).map_err(anyhow::Error::msg)?;
        engine.apply_and_layout("body", 0).map_err(anyhow::Error::msg)?;
        let display_list = engine.with_display_list(Clone::clone).context("the document produced no display list")?;
        let images = images::ImageRegistry::load(bytes)?;
        Ok(Docx { display_list, fonts, images, wants_cjk })
    }

    pub fn pages(&self) -> Vec<PageInfo> {
        let px = |n: &serde_json::Number| n.as_f64().unwrap_or(0.0);
        self.display_list.pages.iter().map(|p| PageInfo { width: px(&p.width), height: px(&p.height), ..PageInfo::default() }).collect()
    }

    pub fn scene(&self, page: usize) -> Result<PageScene> {
        let page = self.display_list.pages.get(page).context("page out of range")?;
        scene::translate_page(page, &self.fonts, &self.images)
    }
}

/// The region layout's request (`buildResidentRegionLayoutRequest`): the document's sections and
/// the final one, its normal footnotes and endnotes, and with `font_chains` the measurement.
fn region_request(package: &S9PackageWire, font_chains: Option<&BTreeMap<String, Vec<u32>>>) -> Result<Value> {
    let body = &package.document;
    let mut sections = body
        .sections
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|section| {
            let properties = serde_json::to_value(&section.properties)?;
            let id = section.id.clone().or_else(|| properties["sectionId"].as_str().map(str::to_owned));
            Ok(json!({ "sectionId": id, "properties": properties }))
        })
        .collect::<Result<Vec<_>>>()?;
    let last = body.final_section_properties.as_ref().map(serde_json::to_value).transpose()?.unwrap_or_else(|| json!({}));
    sections.push(json!({ "sectionId": last["sectionId"].as_str(), "properties": last }));
    let watermark = sections.last().and_then(|s| s["properties"].get("watermark")).cloned().unwrap_or(Value::Null);
    let mut notes = Vec::new();
    for (list, kind) in [(package.footnotes.as_deref(), "footnote"), (package.endnotes.as_deref(), "endnote")] {
        notes.extend(list.unwrap_or_default().iter().filter(|n| !n.is_separator()).map(|n| json!({ "id": n.id as i64, "noteKind": kind, "height": 0 })));
    }
    let compat = &package.settings.compatibility_flags;
    let mut request = json!({
        "bodyStory": "body",
        "options": { "pageGap": 24 },
        "regions": { "sections": sections, "settings": package.settings, "watermark": watermark },
        "notes": { "contents": notes },
        "renderEnv": {}
    });
    if let Some(chains) = font_chains {
        request["measurement"] = json!({
            "fontChains": chains,
            "defaults": { "fontSize": 11, "fontFamily": "Calibri" },
            "compat": { "noLeading": compat.no_leading, "doNotExpandShiftReturn": compat.do_not_expand_shift_return },
            "authoritativeShaping": true
        });
    }
    Ok(request)
}

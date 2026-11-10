//! A Word file's pictures, decoded once, by the scoped keys its display list names them with.
//! After BetterOffice's native viewer (`images.rs`, Apache-2.0).

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Cursor;

use anyhow::Result;
use base64::Engine as _;
use docx_parse::{
    RelationshipTarget, is_footer_relationship, is_header_relationship, is_image_relationship,
    parse_docx_relationship_parts, resolve_relationship_target,
};
use docx_raster::{
    ImageScope, MAX_DATA_URL_BYTES, MAX_IMAGE_BYTES, MAX_IMAGE_PIXELS, NoteKind,
    scoped_image_key,
};
use vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};

pub struct ImageRegistry {
    decoded: HashMap<String, ImageData>,
    data_urls: RefCell<HashMap<String, Option<ImageData>>>,
}

impl ImageRegistry {
    pub fn load(docx: &[u8]) -> Result<Self> {
        let parts = ooxml_opc::unzip_parts(docx).map_err(anyhow::Error::msg)?;
        let package = parts
            .into_iter()
            .map(|(path, bytes)| (path.to_lowercase(), bytes))
            .collect::<HashMap<_, _>>();
        let relationships = parse_docx_relationship_parts(docx)?;
        let body_part = relationships.relationship_parts.iter().find(|part| {
            part.path
                .eq_ignore_ascii_case("word/_rels/document.xml.rels")
        });
        let mut header_footer_ids = HashMap::new();
        if let Some(body_part) = body_part {
            for (_, relationship) in &body_part.relationships {
                if !(is_header_relationship(relationship) || is_footer_relationship(relationship)) {
                    continue;
                }
                if let RelationshipTarget::Internal(target) =
                    resolve_relationship_target(&body_part.path, relationship)?
                {
                    header_footer_ids.insert(target.to_lowercase(), relationship.id.clone());
                }
            }
        }

        let mut decoded = HashMap::new();
        for part in &relationships.relationship_parts {
            let Some(owner) = relationship_owner(&part.path) else {
                continue;
            };
            let owner_lower = owner.to_lowercase();
            for (_, relationship) in &part.relationships {
                if !is_image_relationship(relationship) {
                    continue;
                }
                let RelationshipTarget::Internal(target) =
                    resolve_relationship_target(&part.path, relationship)?
                else {
                    continue;
                };
                let Some(bytes) = package.get(&target.to_lowercase()) else {
                    continue;
                };
                let key = if owner_lower == "word/document.xml" {
                    scoped_image_key(ImageScope::Body, &relationship.id)
                } else if owner_lower == "word/footnotes.xml" {
                    scoped_image_key(ImageScope::Notes(NoteKind::Footnote), &relationship.id)
                } else if owner_lower == "word/endnotes.xml" {
                    scoped_image_key(ImageScope::Notes(NoteKind::Endnote), &relationship.id)
                } else if let Some(r_id) = header_footer_ids.get(&owner_lower) {
                    scoped_image_key(ImageScope::HeaderFooter(r_id), &relationship.id)
                } else {
                    continue;
                };
                if let Some(image) = decode_image(bytes) {
                    decoded.insert(key, image);
                }
            }
        }
        Ok(Self {
            decoded,
            data_urls: RefCell::new(HashMap::new()),
        })
    }

    pub fn get(&self, scope: ImageScope<'_>, rel_id: &str) -> Option<ImageData> {
        if rel_id.starts_with("data:") {
            if let Some(image) = self.data_urls.borrow().get(rel_id) {
                return image.clone();
            }
            let image = decode_data_url(rel_id);
            self.data_urls
                .borrow_mut()
                .insert(rel_id.to_owned(), image.clone());
            return image;
        }
        let key = scoped_image_key(scope, rel_id);
        self.decoded.get(&key).cloned()
    }
}

fn decode_data_url(source: &str) -> Option<ImageData> {
    let (metadata, payload) = source.split_once(',')?;
    if !metadata.ends_with(";base64") {
        return None;
    }
    let declared = payload.len() as u64 / 4 * 3;
    if declared > MAX_DATA_URL_BYTES {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .ok()?;
    decode_image(&bytes)
}

fn decode_image(bytes: &[u8]) -> Option<ImageData> {
    use image::ImageDecoder as _;

    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    if reader.format() == Some(image::ImageFormat::Tiff)
        && ooxml_drawingml::media::tiff_bounded_pixels(bytes).is_none()
    {
        return None;
    }
    let mut decoder = reader.into_decoder().ok()?;
    let (declared_width, declared_height) = decoder.dimensions();
    let declared = u64::from(declared_width) * u64::from(declared_height);
    if declared > MAX_IMAGE_PIXELS
        || decoder
            .total_bytes()
            .saturating_add(declared.saturating_mul(4))
            > MAX_IMAGE_BYTES
    {
        return None;
    }
    let orientation = decoder.orientation().ok()?;
    let mut image = image::DynamicImage::from_decoder(decoder).ok()?;
    image.apply_orientation(orientation);
    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    Some(ImageData {
        data: Blob::from(rgba.into_raw()),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width,
        height,
    })
}

fn relationship_owner(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    if let Some((directory, file)) = normalized.rsplit_once("/_rels/") {
        return Some(format!("{directory}/{}", file.strip_suffix(".rels")?));
    }
    let file = normalized.strip_prefix("_rels/")?;
    Some(file.strip_suffix(".rels")?.to_owned())
}

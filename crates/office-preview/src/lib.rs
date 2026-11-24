//! Read-only previews of Word, Excel and PowerPoint files, as Desktop's Microsoft Office plugin
//! shows them (`gui-plugins/src/microsoft-office`). BetterOffice's engines open and lay each file
//! out, and every page, slide or sheet frame becomes a Vello scene: the display list drawn as
//! vectors, so a GPU paints it sharp at any scale and zoom. Each format follows BetterOffice's
//! native viewer (`apps/native-viewer`, Apache-2.0), which translates the same display lists.
//!
//! A document's engines keep per-thread state (Word's measurement fonts), so open each document
//! on a thread of its own and keep it there.

pub mod cjk;
mod docx;
pub mod engine;
pub mod fonts;
mod pptx;
mod shared;
pub mod xlsx;

pub use shared::{PageScene, Skipped};

use anyhow::Result;

use fonts::FontPacks;

/// An Office file's format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Document,
    Spreadsheet,
    Presentation,
}

impl Kind {
    pub fn of(path: &str) -> Option<Kind> {
        match path.rsplit('.').next()?.to_ascii_lowercase().as_str() {
            "docx" => Some(Kind::Document),
            "xlsx" => Some(Kind::Spreadsheet),
            "pptx" => Some(Kind::Presentation),
            _ => None,
        }
    }
}

/// A page or slide as laid out: its size in CSS px, and a slide's visibility and speaker notes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageInfo {
    pub width: f64,
    pub height: f64,
    pub hidden: bool,
    pub notes: String,
}

/// An open file.
pub enum Document {
    Word(Box<docx::Docx>),
    PowerPoint(Box<pptx::Pptx>),
    Excel(Box<xlsx::Xlsx>),
}

impl Document {
    /// Opens and lays out `bytes`; a workbook calculates with `now_serial` as the local time
    /// (`xlsx::Xlsx::open`). Blocking: run it on the document's own thread.
    pub fn open(kind: Kind, bytes: &[u8], packs: &FontPacks, now_serial: Option<f64>) -> Result<Document> {
        Ok(match kind {
            Kind::Document => Document::Word(Box::new(docx::Docx::open(bytes, packs)?)),
            Kind::Presentation => Document::PowerPoint(Box::new(pptx::Pptx::open(bytes, packs)?)),
            Kind::Spreadsheet => Document::Excel(Box::new(xlsx::Xlsx::open(bytes, now_serial)?)),
        })
    }

    pub fn kind(&self) -> Kind {
        match self {
            Document::Word(_) => Kind::Document,
            Document::PowerPoint(_) => Kind::Presentation,
            Document::Excel(_) => Kind::Spreadsheet,
        }
    }

    /// The pages or slides; a workbook has sheets instead.
    pub fn pages(&self) -> Vec<PageInfo> {
        match self {
            Document::Word(d) => d.pages(),
            Document::PowerPoint(p) => p.pages(),
            Document::Excel(_) => Vec::new(),
        }
    }

    /// Page or slide `page` as a scene.
    pub fn scene(&self, page: usize) -> Result<PageScene> {
        match self {
            Document::Word(d) => d.scene(page),
            Document::PowerPoint(p) => p.scene(page),
            Document::Excel(_) => anyhow::bail!("a workbook draws frames, not pages"),
        }
    }

    /// The file needs the Chinese, Japanese and Korean faces to show as written.
    pub fn wants_cjk(&self) -> bool {
        match self {
            Document::Word(d) => d.wants_cjk,
            Document::PowerPoint(p) => p.wants_cjk,
            Document::Excel(_) => false,
        }
    }
}

fn cjk_requested(script: &str) -> bool {
    script.starts_with("cjk")
}

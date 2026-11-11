//! One thread per open file, as Desktop gives each its own worker (`office-worker.ts`): it opens
//! the file, then answers for pages, slides and sheet frames one job at a time, newest first. Its
//! engines keep per-thread state, so the document never leaves it. Dropping the job sender ends it.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use office_preview::fonts::FontPacks;
use office_preview::xlsx::{CellText, Range, SheetLayout};
use office_preview::{Document, Kind, PageInfo};

use super::sheet::state::Purpose;

/// What the UI asks a document's thread for.
pub enum Job {
    /// Page or slide `page`'s scene.
    Scene { page: usize },
    /// Sheet `sheet`'s extent and panes.
    Layout { sheet: usize },
    /// The cells of sheet `sheet` in a viewport, CSS px: x, y, width, height.
    /// `ratio`: device pixels per CSS px, which the frame's lines snap to (`canvas2d.ts`).
    Frame { sheet: usize, viewport: [f64; 4], ratio: f64, ticket: u64 },
    /// The cells of a range of sheet `sheet` as they show, for a copy or screen readers.
    Cells { sheet: usize, range: Range, ticket: u64, purpose: Purpose },
    /// Where a cell of sheet `sheet` is, to scroll it into view.
    Position { sheet: usize, cell: [u32; 2], ticket: u64 },
    /// Opens the file again with these faces (the CJK pack arrived), then answers as at first.
    Reopen(FontPacks),
}

/// What the thread answers, tagged with its document.
pub enum Reply {
    Opened(Opened),
    Failed(String),
    Scene { page: usize, scene: Result<Arc<Scene>, String> },
    Layout { sheet: usize, layout: Result<SheetLayout, String> },
    Frame { sheet: usize, ticket: u64, frame: Result<Arc<SheetFrame>, String> },
    Cells { sheet: usize, range: Range, ticket: u64, cells: Result<Vec<Vec<CellText>>, String> },
    /// A copy's cells as tab-separated text and an HTML table, written here off the UI thread.
    Copied { sheet: usize, ticket: u64, copy: Result<(String, String), String> },
    Position { sheet: usize, ticket: u64, rect: Result<[f64; 4], String> },
}

pub struct Opened {
    pub pages: Vec<PageInfo>,
    pub sheets: Vec<String>,
    pub wants_cjk: bool,
}

/// A page's scene as the compositor draws it: CSS px, its paper included.
pub struct Scene {
    pub scene: vello::Scene,
    pub width: f64,
    pub height: f64,
}

/// One viewport of a sheet: its scene, the grid its headers follow and the links in it.
pub struct SheetFrame {
    pub scene: Scene,
    pub grid: office_preview::xlsx::GridMeta,
    pub links: Vec<office_preview::xlsx::HyperlinkRegion>,
    pub viewport: [f64; 4],
}

/// Starts the thread for `bytes`; its answers come on `replies` and each one calls `wake`.
pub fn start(kind: Kind, bytes: Vec<u8>, packs: FontPacks, replies: Sender<Reply>, wake: Arc<dyn Fn() + Send + Sync>) -> Option<Sender<Job>> {
    let (jobs, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new().name("office-document".into()).spawn(move || run(kind, &bytes, packs, &rx, &replies, &*wake)).ok()?;
    Some(jobs)
}

fn run(kind: Kind, bytes: &[u8], packs: FontPacks, jobs: &Receiver<Job>, replies: &Sender<Reply>, wake: &(dyn Fn() + Send + Sync)) {
    let send = |r: Reply| {
        let ok = replies.send(r).is_ok();
        wake();
        ok
    };
    let Some(mut document) = open(kind, bytes, &packs, &send) else { return };
    while let Ok(first) = jobs.recv() {
        // Jobs that queued while the last one ran: a newer frame of a sheet supersedes older ones.
        let mut batch = vec![first];
        batch.extend(jobs.try_iter());
        let newest = batch.iter().rev().find_map(|j| if let Job::Frame { ticket, .. } = j { Some(*ticket) } else { None });
        for job in batch {
            let alive = match job {
                Job::Scene { page } => send(Reply::Scene { page, scene: document.scene(page).map(|p| Arc::new(Scene { scene: p.scene, width: p.width, height: p.height })).map_err(|e| e.to_string()) }),
                Job::Layout { sheet } => send(Reply::Layout { sheet, layout: excel(&mut document).and_then(|x| x.layout(sheet).map_err(|e| e.to_string())) }),
                Job::Frame { ticket, .. } if Some(ticket) != newest => true,
                Job::Frame { sheet, viewport, ratio, ticket } => {
                    let frame = excel(&mut document).and_then(|x| x.frame(sheet, viewport[0], viewport[1], viewport[2], viewport[3], ratio).map_err(|e| e.to_string()));
                    send(Reply::Frame { sheet, ticket, frame: frame.map(|f| Arc::new(SheetFrame { scene: Scene { scene: f.page.scene, width: f.page.width, height: f.page.height }, grid: f.grid, links: f.links, viewport })) })
                }
                Job::Cells { sheet, range, ticket, purpose } => {
                    let cells = excel(&mut document).and_then(|x| x.cells(sheet, range).map_err(|e| e.to_string()));
                    send(match purpose {
                        Purpose::Read => Reply::Cells { sheet, range, ticket, cells },
                        Purpose::Copy => Reply::Copied { sheet, ticket, copy: cells.map(|c| (super::sheet::copy::tsv(&c), super::sheet::copy::html(&c))) },
                    })
                }
                Job::Position { sheet, cell, ticket } => send(Reply::Position { sheet, ticket, rect: excel(&mut document).and_then(|x| x.position(sheet, cell[0], cell[1]).map_err(|e| e.to_string())) }),
                Job::Reopen(packs) => match open(kind, bytes, &packs, &send) {
                    Some(reopened) => {
                        document = reopened;
                        true
                    }
                    None => false,
                },
            };
            if !alive {
                return;
            }
        }
    }
}

fn open(kind: Kind, bytes: &[u8], packs: &FontPacks, send: &dyn Fn(Reply) -> bool) -> Option<Document> {
    match Document::open(kind, bytes, packs, now_serial()) {
        Ok(document) => {
            let sheets = match &document {
                Document::Excel(x) => x.sheets.clone(),
                _ => Vec::new(),
            };
            send(Reply::Opened(Opened { pages: document.pages(), sheets, wants_cjk: document.wants_cjk() })).then_some(document)
        }
        Err(e) => {
            send(Reply::Failed(format!("{e:#}")));
            None
        }
    }
}

/// The local time as a 1900-system serial (`xlsx-wasm` `now_serial`): days since 1899-12-30.
fn now_serial() -> Option<f64> {
    let ms = crate::wall_ms();
    let local = ms + i64::from(rune::LocalTime::at(ms).offset) * 1000;
    Some(local as f64 / 86_400_000.0 + 25_569.0)
}

fn excel(document: &mut Document) -> Result<&mut office_preview::xlsx::Xlsx, String> {
    match document {
        Document::Excel(x) => Ok(x.as_mut()),
        _ => Err("not a workbook".into()),
    }
}

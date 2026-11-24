//! One thread per open file, as Desktop gives each its own worker (`office-worker.ts`): it opens
//! the file, then answers for pages, slides and sheet frames one job at a time, newest first. Its
//! engines keep per-thread state, so the document never leaves it. Dropping the `Worker` ends it.
//!
//! This is all an app calls of the engines, so it is also the line a build can cut them off at:
//! `crates/office-library` exports `start`, `send` and `close` from a library of its own, and
//! the app then carries none of the engines' code or fonts in its exe (the system reads every
//! byte of a new exe before its first run; a library is read when it is first loaded). `Host`,
//! `Api` and `Abi` are that library's C interface. Rust values cross it, so both sides must
//! come from one build: `Abi` is how the library tells, before anything crosses.

#![allow(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use crate::fonts::FontPacks;
use crate::xlsx::{CellText, GridMeta, HyperlinkRegion, Range, SheetLayout, Xlsx};
use crate::{Document, Kind, PageInfo};

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
    /// Opens the file again with the CJK pack in `cjk` (it arrived), then answers as at first.
    Reopen { cjk: Option<PathBuf>, now_serial: Option<f64> },
}

/// What a read of cells is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// The block screen readers browse (`cells`).
    Read,
    /// A copy (`copy`).
    Copy,
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
    pub grid: GridMeta,
    pub links: Vec<HyperlinkRegion>,
    pub viewport: [f64; 4],
}

/// A file to open.
pub struct Start {
    pub kind: Kind,
    pub bytes: Vec<u8>,
    /// The CJK pack's folder (`cjk::dir`) when it is installed.
    pub cjk: Option<PathBuf>,
    /// The local time as a 1900-system serial, which a workbook calculates with.
    pub now_serial: Option<f64>,
    /// Takes each answer, on the document's thread; false when nobody listens any more.
    pub reply: Box<dyn Fn(Reply) -> bool + Send>,
}

/// A document's thread. Dropping it ends the thread after the job it is on.
pub struct Worker {
    jobs: Sender<Job>,
}

impl Worker {
    /// Queues `job`; false when the thread has ended.
    pub fn send(&self, job: Job) -> bool {
        self.jobs.send(job).is_ok()
    }
}

/// Starts the thread for `start.bytes`; `None` when the system gave no thread.
pub fn start(start: Start) -> Option<Worker> {
    let (jobs, rx) = std::sync::mpsc::channel();
    // Word's parser recurses once per nested table or content control (~40 KB a level) up to its
    // 64-level cap (~2.80 MiB), which overflowed Rust's 2 MiB default worker stack at 41 levels.
    // Address space only; at most two document threads stay open (`machine.KEPT`).
    std::thread::Builder::new().name("office-document".into()).stack_size(8 * 1024 * 1024).spawn(move || run(start, &rx)).ok()?;
    Some(Worker { jobs })
}

fn run(start: Start, jobs: &Receiver<Job>) {
    let Start { kind, bytes, cjk, now_serial, reply } = start;
    let send = |r: Reply| reply(r);
    let Some(mut document) = open(kind, &bytes, cjk, now_serial, &send) else { return };
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
                        Purpose::Copy => Reply::Copied { sheet, ticket, copy: cells.map(|c| (crate::xlsx::copy::tsv(&c), crate::xlsx::copy::html(&c))) },
                    })
                }
                Job::Position { sheet, cell, ticket } => send(Reply::Position { sheet, ticket, rect: excel(&mut document).and_then(|x| x.position(sheet, cell[0], cell[1]).map_err(|e| e.to_string())) }),
                Job::Reopen { cjk, now_serial } => match open(kind, &bytes, cjk, now_serial, &send) {
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

fn open(kind: Kind, bytes: &[u8], cjk: Option<PathBuf>, now_serial: Option<f64>, send: &dyn Fn(Reply) -> bool) -> Option<Document> {
    match Document::open(kind, bytes, &FontPacks::new(cjk.as_deref()), now_serial) {
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

fn excel(document: &mut Document) -> Result<&mut Xlsx, String> {
    match document {
        Document::Excel(x) => Ok(x.as_mut()),
        _ => Err("not a workbook".into()),
    }
}

/// The name of the one function the library exports: `Entry`.
pub const ENTRY: &str = "opencode_office_v1";

/// The library's entry. It answers null, and touches nothing, when `abi` is not its own.
pub type Entry = unsafe extern "C" fn(host: *const Host, abi: *const Abi) -> *const Api;

/// What the app hands the library: its allocator. Every allocation of the library goes through
/// it, so a value one side made the other may drop, and the app's memory counters see both.
#[repr(C)]
pub struct Host {
    pub alloc: unsafe extern "C" fn(size: usize, align: usize) -> *mut u8,
    pub alloc_zeroed: unsafe extern "C" fn(size: usize, align: usize) -> *mut u8,
    pub dealloc: unsafe extern "C" fn(ptr: *mut u8, size: usize, align: usize),
    pub realloc: unsafe extern "C" fn(ptr: *mut u8, size: usize, align: usize, new_size: usize) -> *mut u8,
}

/// What the library hands back: `start`, `Worker::send` and the drop of a `Worker`.
#[repr(C)]
pub struct Api {
    /// Takes the box. Null when no thread started.
    pub start: unsafe extern "C" fn(start: *mut Start) -> *mut Worker,
    /// Takes the job's box.
    pub send: unsafe extern "C" fn(worker: *const Worker, job: *mut Job) -> bool,
    /// Takes the worker's box.
    pub close: unsafe extern "C" fn(worker: *mut Worker),
}

/// What must be equal on both sides before a Rust value may cross: the build (the app's version
/// as its build stamps it) and the sizes of what crosses.
#[repr(C)]
pub struct Abi {
    pub interface: u32,
    pub sizes: [usize; 5],
    pub build: *const u8,
    pub build_len: usize,
}

const BUILD: &str = match option_env!("OPENCODE_RUNE_VERSION") {
    Some(version) => version,
    None => "unversioned",
};

/// This build's `Abi`.
pub fn abi() -> Abi {
    Abi {
        interface: 1,
        sizes: [size_of::<Start>(), size_of::<Job>(), size_of::<Reply>(), size_of::<Scene>(), size_of::<SheetFrame>()],
        build: BUILD.as_ptr(),
        build_len: BUILD.len(),
    }
}

impl Abi {
    /// Whether `other` (the other side's, which lives as long as the call) is this build's.
    ///
    /// # Safety
    /// `other.build` must point at `other.build_len` readable bytes.
    pub unsafe fn matches(&self, other: &Abi) -> bool {
        // SAFETY: the caller's contract for `other`; `self` is `abi()`'s, a `&'static str`.
        let (mine, theirs) = unsafe { (std::slice::from_raw_parts(self.build, self.build_len), std::slice::from_raw_parts(other.build, other.build_len)) };
        self.interface == other.interface && self.sizes == other.sizes && mine == theirs
    }
}

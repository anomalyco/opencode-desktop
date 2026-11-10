//! Every open Office file of the app, shared by its windows (they run on one thread): each file's
//! state machine (opening, open, failed), the scenes its pages and sheet frames have, and the
//! effect-slot marks the compositor finds them by. The windows poll it; a file's thread wakes the
//! window that opened it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use office_preview::xlsx::SheetLayout;
use office_preview::{Kind, PageInfo};
use rune_luau::{Completer, Data};

use super::worker::{self, Job, Reply, Scene, SheetFrame};

/// Effect-slot marks of Office pages share this upper half; the lower half numbers the page.
const MARK_TAG: u32 = 0x4F46 << 16;

/// Pages either side of the ones on screen that keep their scenes (Desktop's `keptAround`).
const KEPT_AROUND: usize = 10;

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
}

#[derive(Default)]
struct Registry {
    docs: HashMap<u64, Doc>,
    marks: HashMap<u32, (u64, Target)>,
    by_target: HashMap<(u64, Target), u32>,
    next_doc: u64,
    next_mark: u32,
}

/// The windows that opened a file, woken when a file thread or the font fetch has news.
static WAKERS: std::sync::Mutex<Vec<rune::Waker>> = std::sync::Mutex::new(Vec::new());

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Target {
    Page(usize),
    Sheet,
}

struct Doc {
    kind: Kind,
    state: State,
    jobs: std::sync::mpsc::Sender<Job>,
    replies: Receiver<Reply>,
    /// Grows with every change a view of the file draws.
    version: u64,
    zoom: f32,
    /// The width its page column lays out in, once measured.
    width: f32,
}

/// A file's lifecycle (`office-worker.ts`'s `worker-machine.ts`): opening, then open or failed.
enum State {
    Opening { waiters: Vec<(Completer, usize)> },
    Open(Open),
    Failed,
}

struct Open {
    pages: Vec<PageInfo>,
    scenes: HashMap<usize, Slot>,
    sheet: Sheet,
}

/// A page's scene: asked for, drawn, or failed.
enum Slot {
    Asked,
    Ready(Arc<Scene>),
    Failed,
}

/// The sheet a workbook shows, its extent, and its latest frame.
#[derive(Default)]
struct Sheet {
    index: usize,
    layout: Option<SheetLayout>,
    frame: Option<Arc<SheetFrame>>,
    asked: Option<[f64; 4]>,
    /// The device pixels per CSS px the sheet last drew at, which frames snap their lines to.
    ratio: f64,
    ticket: u64,
}

/// What the compositor draws for a mark: a page's scene or a sheet's frame, and a version that
/// changes when it does.
pub enum PageDraw {
    Page(Arc<Scene>),
    Sheet(Arc<SheetFrame>),
}

impl PageDraw {
    pub fn scene(&self) -> &Scene {
        match self {
            PageDraw::Page(scene) => scene,
            PageDraw::Sheet(frame) => &frame.scene,
        }
    }

    pub fn version(&self) -> u64 {
        match self {
            PageDraw::Page(scene) => Arc::as_ptr(scene) as u64,
            PageDraw::Sheet(frame) => Arc::as_ptr(frame) as u64,
        }
    }
}
/// What a view of a file draws: its pages or sheets, whether it failed, and its zoom.
#[derive(Clone, Default)]
pub struct Snapshot {
    pub pages: Vec<PageInfo>,
    pub failed_pages: Vec<usize>,
    pub zoom: f32,
    pub width: f32,
    pub layout: Option<SheetLayout>,
    pub frame: Option<Arc<SheetFrame>>,
}

/// Starts opening `bytes` (`size` bytes long, for the answer) and answers `done` once it opened.
pub fn open(kind: Kind, bytes: Vec<u8>, size: usize, waker: Option<rune::Waker>, done: Completer) {
    let packs = super::cjk::packs();
    let (tx, replies) = std::sync::mpsc::channel();
    if let (Some(w), Ok(mut all)) = (waker.clone(), WAKERS.lock()) {
        all.push(w);
    }
    let wake: Arc<dyn Fn() + Send + Sync> = match waker {
        Some(w) => Arc::new(move || w()),
        None => Arc::new(|| {}),
    };
    let Some(jobs) = worker::start(kind, bytes, packs, tx, wake) else {
        done.complete(answer(size, None, Some("The file could not be opened")));
        return;
    };
    REGISTRY.with_borrow_mut(|r| {
        r.next_doc += 1;
        let id = r.next_doc;
        r.docs.insert(id, Doc { kind, state: State::Opening { waiters: vec![(done, size)] }, jobs, replies, version: 1, zoom: 1.0, width: 0.0 });
    });
}

/// Closes file `id`: its thread ends, and its scenes and marks go.
pub fn close(id: u64) {
    REGISTRY.with_borrow_mut(|r| {
        r.docs.remove(&id);
        r.marks.retain(|_, (doc, _)| *doc != id);
        r.by_target.retain(|(doc, _), _| *doc != id);
    });
}

/// Takes every file thread's answers; true when a view should draw again.
pub fn poll() -> bool {
    REGISTRY.with_borrow_mut(|r| {
        let mut changed = false;
        for (id, doc) in r.docs.iter_mut() {
            while let Ok(reply) = doc.replies.try_recv() {
                changed = true;
                doc.version += 1;
                step(*id, doc, reply);
            }
        }
        changed
    })
}

/// File `id`'s state transition for a reply from its thread.
fn step(id: u64, doc: &mut Doc, reply: Reply) {
    match (&mut doc.state, reply) {
        (State::Opening { waiters }, Reply::Opened(opened)) => {
            if opened.wants_cjk {
                super::cjk::want();
            }
            let record = document(id, doc.kind, &opened.pages, &opened.sheets);
            for (done, size) in waiters.drain(..) {
                done.complete(answer(size, Some(record.clone()), None));
            }
            if doc.kind == Kind::Spreadsheet && !opened.sheets.is_empty() {
                doc.jobs.send(Job::Layout { sheet: 0 }).ok();
            }
            doc.state = State::Open(Open { pages: opened.pages, scenes: HashMap::new(), sheet: Sheet::default() });
        }
        (State::Opening { waiters }, Reply::Failed(reason)) => {
            eprintln!("office: {reason}");
            for (done, size) in waiters.drain(..) {
                done.complete(answer(size, None, Some(&reason)));
            }
            doc.state = State::Failed;
        }
        // Opened again with the CJK faces: the pages lay out anew.
        (State::Open(open), Reply::Opened(opened)) => {
            open.pages = opened.pages;
            open.scenes.clear();
            open.sheet.frame = None;
            open.sheet.asked = None;
        }
        (State::Open(open), Reply::Scene { page, scene }) => {
            open.scenes.insert(page, scene.map_or(Slot::Failed, Slot::Ready));
        }
        (State::Open(open), Reply::Layout { sheet, layout }) if sheet == open.sheet.index => {
            open.sheet.layout = layout.ok();
        }
        (State::Open(open), Reply::Frame { sheet, ticket, frame }) if sheet == open.sheet.index && ticket == open.sheet.ticket => match frame {
            Ok(frame) => open.sheet.frame = Some(frame),
            Err(e) => eprintln!("office: a sheet frame failed: {e}"),
        },
        _ => {}
    }
}

/// The record `Office:OpenAsync` resumes with.
fn answer(size: usize, document: Option<Data>, error: Option<&str>) -> Data {
    crate::plugin_host::luau::services::map([
        ("size", Data::Integer(size as i64)),
        ("signature", Data::String("zip".into())),
        ("document", document.unwrap_or(Data::Nil)),
        ("error", error.map_or(Data::Nil, |e| Data::String(e.to_owned()))),
    ])
}

fn document(id: u64, kind: Kind, pages: &[PageInfo], sheets: &[String]) -> Data {
    let kind = match kind {
        Kind::Document => "docx",
        Kind::Spreadsheet => "xlsx",
        Kind::Presentation => "pptx",
    };
    crate::plugin_host::luau::services::map([
        ("id", Data::Integer(id as i64)),
        ("kind", Data::String(kind.into())),
        ("pages", Data::Integer(pages.len() as i64)),
        ("sheets", Data::List(sheets.iter().cloned().map(Data::String).collect())),
    ])
}

/// What a view of file `id` draws.
pub fn snapshot(id: u64) -> Option<Snapshot> {
    REGISTRY.with_borrow(|r| {
        let doc = r.docs.get(&id)?;
        let State::Open(open) = &doc.state else { return Some(Snapshot { zoom: doc.zoom, width: doc.width, ..Snapshot::default() }) };
        let failed_pages = open.scenes.iter().filter(|(_, s)| matches!(s, Slot::Failed)).map(|(i, _)| *i).collect();
        Some(Snapshot {
            pages: open.pages.clone(),
            failed_pages,
            zoom: doc.zoom,
            width: doc.width,
            layout: open.sheet.layout,
            frame: open.sheet.frame.clone(),
        })
    })
}

/// File `id`'s version, which grows with every change its views draw.
pub fn version(id: u64) -> u64 {
    REGISTRY.with_borrow(|r| r.docs.get(&id).map_or(0, |d| d.version))
}

/// The mark page `page` of file `id` draws its slot with.
pub fn page_mark(id: u64, page: usize) -> u32 {
    mark_for(id, Target::Page(page))
}

/// The mark file `id`'s sheet body draws its slot with.
pub fn sheet_mark(id: u64) -> u32 {
    mark_for(id, Target::Sheet)
}

fn mark_for(id: u64, target: Target) -> u32 {
    REGISTRY.with_borrow_mut(|r| {
        if let Some(mark) = r.by_target.get(&(id, target)) {
            return *mark;
        }
        r.next_mark = (r.next_mark + 1) & 0xFFFF;
        let mark = MARK_TAG | r.next_mark;
        if let Some(old) = r.marks.insert(mark, (id, target)) {
            r.by_target.remove(&old);
        }
        r.by_target.insert((id, target), mark);
        mark
    })
}

pub fn is_mark(mark: u32) -> bool {
    mark & 0xFFFF_0000 == MARK_TAG
}

/// What the compositor draws for `mark`, once it has it.
pub fn scene(mark: u32) -> Option<PageDraw> {
    REGISTRY.with_borrow(|r| {
        let (id, target) = r.marks.get(&mark)?;
        let State::Open(open) = &r.docs.get(id)?.state else { return None };
        match target {
            Target::Page(page) => match open.scenes.get(page)? {
                Slot::Ready(scene) => Some(PageDraw::Page(scene.clone())),
                _ => None,
            },
            Target::Sheet => open.sheet.frame.clone().map(PageDraw::Sheet),
        }
    })
}
/// Pages `from..to` of file `id` are on screen or near it: asks for their scenes and lets go of
/// the far ones.
pub fn show_pages(id: u64, from: usize, to: usize) {
    REGISTRY.with_borrow_mut(|r| {
        let Some(doc) = r.docs.get_mut(&id) else { return };
        let State::Open(open) = &mut doc.state else { return };
        let to = to.min(open.pages.len());
        for page in from..to {
            if !open.scenes.contains_key(&page) && doc.jobs.send(Job::Scene { page }).is_ok() {
                open.scenes.insert(page, Slot::Asked);
            }
        }
        let (keep_from, keep_to) = (from.saturating_sub(KEPT_AROUND), to + KEPT_AROUND);
        open.scenes.retain(|page, slot| (keep_from..keep_to).contains(page) || matches!(slot, Slot::Failed));
    });
}

/// Shows sheet `sheet` of file `id`, its frame drawn anew.
pub fn set_sheet(id: u64, sheet: usize) {
    REGISTRY.with_borrow_mut(|r| {
        let Some(doc) = r.docs.get_mut(&id) else { return };
        let State::Open(open) = &mut doc.state else { return };
        if open.sheet.index == sheet {
            return;
        }
        open.sheet = Sheet { index: sheet, ticket: open.sheet.ticket, ratio: open.sheet.ratio, ..Sheet::default() };
        doc.jobs.send(Job::Layout { sheet }).ok();
        doc.version += 1;
    });
}

/// File `id`'s sheet shows `viewport` (CSS px: scroll x, y, width, height): asks for that frame
/// unless it has it or asked for it.
pub fn show_sheet(id: u64, viewport: [f64; 4]) {
    REGISTRY.with_borrow_mut(|r| {
        let Some(doc) = r.docs.get_mut(&id) else { return };
        let State::Open(open) = &mut doc.state else { return };
        let rounded = viewport.map(f64::round);
        if open.sheet.asked == Some(rounded) || rounded[2] < 1.0 || rounded[3] < 1.0 {
            return;
        }
        open.sheet.asked = Some(rounded);
        open.sheet.ticket += 1;
        let ratio = if open.sheet.ratio > 0.0 { open.sheet.ratio } else { 1.0 };
        doc.jobs.send(Job::Frame { sheet: open.sheet.index, viewport: rounded, ratio, ticket: open.sheet.ticket }).ok();
    });
}

/// The compositor drew `mark` at `ratio` device pixels per CSS px: a sheet whose frame snapped its
/// lines to another ratio asks for the frame again at this one, as Desktop's canvas paints at
/// the device pixel ratio.
pub fn drawn_at(mark: u32, ratio: f64) {
    REGISTRY.with_borrow_mut(|r| {
        let Some((id, Target::Sheet)) = r.marks.get(&mark).copied() else { return };
        let Some(doc) = r.docs.get_mut(&id) else { return };
        let State::Open(open) = &mut doc.state else { return };
        if (open.sheet.ratio - ratio).abs() < 0.001 {
            return;
        }
        open.sheet.ratio = ratio;
        let Some(viewport) = open.sheet.asked else { return };
        open.sheet.ticket += 1;
        doc.jobs.send(Job::Frame { sheet: open.sheet.index, viewport, ratio, ticket: open.sheet.ticket }).ok();
    });
}

/// Changes file `id`'s zoom with `f`; returns whether it changed.
pub fn zoom(id: u64, f: impl FnOnce(f32) -> f32) -> bool {
    REGISTRY.with_borrow_mut(|r| {
        let Some(doc) = r.docs.get_mut(&id) else { return false };
        let next = f(doc.zoom);
        let changed = next != doc.zoom;
        doc.zoom = next;
        doc.version += u64::from(changed);
        changed
    })
}

/// Every open file opens again with the faces the CJK pack brings.
pub fn reopen_all() {
    REGISTRY.with_borrow_mut(|r| {
        let packs = super::cjk::packs();
        for doc in r.docs.values() {
            if matches!(doc.state, State::Open(_)) {
                doc.jobs.send(Job::Reopen(packs.clone())).ok();
            }
        }
    });
}

/// Wakes every window that opened a file, so they draw again. Any thread may call it.
pub fn redraw() {
    if let Ok(all) = WAKERS.lock() {
        all.iter().for_each(|w| w());
    }
}

/// File `id`'s page column lays out `width` wide; returns whether that changed.
pub fn measure(id: u64, width: f32) -> bool {
    REGISTRY.with_borrow_mut(|r| {
        let Some(doc) = r.docs.get_mut(&id) else { return false };
        let changed = (doc.width - width).abs() >= 0.5;
        if changed {
            doc.width = width;
            doc.version += 1;
        }
        changed
    })
}
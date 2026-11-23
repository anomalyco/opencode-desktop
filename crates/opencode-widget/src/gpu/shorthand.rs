//! `wgsl` canvases (`widgets-gpu.md`, The `wgsl` shorthand) on the device live runs share: each
//! a [`widget_gpu::Shorthand`], rendered off the window's thread when the window asks, and shown
//! through the hand-off a run's canvases take ([`Canvases`]), a frame after it asked. The window
//! decides when a canvas renders (its `fps`, `values` and the freeze rule); the worker only
//! renders what it's asked, and nothing when nothing is.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, Weak};

use widget_gpu::{ErrorKind, Shorthand, Uniforms};

use super::{Canvas, Canvases, Device, Lost};

/// A frame of one `wgsl` canvas, as the window asks for it.
#[derive(Clone)]
pub struct Ask {
    pub wgsl: Arc<str>,
    /// A hash of `wgsl`: a new one compiles again.
    pub source: u64,
    /// Physical pixels, [`super::clamp_size`]d.
    pub size: [u32; 2],
    pub uniforms: Uniforms,
}

/// A window's `wgsl` canvases, by id.
pub struct Shorthands {
    shared: Arc<Shared>,
}

struct Shared {
    work: Mutex<Work>,
    turn: Condvar,
    /// Made once the worker has its device (whether the window copies depends on it).
    canvases: OnceLock<Arc<Canvases>>,
    /// Asks made, and those the worker published.
    asked: AtomicU64,
    done: AtomicU64,
}

#[derive(Default)]
struct Work {
    asks: HashMap<String, Ask>,
    /// The canvases on screen, when that changed: the others are dropped.
    keep: Option<HashSet<String>>,
    /// The device was lost: the next one, if any.
    lost: Option<Option<Device>>,
    /// Canvases that stopped: id, why, and whether it's the program's error.
    failed: Vec<(String, String, bool)>,
    closed: bool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Work> {
        self.work.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Lost for Shared {
    fn lost(&self, next: Option<&Device>, _reason: &str, _message: Option<&str>) {
        self.lock().lost = Some(next.cloned());
        self.turn.notify_all();
    }
}

impl Default for Shorthands {
    fn default() -> Self {
        Self::new()
    }
}

impl Shorthands {
    pub fn new() -> Shorthands {
        let shared = Arc::new(Shared { work: Mutex::default(), turn: Condvar::new(), canvases: OnceLock::new(), asked: AtomicU64::new(0), done: AtomicU64::new(0) });
        let worker = shared.clone();
        if let Err(e) = std::thread::Builder::new().name("widget-wgsl".into()).spawn(move || work(&worker)) {
            eprintln!("widget gpu: no thread for wgsl canvases: {e}");
            let canvases = Canvases::new(false);
            canvases.start(None);
            shared.canvases.get_or_init(|| canvases);
        }
        Shorthands { shared }
    }

    /// The canvases as shown, once the worker has its device.
    pub fn canvases(&self) -> Option<Arc<Canvases>> {
        self.shared.canvases.get().cloned()
    }

    /// Canvas `id` renders `ask` (replacing one not taken yet).
    pub fn ask(&self, id: String, ask: Ask) {
        self.shared.lock().asks.insert(id, ask);
        self.shared.asked.fetch_add(1, Ordering::AcqRel);
        self.shared.turn.notify_all();
    }

    /// Only these canvases are on screen: the others are dropped.
    pub fn keep(&self, ids: HashSet<String>) {
        self.shared.lock().keep = Some(ids);
        self.shared.turn.notify_all();
    }

    /// The canvases that stopped since the last call: id, why, and whether it's the program's
    /// error.
    pub fn failed(&self) -> Vec<(String, String, bool)> {
        std::mem::take(&mut self.shared.lock().failed)
    }

    /// An ask the worker hasn't published yet.
    pub fn pending(&self) -> bool {
        self.shared.done.load(Ordering::Acquire) < self.shared.asked.load(Ordering::Acquire)
    }
}

impl Drop for Shorthands {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.turn.notify_all();
    }
}

/// One canvas on the worker.
struct Slot {
    shorthand: Option<Shorthand>,
    last: Ask,
    /// Its `Shorthand`'s version last published, and the version the window sees for it (moving
    /// across a new `Shorthand` too).
    drawn: Option<u64>,
    version: u64,
}

fn work(shared: &Arc<Shared>) {
    let weak: Weak<dyn Lost> = Arc::downgrade(&(shared.clone() as Arc<dyn Lost>));
    super::register(weak);
    let mut device = super::device().map(|(_, d)| d);
    let canvases = shared.canvases.get_or_init(|| Canvases::new(device.as_ref().is_some_and(|d| d.copies))).clone();
    canvases.start(device.as_ref().map(|d| &d.device));
    let mut slots: HashMap<String, Slot> = HashMap::new();
    let mut serial = 0u64;
    loop {
        let (asks, keep, lost, upto) = {
            let mut w = shared.lock();
            while !w.closed && w.asks.is_empty() && w.keep.is_none() && w.lost.is_none() {
                w = shared.turn.wait(w).unwrap_or_else(PoisonError::into_inner);
            }
            if w.closed {
                return;
            }
            (std::mem::take(&mut w.asks), w.keep.take(), w.lost.take(), shared.asked.load(Ordering::Acquire))
        };
        let mut failed = Vec::new();
        let mut render: HashSet<String> = HashSet::new();
        if let Some(next) = lost {
            device = next;
            canvases.start(device.as_ref().map(|d| &d.device));
            if device.is_none() {
                slots.clear();
            }
            for (id, slot) in &mut slots {
                if let (Some(d), Some(s)) = (&device, slot.shorthand.as_mut())
                    && let Err(e) = s.reset(d.device.clone(), d.queue.clone())
                {
                    failed.push((id.clone(), e.message, e.kind == ErrorKind::Program));
                    slot.shorthand = None;
                }
                slot.drawn = None;
                render.insert(id.clone());
            }
        }
        let dropped = keep.is_some_and(|keep| {
            let before = slots.len();
            slots.retain(|id, _| keep.contains(id));
            slots.len() != before
        });
        for (id, ask) in asks {
            let Some(d) = &device else { continue };
            let fresh = slots.get(&id).is_none_or(|s| s.last.source != ask.source);
            if fresh {
                let shorthand = match Shorthand::new(d.device.clone(), d.queue.clone(), d.format, &ask.wgsl) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        failed.push((id.clone(), e.message, e.kind == ErrorKind::Program));
                        None
                    }
                };
                slots.insert(id.clone(), Slot { shorthand, last: ask, drawn: None, version: 0 });
            } else if let Some(s) = slots.get_mut(&id) {
                s.last = ask;
            }
            render.insert(id);
        }
        if let Some(d) = device.as_ref().filter(|_| !render.is_empty() || dropped) {
            canvases.begin();
            for id in &render {
                let Some(slot) = slots.get_mut(id) else { continue };
                let Some(s) = slot.shorthand.as_mut() else { continue };
                let Ask { size: [w, h], uniforms, .. } = slot.last;
                if let Err(e) = s.render(w, h, &uniforms) {
                    failed.push((id.clone(), e.message, e.kind == ErrorKind::Program));
                    slot.shorthand = None;
                }
            }
            let mut shown = HashMap::new();
            for (id, slot) in &mut slots {
                let Some(c) = slot.shorthand.as_ref().and_then(Shorthand::canvas) else { continue };
                if slot.drawn != Some(c.version) {
                    serial += 1;
                    slot.drawn = Some(c.version);
                    slot.version = serial;
                }
                shown.insert(id.clone(), Canvas { texture: c.texture.clone(), opaque: false, version: slot.version });
            }
            canvases.end(Some(&d.queue), shown, HashMap::new());
        }
        if !failed.is_empty() {
            shared.lock().failed.extend(failed);
        }
        shared.done.fetch_max(upto, Ordering::AcqRel);
    }
}

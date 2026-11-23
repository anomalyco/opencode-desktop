//! `wgsl` canvases (`widgets-gpu.md`, The `wgsl` shorthand) on the device live runs share: each
//! a [`widget_gpu::Shorthand`], rendered off the window's thread when the window asks, and shown
//! through the hand-off a run's canvases take ([`Canvases`]), a frame after it asked. The window
//! decides when a canvas renders (its `fps` and `values`); the worker only renders what it's
//! asked, and nothing when nothing is. A `Shorthand` skips a frame while its last is still
//! drawing: the worker tries it again shortly. One too slow (over 50 ms a frame, three in a row)
//! stops on its last frame: the window shows it frozen. One whose frame hangs (still unfinished
//! 250 ms after its submit, `widget_gpu::HANG`) stops with its placeholder for good, a loss after
//! it included: the worker looks at a frame about once a display frame while it may still be
//! drawing. It submits one frame at a time, so it can't be stuck submitting while the GPU hangs.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use widget_gpu::{ErrorKind, HANG, Shorthand, Uniforms};

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
    /// Canvases too slow: stopped on their last frame.
    frozen: Vec<String>,
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

    /// The canvases that stopped on their last frame since the last call, for being too slow.
    pub fn frozen(&self) -> Vec<String> {
        std::mem::take(&mut self.shared.lock().frozen)
    }

    /// An ask the worker hasn't drawn yet.
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
    /// `last` isn't drawn yet.
    pending: bool,
    /// Its `Shorthand` stopped for being too slow, and the window heard so.
    frozen: bool,
    /// Its last frame may still be drawing till then: looked at for a hang.
    watch: Option<Instant>,
    /// Its `Shorthand`'s version last published, and the version the window sees for it (moving
    /// across a new `Shorthand` too).
    drawn: Option<u64>,
    version: u64,
}

fn work(shared: &Arc<Shared>) {
    let weak: Weak<dyn Lost> = Arc::downgrade(&(shared.clone() as Arc<dyn Lost>));
    super::register(weak);
    let made = super::device();
    let mut generation = made.as_ref().map_or(0, |(g, _)| *g);
    let mut device = made.map(|(_, d)| d);
    let canvases = shared.canvases.get_or_init(|| Canvases::new(device.as_ref().is_some_and(|d| d.copies))).clone();
    canvases.start(device.as_ref().map(|d| &d.device));
    let mut slots: HashMap<String, Slot> = HashMap::new();
    let mut serial = 0u64;
    loop {
        // A frame a `Shorthand` skipped (its last still drawing) is tried again shortly; one that
        // may still be drawing is looked at about once a display frame.
        let now = Instant::now();
        let retry = slots.values().any(|s| s.pending);
        let watching = slots.values().any(|s| s.watch.is_some_and(|t| t > now));
        let (asks, keep, lost, upto) = {
            let mut w = shared.lock();
            while !w.closed && w.asks.is_empty() && w.keep.is_none() && w.lost.is_none() {
                if retry || watching {
                    let wait = Duration::from_millis(if retry { 1 } else { 16 });
                    w = shared.turn.wait_timeout(w, wait).unwrap_or_else(PoisonError::into_inner).0;
                    break;
                }
                w = shared.turn.wait(w).unwrap_or_else(PoisonError::into_inner);
            }
            if w.closed {
                return;
            }
            (std::mem::take(&mut w.asks), w.keep.take(), w.lost.take(), shared.asked.load(Ordering::Acquire))
        };
        let (mut failed, mut frozen) = (Vec::new(), Vec::new());
        if let Some(next) = lost {
            // Work running a quarter second at the loss hung (`widget_gpu::HANG_AT_LOSS`): judged
            // before the reset, its stop comes from the next poll.
            for s in slots.values_mut().filter_map(|s| s.shorthand.as_mut()) {
                s.lost();
            }
            device = next;
            generation = super::generation().unwrap_or(generation);
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
                slot.pending = true;
            }
        }
        let dropped = keep.is_some_and(|keep| {
            let before = slots.len();
            slots.retain(|id, _| keep.contains(id));
            slots.len() != before
        });
        for (id, ask) in asks {
            let Some(d) = &device else { continue };
            if slots.get(&id).is_none_or(|s| s.last.source != ask.source) {
                let shorthand = match Shorthand::new(d.device.clone(), d.queue.clone(), d.format, &ask.wgsl) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        failed.push((id.clone(), e.message, e.kind == ErrorKind::Program));
                        None
                    }
                };
                slots.insert(id, Slot { shorthand, last: ask, pending: true, frozen: false, watch: None, drawn: None, version: 0 });
            } else if let Some(s) = slots.get_mut(&id) {
                s.last = ask;
                s.pending = true;
            }
        }
        // Its device was lost: nothing more on it, the worker is about to hear it and start over.
        let drawing = slots.values().any(|s| s.pending) && !super::is_lost(generation);
        if let Some(d) = device.as_ref().filter(|_| drawing || dropped) {
            // A frame waits for no GPU here: one still drawing is skipped and tried again.
            canvases.begin_unsettled();
            for (id, slot) in slots.iter_mut().filter(|(_, s)| s.pending) {
                let Some(s) = slot.shorthand.as_mut() else {
                    slot.pending = false;
                    continue;
                };
                let before = s.canvas().map(|c| c.version);
                let Ask { size: [w, h], uniforms, .. } = slot.last;
                if let Err(e) = s.render(w, h, &uniforms) {
                    failed.push((id.clone(), e.message, e.kind == ErrorKind::Program));
                    slot.shorthand = None;
                    slot.pending = false;
                } else if s.too_slow() {
                    // Stopped on its last frame: a client error, logged, not reported.
                    if !slot.frozen {
                        slot.frozen = true;
                        frozen.push(id.clone());
                        if let Some(e) = s.error() {
                            eprintln!("widget gpu: {}", e.message);
                        }
                    }
                    slot.pending = false;
                } else if let Some(e) = s.error() {
                    // Its frame hung: the placeholder, for good.
                    failed.push((id.clone(), e.message.clone(), e.kind == ErrorKind::Program));
                    slot.shorthand = None;
                    slot.pending = false;
                } else {
                    // Skipped while its last frame still draws: tried again.
                    slot.pending = s.canvas().map(|c| c.version) == before;
                    if !slot.pending {
                        slot.watch = Some(Instant::now() + HANG + Duration::from_millis(500));
                    }
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
        // The frames not drawn just now that may still be drawing: a hang stops them.
        let now = Instant::now();
        for (id, slot) in slots.iter_mut().filter(|(_, s)| !s.pending && s.watch.is_some_and(|t| t > now)) {
            let Some(s) = slot.shorthand.as_mut() else { continue };
            s.poll();
            if !s.too_slow()
                && let Some(e) = s.error()
            {
                failed.push((id.clone(), e.message.clone(), e.kind == ErrorKind::Program));
                slot.shorthand = None;
            }
        }
        if !failed.is_empty() || !frozen.is_empty() {
            let mut w = shared.lock();
            w.failed.extend(failed);
            w.frozen.extend(frozen);
        }
        // Drawn once nothing waits to be tried again: the window keeps drawing frames till then.
        if !slots.values().any(|s| s.pending) {
            shared.done.fetch_max(upto, Ordering::AcqRel);
        }
    }
}

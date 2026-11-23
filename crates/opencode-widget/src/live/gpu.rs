//! A live run's GPU canvases (`widgets-gpu.md`): its batches checked and replayed with
//! `widget-gpu` on the device the window provides ([`provide`]), one for every run, and what that
//! shows published for the window ([`crate::gpu::Canvases`]).
//!
//! The run's thread replays a batch as it's posted, its bytes borrowed from the program in place;
//! one that must wait behind posts for the rate is copied into the queue and replayed when it
//! leaves it (`link`). A device loss reaches every run: each starts over on the new device and the
//! program hears `{ type: "gpu", lost }`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};

use psychoscript::{HostValue, TypedKind};
use serde_json::json;
use widget_gpu::{Arg, Gpu, GpuError, SavedStops};

use crate::gpu::watch::Watched;
use crate::gpu::{Canvas, Canvases, Device, Lost};

type Tap = Box<dyn Fn(&Arg, Option<&[u8]>, bool) + Send + Sync>;

static TAP: Mutex<Option<Tap>> = Mutex::new(None);

/// Sees every batch a run posts, as posted, whatever its shape (its ops; its bytes when an
/// `ArrayBuffer`; whether it rode a frame request): for checks against the shared fixtures.
pub fn tap(f: Option<Tap>) {
    *TAP.lock().unwrap_or_else(PoisonError::into_inner) = f;
}

/// A posted batch (`None`: a bare frame request), to the tap if one is set.
pub(super) fn tapped(batch: Option<&HostValue>, frame: bool) {
    let Some(f) = &*TAP.lock().unwrap_or_else(PoisonError::into_inner) else { return };
    let Some(batch) = batch else { return f(&Arg::List(Vec::new()), Some(&[]), frame) };
    let field = |k: &str| <HostValue as crate::draw::Arg>::field(batch, k);
    let ops = field("ops").map_or(Arg::Null, arg);
    match field("bytes") {
        Some(HostValue::TypedArray(t)) if t.kind() == TypedKind::ArrayBuffer => t.with_bytes(|b| f(&ops, Some(b), frame)),
        _ => f(&ops, None, frame),
    }
}

/// One live run's GPU.
pub struct RunGpu {
    pub canvases: Arc<Canvases>,
    inner: Mutex<Inner>,
    /// Tells the program of a loss: `{ type: "gpu", lost, message? }`.
    tell: Box<dyn Fn(serde_json::Value) + Send + Sync>,
    /// Wakes the window: something it shows changed.
    notify: Arc<dyn Fn() + Send + Sync>,
}

struct Inner {
    gpu: Option<Gpu>,
    queue: Option<wgpu::Queue>,
    format: &'static str,
    /// The `gpu: true` canvases of the last render posted: their keys and ids.
    keys: Vec<(String, String)>,
    /// Their sizes, from the window's `canvas` messages, clamped.
    sizes: HashMap<String, (u32, u32)>,
    /// Saved to a snapshot: the greatest handle the program made, and the canvases errors stopped.
    saved: Option<(u64, SavedStops)>,
    /// The device's generation (`crate::gpu`).
    generation: u64,
}

impl RunGpu {
    /// A run's GPU on the provided device; placeholders without one.
    pub fn new(tell: impl Fn(serde_json::Value) + Send + Sync + 'static, notify: Arc<dyn Fn() + Send + Sync>) -> Arc<RunGpu> {
        let device = crate::gpu::device();
        let copies = device.as_ref().is_some_and(|(_, d)| d.copies);
        let run = Arc::new(RunGpu { canvases: Canvases::new(copies), inner: Mutex::new(Inner { gpu: None, queue: None, format: "rgba8unorm", keys: Vec::new(), sizes: HashMap::new(), saved: None, generation: device.as_ref().map_or(0, |(g, _)| *g) }), tell: Box::new(tell), notify });
        run.start(device.as_ref().map(|(_, d)| d));
        crate::gpu::register(Arc::downgrade(&run) as std::sync::Weak<dyn Lost>);
        crate::gpu::watch::add(Arc::downgrade(&run) as std::sync::Weak<dyn Watched>);
        run
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn start(&self, device: Option<&Device>) {
        let mut inner = self.lock();
        let gpu = device.and_then(|d| match Gpu::new(d.device.clone(), d.queue.clone(), d.format) {
            Ok(g) => Some(g),
            Err(e) => {
                eprintln!("widget gpu: {e}");
                None
            }
        });
        if let Some(g) = &gpu {
            inner.format = g.preferred_format();
        }
        inner.queue = gpu.as_ref().and(device).map(|d| d.queue.clone());
        self.canvases.start(gpu.as_ref().and(device).map(|d| &d.device));
        inner.gpu = gpu;
    }

    /// The run was saved to a snapshot (`widgets-gpu.md`, A run restored from a snapshot): its
    /// GPU goes, keeping the greatest handle its program made and the canvases errors stopped.
    pub fn save(&self) {
        let mut inner = self.lock();
        if let Some(gpu) = inner.gpu.take() {
            inner.saved = Some((gpu.last_handle(), gpu.saved_stops()));
            inner.queue = None;
        }
    }

    /// The run goes on from its snapshot, which is a device loss: a new GPU counts the handles
    /// made before as gone and keeps the stopped canvases stopped, and the program hears
    /// `{ type: "gpu", lost: "unknown" }` before it runs on.
    pub fn restore(&self) {
        let Some((last, stops)) = self.lock().saved.take() else { return };
        let device = crate::gpu::device();
        self.start(device.as_ref().map(|(_, d)| d));
        {
            let mut inner = self.lock();
            inner.generation = device.as_ref().map_or(inner.generation, |(g, _)| *g);
            let Inner { gpu, keys, sizes, .. } = &mut *inner;
            if let Some(gpu) = gpu {
                gpu.restored(last);
                gpu.restore_stops(stops);
                for (k, _) in keys.iter() {
                    let (w, h) = sizes.get(k).copied().unwrap_or((0, 0));
                    gpu.resize(k, w, h);
                }
            }
            self.canvases.begin();
            self.publish(&inner);
        }
        (self.tell)(json!({ "type": "gpu", "lost": "unknown" }));
    }

    /// The live input's `gpu` (`widgets-gpu.md`): whether batches replay, and the canvas format.
    pub fn input(&self) -> serde_json::Value {
        let inner = self.lock();
        json!({ "available": inner.gpu.is_some(), "format": inner.format })
    }

    /// A render was posted: its `gpu: true` canvases are the ones `context` ops name. Those that
    /// left are removed; those that came take the size the window last gave them (0 until it does).
    pub fn rendered(&self, keys: Vec<(String, String)>) {
        let mut inner = self.lock();
        let Inner { gpu, keys: old, sizes, .. } = &mut *inner;
        let has = |list: &[(String, String)], k: &str| list.iter().any(|(key, _)| key == k);
        if let Some(gpu) = gpu {
            for (k, _) in old.iter().filter(|(k, _)| !has(&keys, k)) {
                gpu.remove(k);
            }
            for (k, _) in keys.iter().filter(|(k, _)| !has(old, k)) {
                let (w, h) = sizes.get(k).copied().unwrap_or((0, 0));
                gpu.resize(k, w, h);
            }
        }
        self.canvases.set_keys(keys.iter().map(|(k, id)| (id.clone(), k.clone())).collect());
        *old = keys;
    }

    /// The window's `canvas` message reaches the program: canvas `key` is `width` × `height`
    /// physical pixels, clamped (`widget_gpu::clamp_size`).
    pub fn resize(&self, key: &str, width: u32, height: u32) {
        let mut inner = self.lock();
        inner.sizes.insert(key.to_owned(), (width, height));
        if inner.keys.iter().any(|(k, _)| k == key)
            && let Some(gpu) = &mut inner.gpu
        {
            gpu.resize(key, width, height);
        }
    }

    /// A frame request, after the batch it carries: the per-frame limits count again.
    pub fn end_frame(&self) {
        if let Some(gpu) = &mut self.lock().gpu {
            gpu.end_frame();
        }
    }

    /// Checks and replays a batch; the error to report, if any.
    pub fn apply(&self, ops: &[Arg], bytes: &[u8]) -> Option<GpuError> {
        let generation = {
            let inner = self.lock();
            inner.gpu.as_ref()?;
            inner.generation
        };
        // Its device was lost and the run is about to start over: dropped, as a batch from before
        // the program heard of the loss is.
        if crate::gpu::is_lost(generation) {
            return None;
        }
        // Its turn (which may wait for the window's copy, or the GPU) is taken before the GPU: the
        // hang watch can look at it meanwhile.
        self.canvases.begin();
        let mut inner = self.lock();
        let failed = inner.gpu.as_mut().and_then(|g| g.apply(ops, bytes).err());
        self.publish(&inner);
        crate::gpu::watch::poke();
        failed
    }

    /// A batch whose shape the host checks failed (`message`; `ops` when they were a list): it
    /// stops the canvases it renders into, or all of them, as a replayed batch's error does
    /// (`widgets-gpu.md`, Which canvases stop). The error to report, if it stopped any anew.
    pub fn refuse(&self, ops: Option<&[Arg]>, message: String) -> Option<GpuError> {
        self.lock().gpu.as_ref()?;
        self.canvases.begin();
        let mut inner = self.lock();
        let failed = inner.gpu.as_mut().and_then(|g| g.shape_error(ops, message).err());
        self.publish(&inner);
        failed
    }

    /// Publishes the canvases as they are for the window.
    fn publish(&self, inner: &Inner) {
        let mut shown = HashMap::new();
        if let Some(gpu) = &inner.gpu {
            for (k, id) in &inner.keys {
                if let Some(c) = gpu.canvas(k) {
                    shown.insert(id.clone(), Canvas { texture: c.texture.clone(), opaque: c.alpha == widget_gpu::AlphaMode::Opaque, version: c.version });
                }
            }
        }
        self.canvases.end(inner.queue.as_ref(), shown, Self::stopped(inner));
        (self.notify)();
    }

    /// The canvases an error stopped, by id, with why.
    fn stopped(inner: &Inner) -> HashMap<String, String> {
        let Some(gpu) = &inner.gpu else { return HashMap::new() };
        inner.keys.iter().filter_map(|(k, id)| Some((id.clone(), gpu.error(k)?.message.clone()))).collect()
    }

}

impl Watched for RunGpu {
    fn busy(&self) -> bool {
        self.canvases.busy()
    }

    /// A GPU hang (`widgets-gpu.md`, GPU hang) stops the canvases its batch draws into: a client
    /// error, logged and shown, not reported. A run busy with its GPU is looked at next time.
    fn look(&self) {
        self.canvases.poll();
        let mut inner = match self.inner.try_lock() {
            Ok(inner) => inner,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => return,
        };
        let Some(gpu) = inner.gpu.as_mut() else { return };
        if let Err(e) = gpu.poll() {
            eprintln!("widget gpu: {}", e.message);
            self.canvases.set_stopped(Self::stopped(&inner));
            (self.notify)();
        }
    }
}

impl Lost for RunGpu {
    /// The device was lost: the run starts over on `next` (handles gone, stopped canvases kept),
    /// and its program hears it.
    fn lost(&self, next: Option<&Device>, reason: &str, message: Option<&str>) {
        {
            let mut inner = self.lock();
            // Saved, it has no GPU: its restore is the loss it hears.
            if inner.saved.is_some() {
                return;
            }
            // Work running a quarter second at the loss hung (`widget_gpu::HANG_AT_LOSS`): judged
            // before the reset and the program hearing of it; its stop comes from the next poll.
            if let Some(gpu) = inner.gpu.as_mut() {
                gpu.lost();
            }
            let reset = match (inner.gpu.as_mut(), next) {
                (Some(gpu), Some(d)) => gpu.reset(d.device.clone(), d.queue.clone()).map_err(|e| eprintln!("widget gpu: {e}")).is_ok(),
                _ => false,
            };
            if !reset {
                inner.gpu = None;
            }
            inner.generation = crate::gpu::generation().unwrap_or(inner.generation);
            // Work the lost device hung on stays stopped (`Gpu::reset`): a client error, logged.
            if let Some(Err(e)) = inner.gpu.as_mut().map(Gpu::poll) {
                eprintln!("widget gpu: {}", e.message);
            }
            inner.queue = next.filter(|_| reset).map(|d| d.queue.clone());
            self.canvases.start(next.filter(|_| reset).map(|d| &d.device));
            self.canvases.begin();
            self.publish(&inner);
        }
        let mut lost = json!({ "type": "gpu", "lost": reason });
        if let Some(m) = message {
            lost["message"] = m.into();
        }
        (self.tell)(lost);
    }
}

/// A posted batch (`{ ops, bytes }`) read as `widget-gpu` takes it, with the host checks of its
/// shape (`widgets-gpu.md`, Host checks); its bytes borrowed in place for `f`. `Err`: the message,
/// with the ops when they were a list.
pub fn with_batch<R>(batch: &HostValue, f: impl FnOnce(&[Arg], &[u8]) -> R) -> Result<R, (Option<Vec<Arg>>, String)> {
    let field = |k: &str| <HostValue as crate::draw::Arg>::field(batch, k);
    let ops = match field("ops") {
        Some(HostValue::Array(items)) => items.iter().map(|i| arg(i.as_ref().unwrap_or(&HostValue::Null))).collect::<Vec<_>>(),
        _ => return Err((None, "a batch's ops must be a list".into())),
    };
    match field("bytes") {
        Some(HostValue::TypedArray(t)) if t.kind() == TypedKind::ArrayBuffer => Ok(t.with_bytes(|b| f(&ops, b))),
        _ => Err((Some(ops), "a batch's bytes must be an ArrayBuffer".into())),
    }
}

/// An op's value as JSON reads it (`widgets-gpu.md`, Transport): `undefined` and functions left
/// out of objects, `null` in lists. Numbers stay as they are: `NaN` and infinities fail the check
/// that reads them.
fn arg(v: &HostValue) -> Arg {
    match v {
        HostValue::Bool(b) => Arg::Bool(*b),
        HostValue::Number(n) => Arg::Num(*n),
        HostValue::String(s) => Arg::Str(s.to_string_lossy().into_owned()),
        HostValue::Array(items) => Arg::List(items.iter().map(|i| i.as_ref().filter(|i| !crate::host_value::skipped(i)).map_or(Arg::Null, arg)).collect()),
        HostValue::Object(fields) => Arg::Obj(fields.iter().filter(|(_, v)| !crate::host_value::skipped(v)).map(|(k, v)| (k.to_string_lossy().into_owned(), arg(v))).collect()),
        _ => match v.to_json() {
            Some(j) => Arg::from_json(&j),
            None => Arg::Null,
        },
    }
}

//! GPU canvases as a window shows them (`widgets-gpu.md`): a live run replays its batches on a GPU
//! device (`live::gpu`) and publishes each canvas's texture, or why it shows its placeholder, here;
//! the window's compositor shows them.
//!
//! Where the run's device is the window's, the window samples a canvas's texture as it is: one
//! queue orders the replay and the frame. Where it isn't (on Rune's Metal backend the run's device
//! is a stock wgpu one on the same `MTLDevice`, for wgpu's validation), the two queues don't order
//! each other: the host hands each frame over on the GPU ([`Handoff`]), so neither the run nor the
//! window waits for the other.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

mod shorthand;
pub(crate) mod watch;

pub use shorthand::{Ask, Shorthands};
pub use widget_gpu::{Arg, ErrorKind, Uniforms, clamp_size, device_limits, preferred_format};

/// `f`, which polls a device: `None` if it found the device lost. wgpu 30 panics in
/// `Device::poll` once a fence read fails ("Error in Device::poll: Parent device is lost"), which is
/// how a lost device shows; only that panic is caught (wgpu holds no lock when it panics there),
/// as Rune's `rune_gpu::health::poll` and `widget-gpu`'s own polls do. The device's loss callback
/// has run by then.
fn unless_lost<R>(f: impl FnOnce() -> R) -> Option<R> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => Some(r),
        Err(panic) => {
            let message = panic.downcast_ref::<String>().map(String::as_str).or_else(|| panic.downcast_ref::<&str>().copied()).unwrap_or("");
            if message.starts_with("Error in Device::poll") {
                return None;
            }
            std::panic::resume_unwind(panic)
        }
    }
}

/// Polls `device`, without waiting or with; nothing if it's lost (`unless_lost`).
pub(crate) fn poll(device: &wgpu::Device, kind: wgpu::PollType) {
    unless_lost(|| device.poll(kind).ok());
}

/// A device for GPU canvases, as the window provides it.
#[derive(Clone)]
pub struct Device {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// The canvases' format: `Bgra8Unorm` or `Rgba8Unorm` (`widget_gpu::preferred_format`).
    pub format: wgpu::TextureFormat,
    /// Not the window's own device: how its frames reach the window ([`Handoff`]).
    pub handoff: Option<Arc<dyn Handoff>>,
}

/// How a window on another device than the runs' takes their canvases' frames: on the GPU, so
/// neither waits for the other. Called on the thread that submitted them, right after.
pub trait Handoff: Send + Sync {
    /// `canvases`' run submitted these frames on `queue` (the canvases whose version moved).
    fn publish(&self, canvases: &Arc<Canvases>, queue: &wgpu::Queue, frames: &[(String, Canvas)]);
    /// `canvases`' run isn't replaying, as of the lock it holds: a frame `publish` couldn't hand
    /// over yet may be now.
    fn retry(&self, canvases: &Arc<Canvases>, queue: &wgpu::Queue);
    /// `canvases`' frames are no longer shown: a new device, or none.
    fn forget(&self, canvases: &Canvases);
}

type Provider = Arc<dyn Fn() -> Option<Device> + Send + Sync>;

struct Host {
    provider: Option<Provider>,
    /// The device made from the provider, once a run needed one; its generation.
    current: Option<(u64, Device)>,
    generation: u64,
    runs: Vec<Weak<dyn Lost>>,
}

static HOST: Mutex<Host> = Mutex::new(Host { provider: None, current: None, generation: 0, runs: Vec::new() });

/// The generation whose device wgpu reported lost, until its runs start over (`u64::MAX`: none).
static LOST: AtomicU64 = AtomicU64::new(u64::MAX);

/// Whether the device of `generation` was lost: work on it is dropped (it would fail as if the
/// program's), the runs about to start over on a new one.
#[cfg_attr(not(feature = "runner"), allow(dead_code))]
pub(crate) fn is_lost(generation: u64) -> bool {
    LOST.load(Ordering::Acquire) == generation || host().generation != generation
}

/// A user of the device (a live run, the `wgsl` canvases), which hears of its loss.
pub trait Lost: Send + Sync {
    /// The device was lost: start over on `next` (none: no GPU), and tell the program `reason`.
    fn lost(&self, next: Option<&Device>, reason: &str, message: Option<&str>);
}

/// Registers a live run, which a loss reaches.
#[cfg_attr(not(feature = "runner"), allow(dead_code))]
pub(crate) fn register(run: Weak<dyn Lost>) {
    host().runs.push(run);
}

/// Holds the hang watch (`true`), so nothing polls the runs' device, or lets it look again: for a
/// check that loses the device while GPU work isn't seen finished (the shared fixtures' `inFlight`
/// losses).
#[doc(hidden)]
pub fn hold_watch(held: bool) {
    watch::hold(held);
}

/// The device's generation now, for a check that loses it on purpose.
pub fn generation() -> Option<u64> {
    host().current.as_ref().map(|(g, _)| *g)
}

fn host() -> MutexGuard<'static, Host> {
    HOST.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How live runs get their GPU: `make` is called on a run's thread when one first needs it, and
/// again after a loss. Without a provider, GPU canvases show their placeholder.
pub fn provide(make: impl Fn() -> Option<Device> + Send + Sync + 'static) {
    host().provider = Some(Arc::new(make));
}

/// The device, made now if no run made it yet.
#[cfg_attr(not(feature = "runner"), allow(dead_code))]
pub(crate) fn device() -> Option<(u64, Device)> {
    let (provider, generation) = {
        let h = host();
        if let Some(c) = &h.current {
            return Some(c.clone());
        }
        (h.provider.clone()?, h.generation)
    };
    let d = provider()?;
    let mut h = host();
    if let Some(c) = &h.current {
        return Some(c.clone());
    }
    if h.generation != generation {
        return None;
    }
    let lost = generation;
    d.device.set_device_lost_callback(move |reason, message| {
        LOST.store(lost, Ordering::Release);
        let reason = match reason {
            wgpu::DeviceLostReason::Destroyed => "destroyed",
            _ => "unknown",
        };
        let message = (!message.is_empty()).then_some(message);
        // Off whatever thread noticed (the window's, polling): runs start over on a new device.
        if let Err(e) = std::thread::Builder::new().name("widget-gpu-lost".into()).spawn(move || lose(lost, reason, message)) {
            eprintln!("widget gpu: couldn't hear the device's loss: {e}");
        }
    });
    h.current = Some((generation, d.clone()));
    Some((generation, d))
}

/// The device of generation `generation` is lost: every run starts over on a new one, and its
/// program hears it.
pub fn lose(generation: u64, reason: &str, message: Option<String>) {
    let runs: Vec<Arc<dyn Lost>> = {
        let mut h = host();
        if h.generation != generation {
            return;
        }
        h.generation += 1;
        h.current = None;
        h.runs.retain(|r| r.strong_count() > 0);
        h.runs.iter().filter_map(Weak::upgrade).collect()
    };
    let next = device();
    for run in runs {
        run.lost(next.as_ref().map(|(_, d)| d), reason, message.as_deref());
    }
}

/// A canvas's texture, as the run last presented it.
#[derive(Clone, Debug)]
pub struct Canvas {
    pub texture: wgpu::Texture,
    /// `alphaMode: "opaque"`: its alpha is ignored. Else premultiplied.
    pub opaque: bool,
    /// Moves each time a submitted render pass wrote to it.
    pub version: u64,
}

/// What a GPU canvas shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Its texture, once a pass drew into it ([`Canvases::canvas`]).
    Drawn,
    /// Its placeholder: this client has no GPU for it, a client error (not reported).
    Unavailable,
    /// Its placeholder with the error that stopped it, for the rest of the run.
    Stopped(String),
}

/// A live run's GPU canvases, shared by the run (which publishes) and the window (which shows).
pub struct Canvases {
    state: Mutex<State>,
    /// Batches the run's GPU finished, of those submitted (`State::submitted`).
    finished: Arc<AtomicU64>,
    /// The run's device, which the hang watch polls (without waiting) to learn what finished.
    device: Mutex<Option<wgpu::Device>>,
    /// Wakes the window: a frame it shows changed, or reached it.
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// By canvas id (the node's, as the window has it).
#[derive(Default)]
struct State {
    /// The `gpu: true` canvases of the last render posted: their keys, by id.
    keys: HashMap<String, String>,
    shown: HashMap<String, Canvas>,
    stopped: HashMap<String, String>,
    available: bool,
    /// A batch replays: submits may be under way on the run's queue.
    replaying: bool,
    submitted: u64,
    /// The run's queue and how its frames reach the window, when that isn't the window's.
    handoff: Option<(wgpu::Queue, Arc<dyn Handoff>)>,
    /// Moves when a canvas's status does: the widget builds again.
    revision: u64,
    /// Moves when anything shown does (a version or a status).
    published: u64,
}

impl std::fmt::Debug for Canvases {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Canvases")
    }
}

impl Canvases {
    /// Canvases whose changes `wake` the window (none: it looks every frame).
    pub fn new(wake: Option<Arc<dyn Fn() + Send + Sync>>) -> Arc<Canvases> {
        Arc::new(Canvases { state: Mutex::default(), finished: Arc::default(), device: Mutex::default(), wake })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What canvas `key` shows.
    pub fn status(&self, key: &str) -> Status {
        let st = self.lock();
        match (st.stopped.get(key), st.available) {
            (Some(e), _) => Status::Stopped(e.clone()),
            (None, false) => Status::Unavailable,
            (None, true) => Status::Drawn,
        }
    }

    /// Moves when any canvas's status does.
    pub fn revision(&self) -> u64 {
        self.lock().revision
    }

    /// Moves when anything a canvas shows does: the window draws a frame for it.
    pub fn published(&self) -> u64 {
        self.lock().published
    }

    /// Canvas `key`'s key, which the program names it by (its `canvas` messages).
    pub fn key(&self, id: &str) -> Option<String> {
        self.lock().keys.get(id).cloned()
    }

    /// Whether the window takes the frames through a [`Handoff`] (the run's device isn't its).
    pub fn handed_off(&self) -> bool {
        self.lock().handoff.is_some()
    }

    #[cfg_attr(not(feature = "runner"), allow(dead_code))]
    pub(crate) fn set_keys(&self, keys: HashMap<String, String>) {
        self.lock().keys = keys;
    }

    /// Canvas `key`'s texture, once drawn: what a window on the run's own device samples.
    pub fn canvas(&self, key: &str) -> Option<Canvas> {
        self.lock().shown.get(key).cloned()
    }

    /// The run's device.
    pub fn device(&self) -> Option<wgpu::Device> {
        self.device.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Learns, without waiting, which batches the run's GPU finished.
    pub fn poll(&self) {
        let device = self.device.lock().unwrap_or_else(PoisonError::into_inner).clone();
        // A loss shows through the device's callback; nothing else to learn here.
        if let Some(d) = device {
            poll(&d, wgpu::PollType::Poll);
        }
    }

    /// Whether something wakes the window when a frame reaches it ([`Canvases::wake`]).
    pub fn wakes(&self) -> bool {
        self.wake.is_some()
    }

    /// Wakes the window: a frame it shows reached it.
    pub fn wake(&self) {
        if let Some(wake) = &self.wake {
            wake();
        }
    }

    /// Lets the [`Handoff`] hand over a frame it couldn't before, unless a batch replays (its
    /// submits mustn't interleave with the handoff's).
    pub fn retry(self: &Arc<Self>) {
        let st = self.lock();
        if st.replaying {
            return;
        }
        if let Some((queue, handoff)) = &st.handoff {
            handoff.retry(self, queue);
        }
    }

    /// Whether GPU work the run submitted may be unfinished (as of the last poll of its device).
    #[cfg_attr(not(feature = "runner"), allow(dead_code))]
    pub(crate) fn busy(&self) -> bool {
        self.finished.load(Ordering::Acquire) < self.lock().submitted
    }

    /// The canvases stopped now, outside a batch (a GPU hang the watch found).
    #[cfg_attr(not(feature = "runner"), allow(dead_code))]
    pub(crate) fn set_stopped(&self, stopped: HashMap<String, String>) {
        let mut st = self.lock();
        if st.stopped != stopped {
            st.stopped = stopped;
            st.revision += 1;
            st.published += 1;
        }
    }

    /// The run's GPU, or none (`available` false: placeholders).
    #[cfg_attr(not(feature = "runner"), allow(dead_code))]
    pub(crate) fn start(&self, device: Option<&Device>) {
        *self.device.lock().unwrap_or_else(PoisonError::into_inner) = device.map(|d| d.device.clone());
        let mut st = self.lock();
        if let Some((_, handoff)) = st.handoff.take() {
            handoff.forget(self);
        }
        st.handoff = device.and_then(|d| Some((d.queue.clone(), d.handoff.clone()?)));
        st.available = device.is_some();
        st.shown.clear();
        st.revision += 1;
    }

    /// A batch is about to replay: until [`Canvases::end`], its submits may be under way.
    #[cfg_attr(not(feature = "runner"), allow(dead_code))]
    pub(crate) fn begin(&self) {
        self.lock().replaying = true;
    }

    /// The batch replayed on `queue`: the canvases as they are now, and those stopped. Frames
    /// that moved go to the [`Handoff`], if any.
    #[cfg_attr(not(feature = "runner"), allow(dead_code))]
    pub(crate) fn end(self: &Arc<Self>, queue: Option<&wgpu::Queue>, shown: HashMap<String, Canvas>, stopped: HashMap<String, String>) {
        let mut st = self.lock();
        st.submitted += 1;
        if let Some(queue) = queue {
            let (finished, n) = (self.finished.clone(), st.submitted);
            queue.on_submitted_work_done(move || {
                finished.fetch_max(n, Ordering::AcqRel);
            });
        } else {
            self.finished.fetch_max(st.submitted, Ordering::AcqRel);
        }
        if st.stopped != stopped {
            st.revision += 1;
            st.published += 1;
        }
        let moved: Vec<(String, Canvas)> = shown.iter().filter(|(k, c)| st.shown.get(*k).is_none_or(|o| o.version != c.version || o.texture != c.texture)).map(|(k, c)| (k.clone(), c.clone())).collect();
        if !moved.is_empty() {
            st.published += 1;
        }
        st.shown = shown;
        st.stopped = stopped;
        // Still replaying until handed over, so a retry can't come in between.
        if let Some((queue, handoff)) = st.handoff.clone()
            && !moved.is_empty()
        {
            drop(st);
            handoff.publish(self, &queue, &moved);
            st = self.lock();
        }
        st.replaying = false;
    }
}

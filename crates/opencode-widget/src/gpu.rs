//! GPU canvases as a window shows them (`widgets-gpu.md`): a live run replays its batches on a GPU
//! device (`live::gpu`) and publishes each canvas's texture, or why it shows its placeholder, here;
//! the window's compositor shows them.
//!
//! Where the run's device is the window's, the window samples a canvas's texture as it is: one
//! queue orders the replay and the frame. Where it isn't (on Rune's Metal backend the run's device
//! is a stock wgpu one on the same `MTLDevice`, for wgpu's validation), the two queues don't order
//! each other, so the window shows a copy, handed over so the window never waits:
//! - it copies a canvas only once the run's GPU finished every batch submitted before, and never
//!   while one replays ([`Canvases::due`]);
//! - a batch waits to replay while a copy is in flight ([`Canvases::begin`]): until the window's
//!   queue finished it, normally done already;
//! - a run whose GPU never catches up (it submits faster than it finishes) would leave no moment
//!   to copy in: when the window found its GPU busy, the next batch first lets it finish and gives
//!   the window a moment (20 ms at most) to take its copy.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};

pub use widget_gpu::{Arg, ErrorKind, clamp_size, device_limits, preferred_format};

/// A device for GPU canvases, as the window provides it.
#[derive(Clone)]
pub struct Device {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// The canvases' format: `Bgra8Unorm` or `Rgba8Unorm` (`widget_gpu::preferred_format`).
    pub format: wgpu::TextureFormat,
    /// Not the window's own device: the window shows copies ([`Canvases::copies`]).
    pub copies: bool,
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

/// A live run on the device, which hears of its loss.
pub trait Lost: Send + Sync {
    /// The device was lost: start over on `next` (none: no GPU), and tell the program `reason`.
    fn lost(&self, next: Option<&Device>, reason: &str, message: Option<&str>);
}

/// Registers a live run, which a loss reaches.
#[cfg_attr(not(feature = "runner"), allow(dead_code))]
pub(crate) fn register(run: Weak<dyn Lost>) {
    host().runs.push(run);
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
    turn: Condvar,
    /// Batches the run's GPU finished, of those submitted (`State::submitted`).
    finished: Arc<AtomicU64>,
    /// The window shows copies: the run's device isn't the window's.
    pub copies: bool,
    /// The run's device, which the window polls (without waiting) to learn what finished.
    device: Mutex<Option<wgpu::Device>>,
}

/// By canvas id (the node's, as the window has it).
#[derive(Default)]
struct State {
    /// The `gpu: true` canvases of the last render posted: their keys, by id.
    keys: HashMap<String, String>,
    shown: HashMap<String, Canvas>,
    stopped: HashMap<String, String>,
    available: bool,
    replaying: bool,
    copying: bool,
    /// The window found the run's GPU busy when it wanted a copy.
    wanted: bool,
    submitted: u64,
    /// The versions last copied, by key.
    copied: HashMap<String, u64>,
    /// Moves when a canvas's status does: the widget builds again.
    revision: u64,
    /// Moves when anything shown does (a version or a status).
    published: u64,
    /// Batches that waited for a copy: how many, how long in all, the longest.
    waits: (u64, std::time::Duration, std::time::Duration),
}

impl std::fmt::Debug for Canvases {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Canvases")
    }
}

impl Canvases {
    pub fn new(copies: bool) -> Arc<Canvases> {
        Arc::new(Canvases { state: Mutex::default(), turn: Condvar::new(), finished: Arc::default(), copies, device: Mutex::default() })
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

    /// Batches that waited for a copy to finish before replaying: how many, how long in all, and
    /// the longest wait.
    pub fn waits(&self) -> (u64, std::time::Duration, std::time::Duration) {
        self.lock().waits
    }

    /// Moves when anything a canvas shows does: the window draws a frame for it.
    pub fn published(&self) -> u64 {
        self.lock().published
    }

    /// Canvas `id`'s key, which the program names it by (its `canvas` messages).
    pub fn key(&self, id: &str) -> Option<String> {
        self.lock().keys.get(id).cloned()
    }

    /// Whether a canvas has a version the window hasn't copied yet.
    pub fn behind(&self) -> bool {
        let st = self.lock();
        st.shown.iter().any(|(k, c)| st.copied.get(k) != Some(&c.version))
    }

    #[cfg_attr(not(feature = "runner"), allow(dead_code))]
    pub(crate) fn set_keys(&self, keys: HashMap<String, String>) {
        self.lock().keys = keys;
    }

    /// Canvas `key`'s texture, once drawn: what a window on the run's own device samples.
    pub fn canvas(&self, key: &str) -> Option<Canvas> {
        self.lock().shown.get(key).cloned()
    }

    /// Learns, without waiting, which batches the run's GPU finished.
    pub fn poll(&self) {
        let device = self.device.lock().unwrap_or_else(PoisonError::into_inner).clone();
        // A loss shows through the device's callback; nothing else to learn here.
        if let Some(d) = device {
            d.poll(wgpu::PollType::Poll).ok();
        }
    }

    /// The canvases to copy now, newer than their last copy; then a copy is in flight until
    /// [`Canvases::copied`]. None while a batch replays, its work isn't finished, or a copy is in
    /// flight: the window shows what it copied last.
    pub fn due(&self) -> Option<Vec<(String, Canvas)>> {
        let mut st = self.lock();
        if st.replaying || st.copying {
            return None;
        }
        if self.finished.load(Ordering::Acquire) < st.submitted {
            st.wanted = st.shown.iter().any(|(k, c)| st.copied.get(k) != Some(&c.version));
            return None;
        }
        let due: Vec<(String, Canvas)> = st.shown.iter().filter(|(k, c)| st.copied.get(*k) != Some(&c.version)).map(|(k, c)| (k.clone(), c.clone())).collect();
        if due.is_empty() {
            return None;
        }
        for (k, c) in &due {
            st.copied.insert(k.clone(), c.version);
        }
        st.copying = true;
        st.wanted = false;
        self.turn.notify_all();
        Some(due)
    }

    /// The window's queue finished the copy [`Canvases::due`] gave: batches may replay.
    pub fn copied(&self) {
        self.lock().copying = false;
        self.turn.notify_all();
    }

    /// The run's GPU, or none (`available` false: placeholders).
    #[cfg_attr(not(feature = "runner"), allow(dead_code))]
    pub(crate) fn start(&self, device: Option<&wgpu::Device>) {
        *self.device.lock().unwrap_or_else(PoisonError::into_inner) = device.cloned();
        let mut st = self.lock();
        st.available = device.is_some();
        st.shown.clear();
        st.copied.clear();
        st.revision += 1;
    }

    /// A batch is about to replay: once no copy is in flight. Returns how long it waited.
    #[cfg_attr(not(feature = "runner"), allow(dead_code))]
    pub(crate) fn begin(&self) -> std::time::Duration {
        let since = std::time::Instant::now();
        let mut st = self.lock();
        let mut waited = false;
        loop {
            while st.copying {
                waited = true;
                st = self.turn.wait(st).unwrap_or_else(PoisonError::into_inner);
            }
            if !st.wanted {
                break;
            }
            // The window wants a copy but found the GPU busy: let it finish, and the window take
            // its copy (or, if it doesn't come, go on).
            waited = true;
            drop(st);
            let device = self.device.lock().unwrap_or_else(PoisonError::into_inner).clone();
            if let Some(d) = device {
                d.poll(wgpu::PollType::wait_indefinitely()).ok();
            }
            st = self.lock();
            let (guard, timeout) = self.turn.wait_timeout_while(st, std::time::Duration::from_millis(20), |s| s.wanted && !s.copying).unwrap_or_else(PoisonError::into_inner);
            st = guard;
            if timeout.timed_out() {
                st.wanted = false;
            }
        }
        st.replaying = true;
        let d = since.elapsed();
        if waited {
            st.waits = (st.waits.0 + 1, st.waits.1 + d, st.waits.2.max(d));
        }
        d
    }

    /// The batch replayed on `queue`: the canvases as they are now, and those stopped. The
    /// window may copy them once the GPU finished it.
    #[cfg_attr(not(feature = "runner"), allow(dead_code))]
    pub(crate) fn end(&self, queue: Option<&wgpu::Queue>, shown: HashMap<String, Canvas>, stopped: HashMap<String, String>) {
        let mut st = self.lock();
        st.replaying = false;
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
        if shown.iter().any(|(k, c)| st.shown.get(k).is_none_or(|o| o.version != c.version || o.texture != c.texture)) {
            st.published += 1;
        }
        st.shown = shown;
        st.stopped = stopped;
    }
}

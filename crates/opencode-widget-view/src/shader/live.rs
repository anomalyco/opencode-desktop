//! GPU canvases in the window's frame (`widgets-gpu.md`), a live program's or `wgsl`: the device
//! they render on ([`provide`]), and each canvas's texture drawn at its slot.
//!
//! Where the window draws with stock wgpu, runs replay on its own device and the frame samples a
//! canvas's texture as it is (one queue orders both). On Rune's Metal backend, which validates
//! nothing (a program error must never be undefined behaviour), runs replay on a stock wgpu Metal
//! device made on Rune's own `MTLDevice`, and their frames reach the window on the GPU, through a
//! ring of slots per canvas (`handoff`): neither waits for the other.

use std::collections::HashMap;
use std::sync::Arc;

use opencode_widget::gpu::Canvases;
use opencode_widget::gpu::{self as run, Device};
use rune::compose::{DeviceKey, Frame};

/// One live canvas on the window's device: the texture it samples (the run's, or a slot of its
/// ring on Rune's Metal backend), with the blit's uniform and bind group.
struct Target {
    /// What it samples: the run's texture, or which ring generation and slot.
    of: Of,
    blit_uniform: wgpu::Buffer,
    blit_bind: wgpu::BindGroup,
    opaque: bool,
    /// The version it shows (slots: anything but 0 once one shows).
    version: u64,
}

#[derive(PartialEq)]
enum Of {
    Run(wgpu::Texture),
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Slot(u64, usize),
}

/// The GPU canvases' side of the window's GPU, by run and canvas id.
#[derive(Default)]
pub(super) struct Live {
    targets: HashMap<(usize, String), Target>,
    /// A slot's texture wrapped for the window's device, by run, canvas id, ring generation and slot.
    #[cfg(target_os = "macos")]
    wrapped: HashMap<(usize, String, u64, usize), wgpu::Texture>,
    /// The canvases on screen: their run and id, by mark.
    marks: HashMap<u32, (usize, String)>,
    /// A canvas on screen shows an older version than its run published, and nothing will wake
    /// the window when it can show it: frames keep coming.
    pub animating: bool,
}

/// Makes `f`'s device the one live runs replay on, again when it changes (its runs start over
/// on the new one, hearing a loss).
pub(super) fn provide<S>(provided: &mut Option<DeviceKey>, f: &Frame<'_, S>) {
    let key = f.device_key();
    if *provided == Some(key) {
        return;
    }
    let had = provided.is_some();
    *provided = Some(key);
    let (device, queue) = (f.device.clone(), f.queue.clone());
    #[cfg(target_os = "macos")]
    if let Some(raw) = rune_gpu::metal::raw_device(f.device) {
        let rings = rings(raw);
        let raw = raw as *const objc2::runtime::ProtocolObject<dyn objc2_metal::MTLDevice> as usize;
        let stock = SHARE.get().map(|s| s.stock);
        run::provide(move || {
            let mut d = stock.and_then(|f| f(raw))?;
            d.handoff = Some(rings.clone()? as Arc<dyn run::Handoff>);
            Some(d)
        });
        if had && let Some(g) = run::generation() {
            run::lose(g, "unknown", None);
        }
        return;
    }
    let format = widget_gpu_format(device.adapter_info().backend);
    run::provide(move || Some(Device { device: device.clone(), queue: queue.clone(), format, handoff: None }));
    if had && let Some(g) = run::generation() {
        run::lose(g, "unknown", None);
    }
}

/// The canvas format a backend's surfaces prefer (`widget_gpu::preferred_format`).
fn widget_gpu_format(backend: wgpu::Backend) -> wgpu::TextureFormat {
    match backend {
        wgpu::Backend::Metal | wgpu::Backend::Dx12 => wgpu::TextureFormat::Bgra8Unorm,
        _ => wgpu::TextureFormat::Rgba8Unorm,
    }
}

/// What sharing a GPU with Rune's Metal backend takes that only the app may do (`unsafe`, in
/// `apps/desktop`).
#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
pub struct MetalShare {
    /// A stock wgpu Metal device made on Rune's `MTLDevice` (at that address, with the spec's
    /// limits).
    pub stock: fn(usize) -> Option<Device>,
    /// A texture of it as its `MTLTexture`.
    pub raw_texture: fn(&wgpu::Texture) -> Option<RawTexture>,
    /// Commits on `queue` (a stock wgpu Metal one) a command buffer, with retained references,
    /// that waits for `wait`, copies `from` into `to` (textures of that queue's device, of one
    /// size and format) and signals `signal`. False if it couldn't.
    pub copy: CopyFn,
    /// Calls `then`, on a Metal thread, once `event` reaches `value`.
    pub notify: fn(event: &RawEvent, value: u64, then: Box<dyn FnOnce() + Send>),
}

/// [`MetalShare::copy`]: `(queue, from, to, wait, signal)`.
#[cfg(target_os = "macos")]
pub type CopyFn = fn(&wgpu::Queue, &wgpu::Texture, &wgpu::Texture, Option<(&RawEvent, u64)>, (&RawEvent, u64)) -> bool;

/// A texture's `MTLTexture`.
#[cfg(target_os = "macos")]
pub type RawTexture = objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_metal::MTLTexture>>;

/// A shared event, as the hand-off's (`handoff`).
#[cfg(target_os = "macos")]
pub type RawEvent = objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_metal::MTLSharedEvent>>;

#[cfg(target_os = "macos")]
static SHARE: std::sync::OnceLock<MetalShare> = std::sync::OnceLock::new();

/// The canvases' rings, once made on Rune's `MTLDevice`.
#[cfg(target_os = "macos")]
static RINGS: std::sync::Mutex<Option<Arc<super::handoff::Rings>>> = std::sync::Mutex::new(None);

/// How live GPU canvases share Rune's Metal device: without it, on Rune's Metal backend they show
/// their placeholder.
#[cfg(target_os = "macos")]
pub fn share_metal(share: MetalShare) {
    // Set once, at startup.
    SHARE.get_or_init(|| share);
}

/// The rings on `device` (Rune's `MTLDevice`), made again for a new one.
#[cfg(target_os = "macos")]
fn rings(device: &objc2::runtime::ProtocolObject<dyn objc2_metal::MTLDevice>) -> Option<Arc<super::handoff::Rings>> {
    let share = *SHARE.get()?;
    let mut r = RINGS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let rings = Arc::new(super::handoff::Rings::new(share, device, super::handoff::SLOTS));
    *r = Some(rings.clone());
    Some(rings)
}

/// The rings live canvases are handed over through, if any.
#[cfg(target_os = "macos")]
fn current_rings() -> Option<Arc<super::handoff::Rings>> {
    RINGS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

impl Live {
    /// Before the frame records: each GPU canvas on screen (its mark, run's canvases and id there)
    /// gets the newest frame the window can show: through its ring (Rune's Metal backend), or the
    /// run's texture as it is.
    pub(super) fn prepare<S>(&mut self, f: &mut Frame<'_, S>, blit: &super::gpu::Blit, shown: &[(u32, Arc<Canvases>, String)]) {
        self.animating = false;
        let mut on_screen = Vec::new();
        self.marks.clear();
        for (m, canvases, id) in shown {
            let key = (Arc::as_ptr(canvases) as usize, id.clone());
            self.marks.insert(*m, key.clone());
            on_screen.push(key);
        }
        self.targets.retain(|k, _| on_screen.contains(k));
        #[cfg(target_os = "macos")]
        self.wrapped.retain(|(r, id, ..), _| on_screen.iter().any(|(or, oid)| or == r && oid == id));
        for (_, canvases, id) in shown {
            let run = Arc::as_ptr(canvases) as usize;
            if canvases.handed_off() {
                #[cfg(target_os = "macos")]
                self.slot(f, blit, canvases, id);
                continue;
            }
            let Some(c) = canvases.canvas(id) else { continue };
            if let Some(t) = self.target(f.device, blit, run, id, Of::Run(c.texture.clone()), &c.texture) {
                t.opaque = c.opaque;
                t.version = c.version;
            }
        }
    }

    /// Canvas `id` of `canvases` shows the newest slot of its ring the window can.
    #[cfg(target_os = "macos")]
    fn slot<S>(&mut self, f: &mut Frame<'_, S>, blit: &super::gpu::Blit, canvases: &Arc<Canvases>, id: &str) {
        let Some(rings) = current_rings() else { return };
        // A frame that found no free slot may now.
        canvases.retry();
        let run = Arc::as_ptr(canvases) as usize;
        if let Some(shown) = rings.show(canvases, id, f.encoder) {
            let key = (run, id.to_owned(), shown.generation, shown.slot);
            if !self.wrapped.contains_key(&key)
                && let Some(w) = wrap(f.device, &shown.texture)
            {
                self.wrapped.retain(|(r, i, g, _), _| !(*r == run && i == id && *g != shown.generation));
                self.wrapped.insert(key.clone(), w);
            }
            if let Some(w) = self.wrapped.get(&key).cloned()
                && let Some(t) = self.target(f.device, blit, run, id, Of::Slot(shown.generation, shown.slot), &w)
            {
                t.opaque = canvases.canvas(id).is_some_and(|c| c.opaque);
                t.version = 1;
            }
        }
        // A frame on its way: the window draws until it shows, as it does a run's own texture.
        if rings.pending(canvases, id) {
            self.animating = true;
        }
    }

    /// Canvas `id`'s target sampling `texture` (`of`), made again when that changed.
    fn target(&mut self, device: &wgpu::Device, blit: &super::gpu::Blit, run: usize, id: &str, of: Of, texture: &wgpu::Texture) -> Option<&mut Target> {
        let key = (run, id.to_owned());
        if self.targets.get(&key).is_none_or(|t| t.of != of) {
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let (blit_uniform, blit_bind) = blit.bind(device, &view);
            self.targets.insert(key.clone(), Target { of, blit_uniform, blit_bind, opaque: false, version: 0 });
        }
        self.targets.get_mut(&key)
    }

    /// Slot `mark`'s canvas, to draw: its blit uniform, bind group and whether it's opaque.
    pub(super) fn drawn(&self, mark: u32) -> Option<(&wgpu::Buffer, &wgpu::BindGroup, bool)> {
        let t = self.targets.get(self.marks.get(&mark)?)?;
        (t.version > 0).then_some((&t.blit_uniform, &t.blit_bind, t.opaque))
    }
}

/// Slot texture `t` (on the run's device) as a texture of the window's device: the same
/// `MTLTexture`, kept alive by `t`.
#[cfg(target_os = "macos")]
fn wrap(device: &wgpu::Device, t: &wgpu::Texture) -> Option<wgpu::Texture> {
    rune_gpu::metal::raw_device(device)?;
    let raw = (SHARE.get()?.raw_texture)(t)?;
    let desc = wgpu::TextureDescriptor { label: Some("widget gpu canvas slot"), size: t.size(), mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: t.format(), usage: t.usage(), view_formats: &[] };
    Some(rune_gpu::metal::texture_from_raw(raw, &desc, Arc::new(t.clone())))
}

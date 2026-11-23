//! GPU canvases in the window's frame (`widgets-gpu.md`), a live program's or `wgsl`: the device
//! they render on ([`provide`]), and each canvas's texture drawn at its slot.
//!
//! Where the window draws with stock wgpu, runs replay on its own device and the frame samples a
//! canvas's texture as it is (one queue orders both). On Rune's Metal backend, which validates
//! nothing (a program error must never be undefined behaviour), runs replay on a stock wgpu Metal
//! device made on Rune's own `MTLDevice`, and the window shows a copy of each canvas, made on its
//! own queue once the run's GPU finished it and handed back when the copy finished
//! ([`opencode_widget::gpu::Canvases`]): the window never waits on the GPU.

use std::collections::HashMap;
use std::sync::Arc;

use opencode_widget::gpu::{Canvas, Canvases};
use opencode_widget::gpu::{self as run, Device};
use rune::compose::{DeviceKey, Frame};

/// One live canvas's texture on the window's device: a copy (Rune's Metal backend) or the run's
/// own texture, with the blit's uniform and bind group.
struct Target {
    /// The texture drawn: the copy, or the run's.
    texture: wgpu::Texture,
    /// The run's texture wrapped for the window's device, to copy from (copies only).
    source: Option<(wgpu::Texture, wgpu::Texture)>,
    blit_uniform: wgpu::Buffer,
    blit_bind: wgpu::BindGroup,
    opaque: bool,
    /// The version it shows.
    version: u64,
}

/// The GPU canvases' side of the window's GPU, by run and canvas id.
#[derive(Default)]
pub(super) struct Live {
    targets: HashMap<(usize, String), Target>,
    /// The canvases on screen: their run and id, by mark.
    marks: HashMap<u32, (usize, String)>,
    /// A canvas on screen shows an older version than its run published: frames keep coming.
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
        let raw = raw as *const objc2::runtime::ProtocolObject<dyn objc2_metal::MTLDevice> as usize;
        let stock = SHARE.get().map(|s| s.stock);
        run::provide(move || stock.and_then(|f| f(raw)));
        if had && let Some(g) = run::generation() {
            run::lose(g, "unknown", None);
        }
        return;
    }
    let format = widget_gpu_format(device.adapter_info().backend);
    run::provide(move || Some(Device { device: device.clone(), queue: queue.clone(), format, copies: false }));
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
/// `apps/desktop`): a stock wgpu Metal device made on Rune's `MTLDevice` (at that address, with
/// the spec's limits), and a texture of it as its `MTLTexture`.
#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
pub struct MetalShare {
    pub stock: fn(usize) -> Option<Device>,
    pub raw_texture: fn(&wgpu::Texture) -> Option<RawTexture>,
}

/// A texture's `MTLTexture`.
#[cfg(target_os = "macos")]
pub type RawTexture = objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_metal::MTLTexture>>;

#[cfg(target_os = "macos")]
static SHARE: std::sync::OnceLock<MetalShare> = std::sync::OnceLock::new();

/// How live GPU canvases share Rune's Metal device: without it, on Rune's Metal backend they show
/// their placeholder.
#[cfg(target_os = "macos")]
pub fn share_metal(share: MetalShare) {
    // Set once, at startup.
    SHARE.get_or_init(|| share);
}

impl Live {
    /// Before the frame records: each GPU canvas on screen (its mark, run's canvases and id there)
    /// gets its latest finished version, copied on the window's queue (copies) or bound as it is.
    pub(super) fn prepare<S>(&mut self, f: &mut Frame<'_, S>, blit: &super::gpu::Blit, shown: &[(u32, Arc<Canvases>, String)]) {
        self.animating = false;
        let mut runs: Vec<Arc<Canvases>> = Vec::new();
        let mut on_screen = Vec::new();
        self.marks.clear();
        for (m, canvases, id) in shown {
            if !runs.iter().any(|r| Arc::ptr_eq(r, canvases)) {
                runs.push(canvases.clone());
            }
            let key = (Arc::as_ptr(canvases) as usize, id.clone());
            self.marks.insert(*m, key.clone());
            on_screen.push(key);
        }
        self.targets.retain(|k, _| on_screen.contains(k));
        let mut copies: Option<wgpu::CommandEncoder> = None;
        let mut copied: Vec<Arc<Canvases>> = Vec::new();
        for canvases in runs {
            let run = Arc::as_ptr(&canvases) as usize;
            if canvases.copies {
                canvases.poll();
                if let Some(due) = canvases.due() {
                    let enc = copies.get_or_insert_with(|| f.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("widget gpu copies") }));
                    for (id, c) in due {
                        if let Some(t) = self.target(f.device, blit, run, &id, &c, true) {
                            if let Some((_, from)) = &t.source {
                                enc.copy_texture_to_texture(from.as_image_copy(), t.texture.as_image_copy(), c.texture.size());
                            }
                            t.version = c.version;
                        }
                    }
                    copied.push(canvases.clone());
                }
            }
            for (_, id) in on_screen.iter().filter(|(r, _)| *r == run) {
                let Some(c) = canvases.canvas(id) else { continue };
                if !canvases.copies
                    && let Some(t) = self.target(f.device, blit, run, id, &c, false)
                {
                    t.version = c.version;
                }
                if self.targets.get(&(run, id.clone())).is_none_or(|t| t.version != c.version) {
                    self.animating = true;
                }
            }
        }
        if let Some(enc) = copies {
            f.queue.submit([enc.finish()]);
            for c in copied {
                f.queue.on_submitted_work_done(move || c.copied());
            }
        }
    }

    /// Canvas `id`'s target for `c`, made again when its texture changed.
    fn target(&mut self, device: &wgpu::Device, blit: &super::gpu::Blit, run: usize, id: &str, c: &Canvas, copy: bool) -> Option<&mut Target> {
        let key = (run, id.to_owned());
        let fits = self.targets.get(&key).is_some_and(|t| match &t.source {
            Some((of, _)) => *of == c.texture,
            None => t.texture == c.texture,
        });
        if !fits {
            let (texture, source) = if copy {
                let source = wrap(device, &c.texture)?;
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("widget gpu canvas"),
                    size: c.texture.size(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: c.texture.format(),
                    usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                (texture, Some((c.texture.clone(), source)))
            } else {
                (c.texture.clone(), None)
            };
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let (blit_uniform, blit_bind) = blit.bind(device, &view);
            self.targets.insert(key.clone(), Target { texture, source, blit_uniform, blit_bind, opaque: c.opaque, version: 0 });
        }
        let t = self.targets.get_mut(&key)?;
        t.opaque = c.opaque;
        Some(t)
    }

    /// Slot `mark`'s canvas, to draw: its blit uniform, bind group and whether it's opaque.
    pub(super) fn drawn(&self, mark: u32) -> Option<(&wgpu::Buffer, &wgpu::BindGroup, bool)> {
        let t = self.targets.get(self.marks.get(&mark)?)?;
        (t.version > 0).then_some((&t.blit_uniform, &t.blit_bind, t.opaque))
    }
}

/// The run's texture `t` (on the stock device) as a texture of the window's device: the same
/// `MTLTexture`, kept alive by `t`.
#[cfg(target_os = "macos")]
fn wrap(device: &wgpu::Device, t: &wgpu::Texture) -> Option<wgpu::Texture> {
    rune_gpu::metal::raw_device(device)?;
    let raw = (SHARE.get()?.raw_texture)(t)?;
    let desc = wgpu::TextureDescriptor { label: Some("widget gpu canvas (run)"), size: t.size(), mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: t.format(), usage: t.usage(), view_formats: &[] };
    Some(rune_gpu::metal::texture_from_raw(raw, &desc, Arc::new(t.clone())))
}

#[cfg(not(target_os = "macos"))]
fn wrap(_device: &wgpu::Device, _t: &wgpu::Texture) -> Option<wgpu::Texture> {
    None
}

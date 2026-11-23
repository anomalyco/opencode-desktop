//! GPU canvases on the window's GPU, for the host's compositor. Before the frame records, each
//! `wgsl` canvas on screen that is due asks for a frame at the size it shows at
//! ([`opencode_widget::gpu::Shorthands`]: `widget_gpu`'s shorthand, rendered off this thread on
//! the device live runs share); each canvas, `wgsl` or a live run's, gets its latest finished
//! frame (`live`), which is drawn at the slot's place in Rune's pass, clipped as Rune clips. A
//! `wgsl` canvas is due again at its `fps`; with `fps: 0` only when its `values` change or it's
//! resized: a still one renders and copies nothing more. One whose frames take the GPU over
//! 50 ms three in a row stops on its last frame (the `Shorthand` tells), shown frozen.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use opencode_widget::gpu::{Ask, Canvases, Shorthands, Uniforms};
use rune::compose::{DeviceKey, Frame};
use rune::render::{EffectSite, Instance, KIND_EFFECT};

use super::{Status, is_mark, set_status, with_slot};

thread_local! {
    static LIVE_ANIMATING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the window keeps drawing frames for a live GPU canvas showing an older version than
/// its run published: then a new version needs no other frame.
pub fn live_animating() -> bool {
    LIVE_ANIMATING.get()
}

#[derive(Default)]
pub struct ShaderGpu {
    gpu: Option<Gpu>,
    /// The window device live runs replay on was provided for (`live::provide`).
    provided: Option<DeviceKey>,
    /// The shader marks on screen this frame (kept to reuse its allocation).
    on_screen: Vec<(u32, [f32; 4], f32)>,
    animating: bool,
}

struct Gpu {
    key: DeviceKey,
    blit: Blit,
    /// The `wgsl` canvases asked for a frame, by mark.
    slots: HashMap<u32, SlotGpu>,
    shorthands: Shorthands,
    /// The `wgsl` canvases on screen the worker last heard of.
    kept: HashSet<String>,
    /// What each canvas on screen shows: its run's canvases (the shorthands', for `wgsl`), and
    /// its id there.
    shown: Vec<(u32, Arc<Canvases>, String)>,
    /// The canvases' textures on this window's device.
    live: super::live::Live,
}

/// One `wgsl` canvas's drawing as an explicit machine, and the size it last asked for.
struct SlotGpu {
    size: [u32; 2],
    phase: Phase,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Nothing rendered yet.
    Fresh,
    /// Renders every `1 / fps` (or, at `fps: 0`, when `values` change); `start` is when it
    /// first drew (`widget.time` counts from it).
    Running { start: f64, last: f64, values: [f32; 16] },
    /// Too slow: stopped on its last frame.
    Frozen,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct BlitUniform {
    rect: [f32; 4],
    clip: [f32; 4],
    frame: [f32; 4],
    fade: [u32; 4],
    flags: [f32; 4],
}

impl BlitUniform {
    fn bytes(&self) -> [u8; 80] {
        let mut out = [0u8; 80];
        let words = self.rect.iter().map(|f| f.to_bits()).chain(self.clip.iter().map(|f| f.to_bits())).chain(self.frame.iter().map(|f| f.to_bits())).chain(self.fade).chain(self.flags.iter().map(|f| f.to_bits()));
        for (i, w) in words.enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        out
    }
}

pub(super) struct Blit {
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    shader: wgpu::ShaderModule,
    sampler: wgpu::Sampler,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

/// An effect slot's tag, from the two 16-bit halves its record keeps it in (`El::effect`).
fn mark(i: &Instance) -> u32 {
    (i.border[0] as u32 & 0xFFFF) | (i.border[1] as u32 & 0xFFFF) << 16
}

impl Gpu {
    fn new(device: &wgpu::Device, key: DeviceKey) -> Gpu {
        Gpu { key, blit: Blit::new(device), slots: HashMap::new(), shorthands: Shorthands::new(), kept: HashSet::new(), shown: Vec::new(), live: super::live::Live::default() }
    }
}

impl ShaderGpu {
    /// Whether a shader slot was on screen last frame.
    pub fn draws_own(&self) -> bool {
        self.gpu.as_ref().is_some_and(|g| !g.slots.is_empty())
    }

    /// True while a shader on screen animates: the window keeps drawing frames.
    pub fn animating(&self) -> bool {
        self.animating
    }

    /// Asks the `wgsl` canvases on screen that are due for a frame, and gets every canvas on
    /// screen its latest, before the frame records. False when no GPU canvas is on screen.
    pub fn prepare<S>(&mut self, f: &mut Frame<'_, S>) -> bool {
        // Live runs replay on this window's device, or one made on it: provided before any shows.
        super::live::provide(&mut self.provided, f);
        LIVE_ANIMATING.set(false);
        self.on_screen.clear();
        for i in f.rt.instances.iter().filter(|i| i.params[1] == KIND_EFFECT) {
            let m = mark(i);
            if is_mark(m) {
                self.on_screen.push((m, i.rect, i.params[3]));
            }
        }
        self.animating = false;
        if self.on_screen.is_empty() {
            if self.gpu.as_ref().is_some_and(|g| !g.slots.is_empty()) {
                self.gpu = None;
                super::retain(|_| false);
            }
            return false;
        }
        let key = f.device_key();
        if self.gpu.as_ref().is_none_or(|g| g.key != key) {
            self.gpu = Some(Gpu::new(f.device, key));
        }
        let Some(gpu) = self.gpu.as_mut() else { return false };
        let on_screen = &self.on_screen;
        gpu.slots.retain(|m, _| on_screen.iter().any(|(s, ..)| s == m));
        // A `wgsl` canvas that stopped shows its placeholder: it isn't asked again.
        for (id, message, program) in gpu.shorthands.failed() {
            if let Ok(m) = id.parse() {
                gpu.slots.remove(&m);
                set_status(m, Status::Failed { message, program });
            }
        }
        // One too slow stays on its last frame: it isn't asked again.
        for id in gpu.shorthands.frozen() {
            if let Ok(m) = id.parse() {
                if let Some(s) = gpu.slots.get_mut(&m) {
                    s.phase = Phase::Frozen;
                }
                set_status(m, Status::Frozen);
            }
        }
        let shorthands = gpu.shorthands.canvases();
        let mut kept = HashSet::new();
        let mut asks = Vec::new();
        gpu.shown.clear();
        let clock = f.rt.clock;
        for &(m, rect, scale) in on_screen {
            if let Some((canvases, id)) = super::live_slot(m) {
                gpu.shown.push((m, canvases, id));
                continue;
            }
            let Some((wgsl, source, fps, duration, values, pointer, failed)) = with_slot(m, |s| (s.wgsl.clone(), s.source, s.fps, s.duration, s.values, s.pointer, !matches!(s.status, Status::Running | Status::Frozen))) else { continue };
            // Never at a zero width or height (in layout, or entering full screen): it waits.
            if failed || rect[2].round() < 1.0 || rect[3].round() < 1.0 {
                continue;
            }
            let id = m.to_string();
            if let Some(c) = &shorthands {
                if c.status(&id) == opencode_widget::gpu::Status::Unavailable {
                    set_status(m, Status::Unavailable);
                    continue;
                }
                gpu.shown.push((m, c.clone(), id.clone()));
            }
            let size: [u32; 2] = opencode_widget::gpu::clamp_size(rect[2].round() as u32, rect[3].round() as u32).into();
            let slot = gpu.slots.entry(m).or_insert(SlotGpu { size, phase: Phase::Fresh });
            let resized = slot.size != size;
            slot.size = size;
            // The slot's machine: whether this frame renders, and where it goes next.
            let (render, next) = match slot.phase {
                Phase::Fresh => (true, Phase::Running { start: clock, last: clock, values }),
                Phase::Frozen => (false, Phase::Frozen),
                Phase::Running { start, last, values: drawn } => {
                    // With `duration`, it animates until it drew a frame that old.
                    let animating = fps > 0.0 && duration.is_none_or(|d| last - start < d);
                    let due = if animating { clock - last >= 1.0 / fps - 0.002 } else { drawn != values };
                    let render = due || resized;
                    (render, Phase::Running { start, last: if render { clock } else { last }, values })
                }
            };
            slot.phase = next;
            kept.insert(id.clone());
            if let Phase::Running { start, last, .. } = next
                && fps > 0.0
                && duration.is_none_or(|d| last - start < d)
            {
                self.animating = true;
            }
            if !render {
                continue;
            }
            let start = match next {
                Phase::Running { start, .. } => start,
                _ => clock,
            };
            let mut uniforms = Uniforms { time: (clock - start) as f32, scale, pointer: [0.0; 4], values };
            if let Some(([x, y], down)) = pointer {
                uniforms.pointer = [x * scale, y * scale, if down { 1.0 } else { 0.0 }, 0.0];
            }
            asks.push((id, Ask { wgsl, source, size, uniforms }));
        }
        if kept != gpu.kept {
            gpu.kept = kept.clone();
            gpu.shorthands.keep(kept);
        }
        // What's finished shows first, then the next frames are asked for: each waits for this
        // frame's copies, done well before the next frame looks for it.
        gpu.live.prepare(f, &gpu.blit, &gpu.shown);
        for (id, ask) in asks {
            gpu.shorthands.ask(id, ask);
        }
        // Frames keep coming until what was asked for shows.
        self.animating |= gpu.live.animating || gpu.shorthands.pending();
        LIVE_ANIMATING.set(gpu.live.animating);
        true
    }

    /// Draws GPU canvas `site`'s latest frame shown into the open pass, scaled to its box.
    pub fn draw(&mut self, pass: &mut wgpu::RenderPass<'_>, site: &EffectSite<'_>, queue: &wgpu::Queue, device: &wgpu::Device) {
        let Some(gpu) = self.gpu.as_mut() else { return };
        let Some((uniform, bind, opaque)) = gpu.live.drawn(site.mark) else { return };
        let i = site.instance;
        let u = BlitUniform {
            rect: [i.rect[0] - site.origin[0], i.rect[1] - site.origin[1], i.rect[2], i.rect[3]],
            clip: i.clip,
            frame: [site.origin[0], site.origin[1], site.size[0] as f32, site.size[1] as f32],
            fade: i.fade,
            flags: [if site.srgb_blend { 1.0 } else { 0.0 }, if opaque { 1.0 } else { 0.0 }, 0.0, 0.0],
        };
        queue.write_buffer(uniform, 0, &u.bytes());
        let pipeline = gpu.blit.pipeline(device, site.format);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..4, 0..1);
    }
}

impl Blit {
    /// A slot's blit uniform and bind group, sampling `view`.
    pub(super) fn bind(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> (wgpu::Buffer, wgpu::BindGroup) {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("widget shader slot"), size: 80, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("widget shader slot"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        (uniform, bind)
    }

    fn new(device: &wgpu::Device) -> Blit {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("widget shader blit"),
            source: wgpu::ShaderSource::Wgsl(format!("{}\n{}", rune::render::CLIP_WGSL, include_str!("blit.wgsl")).into()),
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty, count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("widget shader blit"),
            entries: &[
                entry(0, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }),
                entry(1, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }),
                entry(2, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("widget shader blit"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("widget shader frame"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        Blit { layout, pipeline_layout, shader, sampler, pipelines: HashMap::new() }
    }

    fn pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        let (shader, layout) = (&self.shader, &self.pipeline_layout);
        self.pipelines.entry(format).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("widget shader blit"),
                layout: Some(layout),
                vertex: wgpu::VertexState { module: shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        })
    }
}

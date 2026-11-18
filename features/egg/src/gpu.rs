//! The egg's GPU side: its three effect slots on the new-session page and what draws in them. The
//! pins render into offscreen layers as Desktop's two canvases do (`pin-quads.ts`, MAX-blended
//! shadows first), then composite into their slots: `BACK` under everything on the panel with the
//! paddle over it, `FRONT` for the pins popped toward the viewer, under the composer. `MID` is the
//! wordmark's box: the flat letters, their shimmer and the lodger. Everything is built while the
//! egg moves, from its state, without the view running; nothing exists while it doesn't.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use rune::compose::{DeviceKey, Frame};
use rune::render::{EffectSite, Instance, KIND_EFFECT};

use super::{EggHost, draw};

const TAG: u32 = 0x4547 << 16;
pub const BACK: u32 = TAG | 1;
pub const MID: u32 = TAG | 2;
pub const FRONT: u32 = TAG | 3;

pub fn is_mark(mark: u32) -> bool {
    mark & 0xFFFF_0000 == TAG
}

thread_local! {
    static ANIMATING: Cell<bool> = const { Cell::new(false) };
}

/// The egg is moving: the window keeps drawing frames for it.
pub fn set_animating(on: bool) {
    ANIMATING.with(|a| a.set(on));
}

pub fn animating() -> bool {
    ANIMATING.with(Cell::get)
}

/// A shape instance (`egg.wgsl` `ShapeIn`).
pub type Shape = [f32; 24];

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct SlotUniform {
    pass: [f32; 4],
    clip: [f32; 4],
    fade: [u32; 4],
    flags: [f32; 4],
    dest: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct LayerUniform {
    view: [f32; 4],
    scale: [f32; 4],
}

/// What this frame draws, built from the egg's state before the frame records.
#[derive(Default)]
pub struct Scene {
    /// Each pin layer's region in panel points, its scale, and whether it shows.
    pub back: Option<draw::LayerAt>,
    pub front: Option<draw::LayerAt>,
    pub paddle: Vec<Shape>,
    pub mid: Vec<Shape>,
    /// The layers' quads, copied out of the egg's state so the frame can record while it's read.
    pub quads: [(Vec<super::pins::draw::Quad>, usize); 2],
}

#[derive(Default)]
pub struct EggGpu {
    gpu: Option<Gpu>,
    scene: Scene,
}

struct Gpu {
    key: DeviceKey,
    shapes: wgpu::ShaderModule,
    slot_layout: wgpu::BindGroupLayout,
    layer_layout: wgpu::BindGroupLayout,
    slot_pipeline_layout: wgpu::PipelineLayout,
    shape_pipes: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    blit_pipes: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    pin_max: wgpu::RenderPipeline,
    pin_over: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    layers: [Option<Layer>; 2],
    slots: HashMap<u32, SlotRes>,
}

/// An offscreen pin layer: its texture, its quads and its composite into the slot. The texture
/// lives as long as the egg, grown in 256 px steps and never shrunk, and each frame draws into its
/// top-left `size`: pins flying change the region every frame, and a texture a frame is a driver
/// allocation a frame.
struct Layer {
    view: wgpu::TextureView,
    size: [u32; 2],
    cap: [u32; 2],
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
    quads: Buffer,
    blit_uniform: wgpu::Buffer,
    blit_written: Option<SlotUniform>,
    blit_bind: wgpu::BindGroup,
}

/// A slot's uniform and shape buffer.
/// A frame has drawn on the GPU, where the egg's slots draw. Until then (a window's first CPU
/// frames, a browser without WebGPU) the egg stays in: its pins and lodger would draw nothing.
static ON_GPU: AtomicBool = AtomicBool::new(false);

pub fn on_gpu() -> bool {
    ON_GPU.load(Ordering::Relaxed)
}

struct SlotRes {
    uniform: wgpu::Buffer,
    /// What `uniform` holds: a write (a staging buffer on D3D12) only when it changes.
    written: Option<SlotUniform>,
    bind: wgpu::BindGroup,
    shapes: Buffer,
}

struct Buffer {
    buf: Option<wgpu::Buffer>,
    cap: u64,
    count: u32,
}

impl Buffer {
    fn new() -> Buffer {
        Buffer { buf: None, cap: 0, count: 0 }
    }

    fn upload<H: EggHost>(&mut self, f: &mut Frame<'_, H>, bytes: &[u8], count: u32, label: &'static str) {
        self.count = count;
        if bytes.is_empty() {
            return;
        }
        let need = (bytes.len() as u64).next_power_of_two().max(1024);
        if self.cap < bytes.len() as u64 || self.buf.is_none() {
            self.buf = Some(f.device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: need, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }));
            self.cap = need;
        }
        if let Some(buf) = &self.buf {
            f.write_buffer(buf, 0, bytes);
        }
    }
}

impl EggGpu {
    /// Lets go of the egg's pipelines, textures and the scene's buffers.
    fn release(&mut self) {
        if self.gpu.is_some() || self.scene.quads[0].0.capacity() > 0 {
            self.gpu = None;
            self.scene = Scene::default();
            super::mark("released", f64::NAN);
        }
    }

    pub fn draws_own(&self) -> bool {
        self.gpu.is_some()
    }

    pub fn animating(&self) -> bool {
        animating()
    }

    /// Builds and renders the egg's layers for this frame (and notes that frames draw on the GPU). False when no egg slot is on screen:
    /// everything the egg held on the GPU is let go.
    pub fn prepare<H: EggHost>(&mut self, f: &mut Frame<'_, H>) -> bool {
        if !ON_GPU.load(Ordering::Relaxed) {
            ON_GPU.store(true, Ordering::Relaxed);
        }
        let started = super::tracing().then(rune::time::Instant::now);
        let drew = self.prepare_frame(f);
        if let Some(t) = started {
            super::COST.with(|c| c.set([c.get()[0], t.elapsed().as_secs_f64() * 1e6]));
        }
        drew
    }

    fn prepare_frame<H: EggHost>(&mut self, f: &mut Frame<'_, H>) -> bool {
        // With no egg out, the app's frames pay one check here and nothing is held.
        let Some(live) = f.rt.state.egg().live.as_deref() else {
            self.release();
            return false;
        };
        let slots: Vec<(u32, [f32; 4], f32)> = f.rt.instances.iter().filter(|i| i.params[1] == KIND_EFFECT).map(|i| (mark(i), i.rect, i.params[3])).filter(|(m, ..)| is_mark(*m)).collect();
        if slots.is_empty() {
            self.release();
            return false;
        }
        let pal = session_view::palette::pal(live.geo.get().dark);
        draw::scene(&mut self.scene, live, pal, &slots, live.clock);
        let key = f.device_key();
        if self.gpu.as_ref().is_none_or(|g| g.key != key) {
            self.gpu = Some(Gpu::new(f.device, key));
        }
        let Some(gpu) = self.gpu.as_mut() else { return false };
        for (n, at) in [self.scene.back, self.scene.front].into_iter().enumerate() {
            // A layer with nothing to draw this frame keeps its texture for the next.
            let Some(at) = at else { continue };
            let (quads, shadows) = &self.scene.quads[n];
            gpu.render_layer(f, n, at, quads, *shadows);
        }
        for (mark, shapes) in [(BACK, &self.scene.paddle), (MID, &self.scene.mid)] {
            let res = gpu.slot(f.device, mark);
            res.shapes.upload(f, bytemuck::cast_slice(shapes), shapes.len() as u32, "egg shapes");
        }
        gpu.slot(f.device, FRONT);
        true
    }

    /// Draws the egg's slot `site` into the open pass.
    pub fn draw(&mut self, pass: &mut wgpu::RenderPass<'_>, site: &EffectSite<'_>, queue: &wgpu::Queue, device: &wgpu::Device) {
        let Some(gpu) = self.gpu.as_mut() else { return };
        let i = site.instance;
        let uniform = |dest: [f32; 4]| SlotUniform {
            pass: [site.origin[0], site.origin[1], site.size[0] as f32, site.size[1] as f32],
            clip: i.clip,
            fade: i.fade,
            flags: [if site.srgb_blend { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
            dest,
        };
        let layer = match site.mark {
            BACK => Some((0, self.scene.back)),
            FRONT => Some((1, self.scene.front)),
            _ => None,
        };
        if let Some((n, Some(at))) = layer
            && let Some(l) = &mut gpu.layers[n]
        {
            // The part of the texture this frame drew.
            let mut u = uniform(at.dest);
            u.flags[1] = l.size[0] as f32 / l.cap[0] as f32;
            u.flags[2] = l.size[1] as f32 / l.cap[1] as f32;
            if l.blit_written != Some(u) {
                queue.write_buffer(&l.blit_uniform, 0, bytemuck::bytes_of(&u));
                l.blit_written = Some(u);
            }
            let bind = l.blit_bind.clone();
            pass.set_pipeline(gpu.blit_pipe(device, site.format));
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..4, 0..1);
        }
        if site.mark == FRONT {
            return;
        }
        let Some(res) = gpu.slots.get_mut(&site.mark) else { return };
        let Some(buf) = &res.shapes.buf else { return };
        if res.shapes.count == 0 {
            return;
        }
        let u = uniform([0.0; 4]);
        if res.written != Some(u) {
            queue.write_buffer(&res.uniform, 0, bytemuck::bytes_of(&u));
            res.written = Some(u);
        }
        let count = res.shapes.count;
        let bind = res.bind.clone();
        let buf = buf.clone();
        pass.set_pipeline(gpu.shape_pipe(device, site.format));
        pass.set_bind_group(0, &bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(0..4, 0..count);
    }
}

/// An effect slot's tag, from the two 16-bit halves its record keeps it in (`El::effect`).
pub fn mark(i: &Instance) -> u32 {
    (i.border[0] as u32 & 0xFFFF) | (i.border[1] as u32 & 0xFFFF) << 16
}

impl Gpu {
    fn new(device: &wgpu::Device, key: DeviceKey) -> Gpu {
        let shapes = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("egg shapes"),
            source: wgpu::ShaderSource::Wgsl(format!("{}\n{}", rune::render::CLIP_WGSL, include_str!("egg.wgsl")).into()),
        });
        let pins = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("egg pins"), source: wgpu::ShaderSource::Wgsl(include_str!("pins.wgsl").into()) });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty, count: None };
        let uniform = wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None };
        let slot_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("egg slot"),
            entries: &[
                entry(0, uniform),
                entry(1, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }),
                entry(2, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        });
        let layer_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("egg layer"), entries: &[entry(0, uniform)] });
        let slot_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("egg slot"), bind_group_layouts: &[Some(&slot_layout)], immediate_size: 0 });
        let layer_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("egg layer"), bind_group_layouts: &[Some(&layer_layout)], immediate_size: 0 });
        let max = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Max };
        let pin_max = pin_pipeline(device, &pins, &layer_pipeline_layout, wgpu::BlendState { color: max, alpha: max });
        let pin_over = pin_pipeline(device, &pins, &layer_pipeline_layout, wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("egg layer"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        Gpu { key, shapes, slot_layout, layer_layout, slot_pipeline_layout, shape_pipes: HashMap::new(), blit_pipes: HashMap::new(), pin_max, pin_over, sampler, layers: [None, None], slots: HashMap::new() }
    }

    fn slot(&mut self, device: &wgpu::Device, mark: u32) -> &mut SlotRes {
        let (layout, sampler) = (&self.slot_layout, &self.sampler);
        self.slots.entry(mark).or_insert_with(|| {
            let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("egg slot"), size: std::mem::size_of::<SlotUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            // Shapes sample no texture; a 1x1 one fills the slot layout's binding.
            let dummy = device.create_texture(&wgpu::TextureDescriptor { label: Some("egg none"), size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] });
            let view = dummy.create_view(&wgpu::TextureViewDescriptor::default());
            let bind = slot_bind(device, layout, &uniform, &view, sampler);
            SlotRes { uniform, written: None, bind, shapes: Buffer::new() }
        })
    }

    fn render_layer<H: EggHost>(&mut self, f: &mut Frame<'_, H>, n: usize, at: draw::LayerAt, quads: &[super::pins::draw::Quad], shadows: usize) {
        let size = [at.pixels[0].max(1), at.pixels[1].max(1)];
        if self.layers[n].as_ref().is_none_or(|l| l.cap[0] < size[0] || l.cap[1] < size[1]) {
            let old = self.layers[n].as_ref().map_or([0, 0], |l| l.cap);
            let cap = [size[0].max(old[0]).next_multiple_of(256), size[1].max(old[1]).next_multiple_of(256)];
            self.layers[n] = Some(self.layer(f.device, cap));
        }
        let Some(l) = self.layers[n].as_mut() else { return };
        l.size = size;
        let u = LayerUniform { view: [at.region[0] as f32, at.region[1] as f32, size[0] as f32, size[1] as f32], scale: [at.scale, 0.0, 0.0, 0.0] };
        f.write_buffer(&l.uniform, 0, bytemuck::bytes_of(&u));
        l.quads.upload(f, bytemuck::cast_slice(quads), quads.len() as u32, "egg pins");
        let mut pass = f.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("egg pins"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &l.view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_viewport(0.0, 0.0, size[0] as f32, size[1] as f32, 0.0, 1.0);
        pass.set_scissor_rect(0, 0, size[0], size[1]);
        let Some(buf) = &l.quads.buf else { return };
        let count = l.quads.count;
        if count == 0 {
            return;
        }
        pass.set_bind_group(0, &l.bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        let shadows = (shadows as u32).min(count);
        if shadows > 0 {
            pass.set_pipeline(&self.pin_max);
            pass.draw(0..4, 0..shadows);
        }
        if count > shadows {
            pass.set_pipeline(&self.pin_over);
            pass.draw(0..4, shadows..count);
        }
    }

    fn layer(&self, device: &wgpu::Device, cap: [u32; 2]) -> Layer {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("egg pins"),
            size: wgpu::Extent3d { width: cap[0], height: cap[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("egg layer"), size: std::mem::size_of::<LayerUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("egg layer"), layout: &self.layer_layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }] });
        let blit_uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("egg blit"), size: std::mem::size_of::<SlotUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let blit_bind = slot_bind(device, &self.slot_layout, &blit_uniform, &view, &self.sampler);
        Layer { view, size: cap, cap, uniform, bind, quads: Buffer::new(), blit_uniform, blit_written: None, blit_bind }
    }

    fn shape_pipe(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        let (module, layout) = (&self.shapes, &self.slot_pipeline_layout);
        self.shape_pipes.entry(format).or_insert_with(|| {
            let attrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4];
            let buffers = [wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Shape>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &attrs }];
            pipeline(device, module, layout, "shape_vs", "shape_fs", &buffers, format, wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING)
        })
    }

    fn blit_pipe(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        let (module, layout) = (&self.shapes, &self.slot_pipeline_layout);
        self.blit_pipes.entry(format).or_insert_with(|| pipeline(device, module, layout, "blit_vs", "blit_fs", &[], format, wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING))
    }
}

fn slot_bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, uniform: &wgpu::Buffer, view: &wgpu::TextureView, sampler: &wgpu::Sampler) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("egg slot"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
}

fn pin_pipeline(device: &wgpu::Device, module: &wgpu::ShaderModule, layout: &wgpu::PipelineLayout, blend: wgpu::BlendState) -> wgpu::RenderPipeline {
    let attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x4];
    let buffers = [wgpu::VertexBufferLayout { array_stride: 40, step_mode: wgpu::VertexStepMode::Instance, attributes: &attrs }];
    pipeline(device, module, layout, "pins_vs", "pins_fs", &buffers, wgpu::TextureFormat::Rgba8Unorm, blend)
}

#[allow(clippy::too_many_arguments)]
fn pipeline(device: &wgpu::Device, module: &wgpu::ShaderModule, layout: &wgpu::PipelineLayout, vs: &str, fs: &str, buffers: &[wgpu::VertexBufferLayout<'_>], format: wgpu::TextureFormat, blend: wgpu::BlendState) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("egg"),
        layout: Some(layout),
        vertex: wgpu::VertexState { module, entry_point: Some(vs), compilation_options: Default::default(), buffers: &buffers.iter().cloned().map(Some).collect::<Vec<_>>() },
        fragment: Some(wgpu::FragmentState { module, entry_point: Some(fs), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL })] }),
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

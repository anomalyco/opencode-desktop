//! Shader widgets on the window's GPU, for the host's compositor: before the frame records,
//! each shader slot on screen that is due renders its template's WGSL (header + source +
//! entry, compiled once per source and shared by every slot with it) into a texture the size
//! the slot shows at; at the slot's place in Rune's pass that texture is drawn, clipped as Rune
//! clips. A slot due again at its `fps`, re-renders; with `fps: 0` only when its `values`
//! change. A frame over 50 ms three frames in a row freezes it on its last frame. Nothing is
//! allocated per frame: uniforms go through the frame's staging belt, textures and bind groups
//! live as long as their slot (resized only when the slot is).

use std::collections::HashMap;
use std::task::{Context, Poll, Waker};

use opencode_widget::shader::{self, SLOW_FRAME_MS, SLOW_FRAMES_TO_FREEZE, Uniforms};
use rune::compose::{DeviceKey, Frame};
use rune::render::{EffectSite, Instance, KIND_EFFECT};

use super::{Status, is_mark, set_status, with_slot};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[derive(Default)]
pub struct ShaderGpu {
    gpu: Option<Gpu>,
    /// The shader marks on screen this frame (kept to reuse its allocation).
    on_screen: Vec<(u32, [f32; 4], f32)>,
    animating: bool,
}

struct Gpu {
    key: DeviceKey,
    uniform_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    /// Compiled pipelines by source hash, or why one didn't compile.
    pipelines: HashMap<u64, Result<wgpu::RenderPipeline, (String, bool)>>,
    blit: Blit,
    slots: HashMap<u32, SlotGpu>,
}

/// One slot's GPU side and its drawing as an explicit machine.
struct SlotGpu {
    uniform: wgpu::Buffer,
    uniform_bind: wgpu::BindGroup,
    target: Option<Target>,
    phase: Phase,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Nothing rendered yet.
    Fresh,
    /// Renders every `1 / fps` (or, at `fps: 0`, when `values` change); `start` is when it
    /// first drew (`widget.time` counts from it), `slow` frames over budget in a row.
    Running { start: f64, last: f64, values: [f32; 16], slow: u32 },
    /// Stopped on its last frame.
    Frozen,
}

struct Target {
    view: wgpu::TextureView,
    size: [u32; 2],
    blit_uniform: wgpu::Buffer,
    blit_bind: wgpu::BindGroup,
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

struct Blit {
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

/// What `f` resolves to at once, as wgpu's error scopes do on native.
fn now<F: std::future::Future>(f: F) -> Option<F::Output> {
    let mut f = std::pin::pin!(f);
    match f.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(v) => Some(v),
        Poll::Pending => None,
    }
}

/// Compiles `source`'s module: checked with naga first, so a template's mistake comes back as
/// a message with its line rather than a device error.
/// The pipeline for `source`, or why not and whether that's the program's error: WGSL naga rejects
/// is; a device refusing WGSL that validated is this client's.
fn compile(device: &wgpu::Device, layout: &wgpu::PipelineLayout, source: &str) -> Result<wgpu::RenderPipeline, (String, bool)> {
    let program = |m: String| (m, true);
    let code = shader::module_with_vertex(source).map_err(program)?;
    // Lines as the template wrote them: the header comes first.
    let header = shader::HEADER.lines().count() as u32;
    let at = |line: u32| line.saturating_sub(header);
    let module = naga::front::wgsl::parse_str(&code).map_err(|e| match e.location(&code) {
        Some(l) => program(format!("line {}: {}", at(l.line_number), e.message())),
        None => program(e.message().to_owned()),
    })?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
        .validate(&module)
        .map_err(|e| program(e.emit_to_string(&code).lines().next().unwrap_or("the shader is invalid").trim_start_matches("error: ").to_owned()))?;
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("widget shader"), source: wgpu::ShaderSource::Wgsl(code.into()) });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("widget shader"),
        layout: Some(layout),
        vertex: wgpu::VertexState { module: &module, entry_point: Some("widget_vertex"), compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("widget_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format: FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    match now(scope.pop()) {
        Some(Some(e)) => Err((e.to_string(), false)),
        _ => Ok(pipeline),
    }
}

impl Gpu {
    fn new(device: &wgpu::Device, key: DeviceKey) -> Gpu {
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("widget shader"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("widget shader"), bind_group_layouts: &[Some(&uniform_layout)], immediate_size: 0 });
        Gpu { key, uniform_layout, pipeline_layout, pipelines: HashMap::new(), blit: Blit::new(device), slots: HashMap::new() }
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

    /// Renders the shader slots on screen that are due, before the frame records. False when no
    /// shader is on screen.
    pub fn prepare<S>(&mut self, f: &mut Frame<'_, S>) -> bool {
        self.on_screen.clear();
        for i in f.rt.instances.iter().filter(|i| i.params[1] == KIND_EFFECT) {
            let m = mark(i);
            if is_mark(m) {
                self.on_screen.push((m, i.rect, i.params[3]));
            }
        }
        self.animating = false;
        if !self.on_screen.is_empty() {
            super::PAINTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
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
        let clock = f.rt.clock;
        let slow_frame = f.dt * 1000.0 > SLOW_FRAME_MS;
        for &(m, rect, scale) in on_screen {
            let Some((source, fps, values, pointer, failed)) = with_slot(m, |s| (s.source, s.fps, s.values, s.pointer, !matches!(s.status, Status::Running))) else { continue };
            if failed {
                continue;
            }
            let pipeline = match gpu.pipelines.get(&source) {
                Some(p) => p,
                None => {
                    let wgsl = with_slot(m, |s| s.wgsl.clone()).unwrap_or_default();
                    let compiled = compile(f.device, &gpu.pipeline_layout, &wgsl);
                    gpu.pipelines.entry(source).or_insert(compiled)
                }
            };
            let pipeline = match pipeline {
                Ok(p) => p,
                Err((message, program)) => {
                    set_status(m, Status::Failed { message: message.clone(), program: *program });
                    continue;
                }
            };
            let size = [rect[2].round().max(1.0) as u32, rect[3].round().max(1.0) as u32];
            let device = f.device;
            let slot = gpu.slots.entry(m).or_insert_with(|| {
                let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("widget shader"), size: Uniforms::SIZE as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
                let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("widget shader"),
                    layout: &gpu.uniform_layout,
                    entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
                });
                SlotGpu { uniform, uniform_bind, target: None, phase: Phase::Fresh }
            });
            let resized = slot.target.as_ref().is_none_or(|t| t.size != size);
            if resized {
                slot.target = Some(Target::new(device, &gpu.blit, size));
            }
            // The slot's machine: whether this frame renders, and where it goes next.
            let (render, next) = match slot.phase {
                Phase::Fresh => (true, Phase::Running { start: clock, last: clock, values, slow: 0 }),
                Phase::Frozen => (resized, Phase::Frozen),
                Phase::Running { start, last, values: drawn, slow } => {
                    let slow = if slow_frame && fps > 0.0 { slow + 1 } else { 0 };
                    if slow >= SLOW_FRAMES_TO_FREEZE {
                        set_status(m, Status::Frozen);
                        (false, Phase::Frozen)
                    } else {
                        let due = if fps > 0.0 { clock - last >= 1.0 / fps - 0.002 } else { drawn != values };
                        let render = due || resized;
                        (render, Phase::Running { start, last: if render { clock } else { last }, values, slow })
                    }
                }
            };
            slot.phase = next;
            if fps > 0.0 && matches!(next, Phase::Running { .. }) {
                self.animating = true;
            }
            if !render {
                continue;
            }
            let start = match next {
                Phase::Running { start, .. } => start,
                _ => clock,
            };
            let mut u = Uniforms { resolution: [size[0] as f32, size[1] as f32], time: (clock - start) as f32, scale, pointer: [0.0; 4], values };
            if let Some(([x, y], down)) = pointer {
                u.pointer = [x * scale, y * scale, if down { 1.0 } else { 0.0 }, 0.0];
            }
            f.write_buffer(&slot.uniform, 0, &u.bytes());
            let Some(target) = slot.target.as_ref() else { continue };
            let mut pass = f.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("widget shader"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &slot.uniform_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        true
    }

    /// Draws shader slot `site`'s last frame into the open pass.
    pub fn draw(&mut self, pass: &mut wgpu::RenderPass<'_>, site: &EffectSite<'_>, queue: &wgpu::Queue, device: &wgpu::Device) {
        let Some(gpu) = self.gpu.as_mut() else { return };
        let Some(target) = gpu.slots.get(&site.mark).and_then(|s| s.target.as_ref()) else { return };
        let i = site.instance;
        let u = BlitUniform {
            rect: [i.rect[0] - site.origin[0], i.rect[1] - site.origin[1], i.rect[2], i.rect[3]],
            clip: i.clip,
            frame: [site.origin[0], site.origin[1], site.size[0] as f32, site.size[1] as f32],
            fade: i.fade,
            flags: [if site.srgb_blend { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0],
        };
        queue.write_buffer(&target.blit_uniform, 0, &u.bytes());
        let pipeline = gpu.blit.pipeline(device, site.format);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &target.blit_bind, &[]);
        pass.draw(0..4, 0..1);
    }
}

impl Target {
    fn new(device: &wgpu::Device, blit: &Blit, size: [u32; 2]) -> Target {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("widget shader frame"),
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let blit_uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("widget shader slot"), size: 80, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let blit_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("widget shader slot"),
            layout: &blit.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: blit_uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&blit.sampler) },
            ],
        });
        Target { view, size, blit_uniform, blit_bind }
    }
}

impl Blit {
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
